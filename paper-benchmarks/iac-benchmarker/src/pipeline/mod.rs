//! Core benchmark pipeline for a single run directory.
//!
//! This module contains [`run_single()`] which orchestrates the full benchmark
//! pipeline (phases 1-6) for one `run_dir`, as well as [`build_aggregate_report()`]
//! for producing the cross-run aggregate statistics.
//!
//! The implementation is split across submodules:
//! - [`types`] — shared context, config, and intermediate result structs
//! - [`phases`] — phase 1–5 implementations (load, filter, set-cover, CDK, validate, count)
//! - [`overperm`] — per-language overpermissioning analysis (autopilot, LLM, iamfast)
//! - [`report_builder`] — phase 6 report generation
//! - [`aggregate_builder`] — cross-run aggregate report

mod aggregate_builder;
mod overperm;
mod phases;
mod report_builder;
pub mod types;

// Re-export public API so external callers (main.rs) see the same interface.
pub use aggregate_builder::build_aggregate_report;
pub use overperm::count_concrete_actions_from_policies;
pub use types::{PipelineConfig, SharedContext};

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use aws_sdk_bedrockruntime::types::Message as BedrockMessage;
use chrono::Utc;
use tracing::{info, warn};

use crate::aggregate::{RunLanguageSummary, RunSummary};
use crate::report::LanguageOverpermissioning;
use crate::scenarios::load_scenario_context_messages;

use phases::{
    cdk_deploy_phase, cdk_destroy_phase, count_actions, load_minimal_policy,
    prefilter_candidates, run_set_cover, validate_managed_policies, validate_minimal_policy,
};
use overperm::{
    run_autopilot_overpermissioning, run_iamfast_overpermissioning, run_llm_overpermissioning,
};
use report_builder::build_report;

// ---------------------------------------------------------------------------
// Orchestrator: run_single
// ---------------------------------------------------------------------------

/// Run the full benchmark pipeline (phases 1-6) for one `run_dir`.
///
/// Requires a pre-built [`SharedContext`] so that AWS clients, the service
/// catalogue, managed-policy cache, and policy index are not re-fetched for
/// every run in batch mode.
///
/// Writes `benchmark_managed_policies.json` (and validation logs) to
/// `output_dir`, and returns a [`RunSummary`] for aggregation.
///
/// **Graceful shutdown:** If `ctx.shutdown` is cancelled (e.g. via SIGINT),
/// remaining validation phases are skipped but CDK destroy still runs to
/// clean up AWS resources.
pub async fn run_single(
    ctx: &SharedContext,
    cfg: &PipelineConfig,
    run_dir: &Path,
    output_dir: &Path,
) -> Result<RunSummary> {
    let timestamp = Utc::now().to_rfc3339();

    let run_name = run_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Phase 0: Validate that the CDK stack file exists.
    // The stack definition is required for LLM prompt context (e.g. knowing
    // whether an S3 bucket uses KMS encryption affects the required permissions).
    let cdk_stack_path = run_dir.join("cdk/lib/stack.ts");
    if !cdk_stack_path.is_file() {
        anyhow::bail!(
            "[{}] CDK stack file not found at {:?}.\n\
             Every run directory must contain cdk/lib/stack.ts.",
            run_name, cdk_stack_path
        );
    }
    info!("[{}] CDK stack file found: {:?}", run_name, cdk_stack_path);

    // Phase 1: Load minimal policy
    let mpd = load_minimal_policy(&run_name, run_dir, &ctx.catalogue)?;

    // Phase 2: Pre-filter candidates
    let cd = prefilter_candidates(
        &run_name,
        &ctx.index,
        &mpd.all_minimal_actions,
        &mpd.required_action_to_resources,
    );

    // Phase 3: Set-cover
    let cover_mode = cfg.cover_mode.to_lowercase();
    let (cover_result, set_cover_coverage_pct) = run_set_cover(
        &run_name,
        &cover_mode,
        &ctx.index,
        &cd.candidates,
        &cd.coverable_required_actions,
        &mpd.required_action_to_resources,
    );

    // Phase 3.5: CDK deploy
    // If shutdown was requested before CDK deploy starts, skip the entire run.
    // No resources have been created yet, so there's nothing to clean up.
    if ctx.shutdown.is_cancelled() {
        warn!(
            "[{}] Shutdown requested before CDK deploy — skipping entire run (no cleanup needed)",
            run_name
        );
        anyhow::bail!("Run cancelled before CDK deploy");
    }
    cdk_deploy_phase(&run_name, cfg, ctx, run_dir).await?;

    // -----------------------------------------------------------------------
    // From this point on, CDK destroy MUST run (even on cancellation).
    // We track whether CDK was deployed so the destroy guard fires correctly.
    // -----------------------------------------------------------------------
    let cdk_deployed = !cfg.skip_deploy;

    // Helper: check if shutdown was requested and log once.
    let cancelled = |phase: &str| -> bool {
        if ctx.shutdown.is_cancelled() {
            warn!("[{}] Shutdown requested — skipping {}", run_name, phase);
            true
        } else {
            false
        }
    };

    // Phase 4: Empirical validation
    let final_selected = cover_result.selected_arns.clone();
    let (validation_success, validation_attempts) = if cancelled("Phase 4 (managed-policy validation)") {
        (false, 0)
    } else {
        validate_managed_policies(
            &run_name,
            cfg,
            ctx,
            run_dir,
            output_dir,
            &final_selected,
            &cd.coverable_required_actions,
        )
        .await?
    };

    // Phase 4b: Validate minimal_policy.json
    let minimal_policy_validation_success = if cancelled("Phase 4b (minimal-policy validation)") {
        false
    } else {
        validate_minimal_policy(
            &run_name,
            cfg,
            ctx,
            run_dir,
            output_dir,
            &mpd.minimal_policy_docs,
        )
        .await?
    };

    // Phase 5: Action count comparison (pure computation, always runs)
    let action_counts = count_actions(
        &run_name,
        &ctx.index,
        &ctx.catalogue,
        &cover_result.selected_arns,
        &mpd.minimal_allow_patterns,
    );

    // Phase 5b: Per-language autopilot overpermissioning
    let (language_overpermissioning, language_summaries_for_aggregate) =
        if cancelled("Phase 5b (autopilot overperm)") {
            (vec![], vec![])
        } else {
            run_autopilot_overpermissioning(
                &run_name,
                cfg,
                ctx,
                run_dir,
                output_dir,
                action_counts.minimal_concrete_actions,
                action_counts.managed_policy_concrete_actions,
            )
            .await
        };

    // Phase 5c/5d: LLM overpermissioning — matrix of context scenarios × prompt strategies.
    //
    // Context scenarios:
    //   S1 "LLM"     — script-only, no context messages
    //   S2 "CTX-LLM" — script + context scenario messages
    //
    // Prompt strategies (from cfg.resource_prompt_strategies):
    //   bare, wildcards, resource-star
    //
    // Each combination produces a composite tag like "LLM/bare", "CTX-LLM/wildcards", etc.

    let scenarios_dir = ctx.context_scenarios_dir.clone();

    // Define the context scenarios as (tag, requires_scenarios_dir).
    struct ContextScenario {
        tag: &'static str,
        /// Returns `true` if this scenario requires a scenarios dir to be configured.
        requires_scenarios_dir: bool,
    }

    let context_scenarios = vec![
        ContextScenario { tag: "LLM",     requires_scenarios_dir: false },
        ContextScenario { tag: "CTX-LLM", requires_scenarios_dir: true },
    ];

    let mut all_llm_results: BTreeMap<String, (Vec<LanguageOverpermissioning>, Vec<RunLanguageSummary>)>
        = BTreeMap::new();

    for scenario in &context_scenarios {
        for strategy in &cfg.resource_prompt_strategies {
            let combo_tag = format!("{}/{}", scenario.tag, strategy);

            if cancelled(&format!("LLM experiment {}", combo_tag)) {
                all_llm_results.insert(combo_tag, (vec![], vec![]));
                continue;
            }

            // Skip scenarios that require a scenarios dir when none is configured.
            if scenario.requires_scenarios_dir && scenarios_dir.is_none() {
                if !cfg.skip_llm {
                    info!("[{}] No context scenarios dir configured — skipping {}", run_name, combo_tag);
                }
                all_llm_results.insert(combo_tag, (vec![], vec![]));
                continue;
            }

            let sd_clone = scenarios_dir.clone();
            let run_name_clone = run_name.clone();
            let scenario_tag = scenario.tag;

            let prior_messages_fn: Box<dyn Fn(&str) -> Vec<BedrockMessage>> = if scenario.requires_scenarios_dir {
                // CTX-LLM: load scenario context messages.
                let sd = sd_clone.unwrap();
                Box::new(move |lang: &str| -> Vec<BedrockMessage> {
                    let msgs = load_scenario_context_messages(&sd, lang);
                    info!(
                        "[{}][{}][{}] Loaded {} context messages from scenario dir",
                        run_name_clone, scenario_tag, lang, msgs.len()
                    );
                    msgs
                })
            } else {
                // LLM (script-only): no prior messages.
                Box::new(|_lang: &str| -> Vec<BedrockMessage> { vec![] })
            };

            let (overperm, summaries) = run_llm_overpermissioning(
                &combo_tag,
                &run_name,
                cfg,
                ctx,
                run_dir,
                output_dir,
                action_counts.minimal_concrete_actions,
                action_counts.managed_policy_concrete_actions,
                *strategy,
                |lang| prior_messages_fn(lang),
            )
            .await;

            all_llm_results.insert(combo_tag, (overperm, summaries));
        }
    }

    // Phase 5e: Per-language iamfast overpermissioning (static analysis)
    let (iamfast_language_overpermissioning, iamfast_language_summaries_for_aggregate) =
        if cancelled("Phase 5e (iamfast overperm)") {
            (vec![], vec![])
        } else {
            run_iamfast_overpermissioning(
                &run_name,
                cfg,
                ctx,
                run_dir,
                output_dir,
                action_counts.minimal_concrete_actions,
                action_counts.managed_policy_concrete_actions,
            )
            .await
        };

    // Phase 4.5: CDK destroy — ALWAYS runs if CDK was deployed.
    if cdk_deployed {
        info!("[{}] Running CDK destroy (guaranteed cleanup) ...", run_name);
    }
    cdk_destroy_phase(&run_name, cfg, ctx, run_dir).await;

    // Split the all_llm_results BTreeMap into separate maps for overperm and summaries.
    let mut llm_experiments_overperm: BTreeMap<String, Vec<LanguageOverpermissioning>> = BTreeMap::new();
    let mut llm_experiment_summaries: BTreeMap<String, Vec<RunLanguageSummary>> = BTreeMap::new();
    for (tag, (overperm, summaries)) in all_llm_results {
        llm_experiments_overperm.insert(tag.clone(), overperm);
        llm_experiment_summaries.insert(tag, summaries);
    }

    // Phase 6: Write report (with whatever data we collected)
    build_report(
        &run_name,
        cfg,
        ctx,
        output_dir,
        &cover_result,
        &cd.coverable_required_actions,
        &cd.uncoverable_actions,
        &mpd.required_action_to_resources,
        mpd.all_minimal_actions.len(),
        cd.candidates.len(),
        set_cover_coverage_pct,
        validation_success,
        validation_attempts,
        minimal_policy_validation_success,
        &action_counts,
        language_overpermissioning,
        llm_experiments_overperm,
        iamfast_language_overpermissioning,
        &timestamp,
    )?;

    // Build and return the RunSummary for aggregation.
    Ok(RunSummary {
        run_name,
        minimal_concrete_actions: action_counts.minimal_concrete_actions,
        managed_policy_concrete_actions: action_counts.managed_policy_concrete_actions,
        over_permission_ratio_managed_vs_minimal: action_counts.over_permission_ratio,
        selected_managed_policy_count: cover_result.selected_arns.len(),
        set_cover_coverage_pct,
        language_summaries: language_summaries_for_aggregate,
        llm_experiment_summaries,
        iamfast_language_summaries: iamfast_language_summaries_for_aggregate,
    })
}
