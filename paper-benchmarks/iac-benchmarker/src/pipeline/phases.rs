//! Core pipeline phases 1–5 (excluding per-language overpermissioning).
//!
//! Each function implements one numbered phase of the benchmark pipeline:
//! - Phase 1: Load minimal policy
//! - Phase 2: Pre-filter candidates
//! - Phase 3: Set-cover
//! - Phase 3.5: CDK deploy
//! - Phase 4: Empirical validation (managed policies)
//! - Phase 4b: Validate minimal_policy.json
//! - Phase 4.5: CDK destroy
//! - Phase 5: Action count comparison

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::{error, info, warn};

use iac_runner::{
    cdk_deploy, cdk_destroy, language_configs,
    run_language_with_policies, RunPolicies,
};

use crate::managed_policies::extract_allow_statements;
use crate::policy_index::PolicyIndex;
use crate::policy_index::index_denies_action;
use crate::service_ref::{action_covered_by, ServiceCatalogue};
use crate::set_cover::{
    actions_covered_by_policy_with_resources, exact_set_cover_with_resources,
    greedy_set_cover_with_resources, min_actions_cover_with_resources, SetCoverResult,
};

use super::types::{
    ActionCounts, CandidateData, MinimalPolicyData, PipelineConfig, SharedContext,
};

// ---------------------------------------------------------------------------
// Phase 1
// ---------------------------------------------------------------------------

/// Phase 1: Load the minimal policy from the run directory and expand
/// wildcard patterns into concrete actions using the service catalogue.
pub(crate) fn load_minimal_policy(
    run_name: &str,
    run_dir: &Path,
    catalogue: &ServiceCatalogue,
) -> Result<MinimalPolicyData> {
    info!("[{}] === Phase 1: Load minimal policy ===", run_name);

    let minimal_policy_path = run_dir.join("minimal_policy.json");
    if !minimal_policy_path.exists() {
        anyhow::bail!(
            "No minimal_policy.json found in {:?}.\n\
             Each run directory must contain a minimal_policy.json ground truth \
             (the repo's integration-tests/projects/run_* directories ship one).",
            run_dir
        );
    }
    info!("[{}] Loading minimal_policy.json from {:?}", run_name, minimal_policy_path);
    let minimal_policy_doc: Value = serde_json::from_str(
        &std::fs::read_to_string(&minimal_policy_path)
            .with_context(|| format!("Failed to read {:?}", minimal_policy_path))?,
    )
    .with_context(|| format!("Failed to parse {:?}", minimal_policy_path))?;
    let minimal_policy_docs = vec![minimal_policy_doc];

    let mut minimal_allow_patterns: Vec<String> = Vec::new();
    let mut required_action_to_resources: HashMap<String, Vec<String>> = HashMap::new();

    for doc in &minimal_policy_docs {
        for stmt in extract_allow_statements(doc) {
            minimal_allow_patterns.extend(stmt.action_patterns.clone());
            for action_pat in &stmt.action_patterns {
                required_action_to_resources
                    .entry(action_pat.clone())
                    .or_default()
                    .extend(stmt.resource_patterns.clone());
            }
        }
    }
    info!("[{}] Minimal allow patterns: {}", run_name, minimal_allow_patterns.len());

    let mut all_minimal_actions: HashSet<String> = HashSet::new();
    for pattern in &minimal_allow_patterns {
        let prefix = match pattern.find(':') {
            Some(pos) => pattern[..pos].to_lowercase(),
            None => continue,
        };
        let res_patterns: Vec<String> = required_action_to_resources
            .get(pattern)
            .cloned()
            .unwrap_or_default();

        if let Some(actions) = catalogue.get(&prefix) {
            for action in actions {
                let concrete = format!("{}:{}", prefix, action);
                if action_covered_by(pattern, &concrete) {
                    if !res_patterns.is_empty() {
                        required_action_to_resources
                            .entry(concrete.clone())
                            .or_default()
                            .extend(res_patterns.clone());
                    }
                    all_minimal_actions.insert(concrete);
                }
            }
        } else {
            all_minimal_actions.insert(pattern.clone());
        }
    }
    info!("[{}] All minimal concrete actions: {}", run_name, all_minimal_actions.len());

    Ok(MinimalPolicyData {
        minimal_policy_docs,
        minimal_allow_patterns,
        required_action_to_resources,
        all_minimal_actions,
    })
}

// ---------------------------------------------------------------------------
// Phase 2
// ---------------------------------------------------------------------------

/// Phase 2: Pre-filter candidate managed policies to those that cover at
/// least one required action and do not deny any required action.
pub(crate) fn prefilter_candidates(
    run_name: &str,
    index: &PolicyIndex,
    all_minimal_actions: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> CandidateData {
    info!("[{}] === Phase 2: Pre-filter candidates ===", run_name);

    let coverable_required_actions: HashSet<String> = all_minimal_actions
        .iter()
        .filter(|action| {
            let prefix = match action.find(':') {
                Some(pos) => action[..pos].to_lowercase(),
                None => return false,
            };
            if let Some(arns) = index.service_prefix_to_policy_arns.get(&prefix) {
                arns.iter().any(|arn| {
                    let single: HashSet<String> =
                        std::iter::once((*action).clone()).collect();
                    !actions_covered_by_policy_with_resources(
                        index,
                        arn,
                        &single,
                        required_action_to_resources,
                    )
                    .is_empty()
                })
            } else {
                false
            }
        })
        .cloned()
        .collect();

    let uncoverable_actions: HashSet<String> = all_minimal_actions
        .difference(&coverable_required_actions)
        .cloned()
        .collect();

    info!(
        "[{}] Coverable: {}, Uncoverable: {}",
        run_name,
        coverable_required_actions.len(),
        uncoverable_actions.len()
    );

    let mut candidate_arns: HashSet<String> = HashSet::new();
    for action in &coverable_required_actions {
        let prefix = match action.find(':') {
            Some(pos) => action[..pos].to_lowercase(),
            None => continue,
        };
        if let Some(arns) = index.service_prefix_to_policy_arns.get(&prefix) {
            candidate_arns.extend(arns.iter().cloned());
        }
    }

    let candidates: Vec<String> = candidate_arns
        .into_iter()
        .filter(|arn| {
            if arn.contains(":policy/aws-service-role/") {
                return false;
            }
            for action in &coverable_required_actions {
                if index_denies_action(index, arn, action) {
                    info!(
                        "[{}] Excluding {} — it denies required action {}",
                        run_name, arn, action
                    );
                    return false;
                }
            }
            !actions_covered_by_policy_with_resources(
                index,
                arn,
                &coverable_required_actions,
                required_action_to_resources,
            )
            .is_empty()
        })
        .collect();

    info!("[{}] Candidate managed policies: {}", run_name, candidates.len());

    CandidateData {
        candidates,
        coverable_required_actions,
        uncoverable_actions,
    }
}

// ---------------------------------------------------------------------------
// Phase 3
// ---------------------------------------------------------------------------

/// Phase 3: Run the set-cover algorithm to find the minimum set of managed
/// policies that covers all coverable required actions.
pub(crate) fn run_set_cover(
    run_name: &str,
    cover_mode: &str,
    index: &PolicyIndex,
    candidates: &[String],
    coverable_required_actions: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> (SetCoverResult, f64) {
    info!("[{}] === Phase 3: Set-cover ===", run_name);

    let cover_result = if coverable_required_actions.is_empty() {
        info!("[{}] No coverable actions — skipping set-cover", run_name);
        SetCoverResult {
            selected_arns: vec![],
            covered_actions: HashSet::new(),
            uncovered_actions: HashSet::new(),
        }
    } else if cover_mode == "exact" {
        info!("[{}] Running exact (branch-and-bound) set-cover ({} candidates) ...", run_name, candidates.len());
        exact_set_cover_with_resources(
            index, candidates, coverable_required_actions,
            required_action_to_resources,
        )
    } else if cover_mode == "min-actions" {
        info!("[{}] Running min-actions (branch-and-bound) set-cover ({} candidates) ...", run_name, candidates.len());
        min_actions_cover_with_resources(
            index, candidates, coverable_required_actions,
            required_action_to_resources,
        )
    } else {
        if cover_mode != "greedy" {
            warn!("[{}] Unknown --cover-mode '{}'; using greedy", run_name, cover_mode);
        }
        info!("[{}] Running greedy set-cover ({} candidates) ...", run_name, candidates.len());
        greedy_set_cover_with_resources(
            index, candidates, coverable_required_actions,
            required_action_to_resources,
        )
    };

    info!(
        "[{}] Set-cover selected {} policies, covered {}/{} actions",
        run_name,
        cover_result.selected_arns.len(),
        cover_result.covered_actions.len(),
        coverable_required_actions.len()
    );

    let set_cover_coverage_pct = if coverable_required_actions.is_empty() {
        100.0
    } else {
        (cover_result.covered_actions.len() as f64 / coverable_required_actions.len() as f64)
            * 100.0
    };

    (cover_result, set_cover_coverage_pct)
}

// ---------------------------------------------------------------------------
// Phase 3.5: CDK deploy
// ---------------------------------------------------------------------------

/// Phase 3.5: CDK deploy (stack must be up for all live-execution phases).
pub(crate) async fn cdk_deploy_phase(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
) -> Result<()> {
    if cfg.skip_validation {
        info!("[{}] --skip-validation: skipping CDK deploy", run_name);
    } else if cfg.skip_deploy {
        info!("[{}] --skip-deploy: assuming stack is already deployed", run_name);
    } else {
        info!("[{}] === Phase 3.5: CDK deploy ===", run_name);
        let cdk_role = cfg.cdk_role_arn.as_deref();
        match cdk_deploy(run_dir, cdk_role, &ctx.region, &ctx.sts).await {
            Ok(true) => info!("[{}] CDK deploy succeeded", run_name),
            Ok(false) => anyhow::bail!("[{}] CDK deploy failed — aborting benchmark", run_name),
            Err(e) => anyhow::bail!("[{}] CDK deploy error: {}", run_name, e),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 4: Validate managed policies
// ---------------------------------------------------------------------------

/// Phase 4: Validate the set-cover result with live AWS execution.
/// Returns `(validation_success, validation_attempts)`.
pub(crate) async fn validate_managed_policies(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
    output_dir: &Path,
    final_selected: &[String],
    coverable_required_actions: &HashSet<String>,
) -> Result<(bool, usize)> {
    info!("[{}] === Phase 4: Empirical validation ===", run_name);

    if cfg.skip_validation {
        info!("[{}] --skip-validation: skipping live AWS execution", run_name);
        return Ok((false, 0));
    }
    if final_selected.is_empty() && coverable_required_actions.is_empty() {
        info!("[{}] No policies selected (no coverable actions) — skipping validation", run_name);
        return Ok((false, 0));
    }

    let lang_configs = language_configs();
    let lang_cfg = match lang_configs.get(cfg.language.as_str()) {
        Some(c) => c.clone(),
        None => {
            error!("[{}] Unknown language: {}", run_name, cfg.language);
            anyhow::bail!("Unknown language: {}", cfg.language);
        }
    };

    let attempt_results_dir = output_dir.join("validation_attempt_1");
    fs::create_dir_all(&attempt_results_dir).ok();

    info!(
        "[{}] Validating set-cover result with {} policies ...",
        run_name, final_selected.len()
    );

    let attempt_policies_path = attempt_results_dir.join("attempted_policies.json");
    if let Ok(json) = serde_json::to_string_pretty(&final_selected) {
        let _ = fs::write(&attempt_policies_path, json);
    }

    let summary = run_language_with_policies(
        &cfg.language,
        run_dir,
        &attempt_results_dir,
        RunPolicies::ManagedArns(final_selected.to_vec()),
        &ctx.region,
        &ctx.account,
        cfg.no_cleanup_roles,
        &ctx.iam,
        &ctx.sts,
        &lang_cfg,
    )
    .await;

    let log_path = output_dir.join("validation_log_attempt_1.json");
    if let Ok(json) = serde_json::to_string_pretty(&summary) {
        let _ = fs::write(&log_path, json);
    }

    if summary.success {
        info!("[{}] Validation succeeded", run_name);
        Ok((true, 1))
    } else {
        warn!(
            "[{}] Managed-policy validation FAILED: {:?}. \
             Continuing with remaining phases so the run is still included in the aggregate report.",
            run_name, summary.failure_reason
        );
        Ok((false, 1))
    }
}

// ---------------------------------------------------------------------------
// Phase 4b: Validate minimal policy
// ---------------------------------------------------------------------------

/// Phase 4b: Validate the minimal_policy.json itself with live execution.
pub(crate) async fn validate_minimal_policy(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
    output_dir: &Path,
    minimal_policy_docs: &[Value],
) -> Result<bool> {
    info!("[{}] === Phase 4b: Validate minimal_policy.json ===", run_name);

    if cfg.skip_validation {
        info!("[{}] --skip-validation: skipping minimal_policy.json validation", run_name);
        return Ok(false);
    }

    let lang_configs = language_configs();
    let lang_cfg = match lang_configs.get(cfg.language.as_str()) {
        Some(c) => c.clone(),
        None => {
            error!("[{}] Unknown language: {}", run_name, cfg.language);
            anyhow::bail!("Unknown language: {}", cfg.language);
        }
    };

    let minimal_results_dir = output_dir.join("minimal_policy_validation");
    fs::create_dir_all(&minimal_results_dir).ok();

    info!(
        "[{}] Validating minimal_policy.json with language '{}' ...",
        run_name, cfg.language
    );

    let minimal_docs = vec![minimal_policy_docs[0].clone()];

    let minimal_summary = run_language_with_policies(
        &cfg.language,
        run_dir,
        &minimal_results_dir,
        RunPolicies::InlineDocuments(minimal_docs),
        &ctx.region,
        &ctx.account,
        cfg.no_cleanup_roles,
        &ctx.iam,
        &ctx.sts,
        &lang_cfg,
    )
    .await;

    let log_path = output_dir.join("minimal_policy_validation_log.json");
    if let Ok(json) = serde_json::to_string_pretty(&minimal_summary) {
        let _ = fs::write(&log_path, json);
    }

    if minimal_summary.success {
        info!("[{}] Minimal policy validation PASSED", run_name);
        Ok(true)
    } else {
        warn!(
            "[{}] Minimal policy validation FAILED: {:?}",
            run_name, minimal_summary.failure_reason
        );
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Phase 4.5: CDK destroy
// ---------------------------------------------------------------------------

/// Phase 4.5: CDK destroy (runs AFTER all validation phases so the stack is
/// available for LLM policy live-execution validation).
pub(crate) async fn cdk_destroy_phase(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
) {
    if cfg.skip_validation {
        // no deploy happened, nothing to destroy
    } else if cfg.skip_destroy {
        info!("[{}] --skip-destroy: leaving stack deployed", run_name);
    } else {
        info!("[{}] === Phase 4.5: CDK destroy ===", run_name);
        let cdk_role = cfg.cdk_role_arn.as_deref();
        match cdk_destroy(run_dir, cdk_role, &ctx.region, &ctx.sts).await {
            Ok(true) => info!("[{}] CDK destroy succeeded", run_name),
            Ok(false) => warn!("[{}] CDK destroy reported failure — stack may still be deployed", run_name),
            Err(e) => warn!("[{}] CDK destroy error: {}", run_name, e),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 5: Action count comparison
// ---------------------------------------------------------------------------

/// Phase 5: Count concrete actions allowed by the managed-policy set and
/// the minimal policy, and compute the over-permission ratio.
pub(crate) fn count_actions(
    run_name: &str,
    index: &PolicyIndex,
    catalogue: &ServiceCatalogue,
    selected_arns: &[String],
    minimal_allow_patterns: &[String],
) -> ActionCounts {
    info!("[{}] === Phase 5: Action count comparison ===", run_name);

    let mut managed_concrete: HashSet<String> = HashSet::new();
    for arn in selected_arns {
        if let Some(patterns) = index.policy_arn_to_allow_patterns.get(arn) {
            for pattern in patterns {
                let prefix = match pattern.find(':') {
                    Some(pos) => pattern[..pos].to_lowercase(),
                    None => continue,
                };
                if let Some(actions) = catalogue.get(&prefix) {
                    for action in actions {
                        let concrete = format!("{}:{}", prefix, action);
                        if action_covered_by(pattern, &concrete) {
                            managed_concrete.insert(concrete);
                        }
                    }
                }
            }
        }
    }
    let managed_policy_concrete_actions = managed_concrete.len() as u32;

    let mut minimal_concrete: HashSet<String> = HashSet::new();
    for pattern in minimal_allow_patterns {
        let prefix = match pattern.find(':') {
            Some(pos) => pattern[..pos].to_lowercase(),
            None => continue,
        };
        if let Some(actions) = catalogue.get(&prefix) {
            for action in actions {
                let concrete = format!("{}:{}", prefix, action);
                if action_covered_by(pattern, &concrete) {
                    minimal_concrete.insert(concrete);
                }
            }
        }
    }
    let minimal_concrete_actions = minimal_concrete.len() as u32;

    let over_permission_ratio =
        managed_policy_concrete_actions as f64 / minimal_concrete_actions.max(1) as f64;

    info!(
        "[{}] Minimal concrete actions: {}, Managed concrete actions: {}, Ratio: {:.2}",
        run_name, minimal_concrete_actions, managed_policy_concrete_actions, over_permission_ratio
    );

    ActionCounts {
        minimal_concrete_actions,
        managed_policy_concrete_actions,
        over_permission_ratio,
    }
}
