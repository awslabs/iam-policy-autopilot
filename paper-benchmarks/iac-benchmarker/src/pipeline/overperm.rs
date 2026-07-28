//! Per-language overpermissioning analysis (Phases 5b–5e).
//!
//! Contains helpers for counting concrete actions, running Access Analyzer,
//! and the per-language autopilot / LLM / iamfast overpermissioning phases.

use std::{
    collections::HashSet,
    fs,
    path::Path,
    process::Command,
};

use aws_sdk_accessanalyzer::Client as AaClient;
use aws_sdk_bedrockruntime::{Client as BedrockClient, types::Message as BedrockMessage};
use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;
use serde_json::Value;
use tracing::{info, warn};

use iac_runner::{
    generate_policies, language_configs,
    run_language_with_policies, RunPolicies,
};

use crate::aggregate::{LlmTrialResult, RunLanguageSummary};
use crate::llm_policy::{
    generate_llm_policy_for_language, validate_policy_with_access_analyzer,
    LlmPolicyOutcome, ResourcePromptStrategy,
};
use crate::managed_policies::extract_allow_statements;
use crate::report::LanguageOverpermissioning;
use crate::service_ref::{action_covered_by, ServiceCatalogue};

use super::types::{PipelineConfig, SharedContext};

// ---------------------------------------------------------------------------
// Unified overpermissioning helpers
// ---------------------------------------------------------------------------

/// Build a "no policy generated" result pair for a language.
pub(crate) fn empty_overperm_result(
    lang: &str,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
    validation_success: Option<bool>,
) -> (LanguageOverpermissioning, RunLanguageSummary) {
    let overperm = LanguageOverpermissioning {
        language: lang.to_string(),
        policy_generated: false,
        concrete_actions: 0,
        minimal_concrete_actions,
        managed_policy_concrete_actions,
        over_permission_ratio_vs_minimal: 0.0,
        over_permission_ratio_vs_managed: 0.0,
        validation_success,
        access_analyzer_error_count: 0,
        access_analyzer_warning_count: 0,
        access_analyzer_suggestion_count: 0,
    };
    let summary = RunLanguageSummary {
        language: lang.to_string(),
        policy_generated: false,
        concrete_actions: 0,
        over_permission_ratio_vs_minimal: 0.0,
        over_permission_ratio_vs_managed: 0.0,
        validation_success,
        access_analyzer_error_count: 0,
        access_analyzer_warning_count: 0,
        access_analyzer_suggestion_count: 0,
        llm_trials: vec![],
        total_input_tokens: 0.0,
        total_output_tokens: 0.0,
        total_tokens: 0.0,
    };
    (overperm, summary)
}

/// Build an overperm + summary pair from computed metrics.
pub(crate) fn build_overperm_result(
    lang: &str,
    concrete_actions: u32,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
    validation_success: Option<bool>,
    aa_error_count: usize,
    aa_warning_count: usize,
    aa_suggestion_count: usize,
) -> (LanguageOverpermissioning, RunLanguageSummary) {
    let ratio_vs_minimal = concrete_actions as f64 / minimal_concrete_actions.max(1) as f64;
    let ratio_vs_managed = concrete_actions as f64 / managed_policy_concrete_actions.max(1) as f64;

    let overperm = LanguageOverpermissioning {
        language: lang.to_string(),
        policy_generated: true,
        concrete_actions,
        minimal_concrete_actions,
        managed_policy_concrete_actions,
        over_permission_ratio_vs_minimal: ratio_vs_minimal,
        over_permission_ratio_vs_managed: ratio_vs_managed,
        validation_success,
        access_analyzer_error_count: aa_error_count,
        access_analyzer_warning_count: aa_warning_count,
        access_analyzer_suggestion_count: aa_suggestion_count,
    };
    let summary = RunLanguageSummary {
        language: lang.to_string(),
        policy_generated: true,
        concrete_actions,
        over_permission_ratio_vs_minimal: ratio_vs_minimal,
        over_permission_ratio_vs_managed: ratio_vs_managed,
        validation_success,
        access_analyzer_error_count: aa_error_count,
        access_analyzer_warning_count: aa_warning_count,
        access_analyzer_suggestion_count: aa_suggestion_count,
        llm_trials: vec![],
        total_input_tokens: 0.0,
        total_output_tokens: 0.0,
        total_tokens: 0.0,
    };
    (overperm, summary)
}

/// Count concrete IAM actions from a set of policy documents using the
/// service catalogue to expand wildcard patterns.
pub fn count_concrete_actions_from_policies(
    policies: &[Value],
    catalogue: &ServiceCatalogue,
) -> HashSet<String> {
    let mut allow_patterns: Vec<String> = Vec::new();
    for policy_doc in policies {
        for stmt in extract_allow_statements(policy_doc) {
            allow_patterns.extend(stmt.action_patterns);
        }
    }

    let mut concrete: HashSet<String> = HashSet::new();
    for pattern in &allow_patterns {
        let prefix = match pattern.find(':') {
            Some(pos) => pattern[..pos].to_lowercase(),
            None => continue,
        };
        if let Some(actions) = catalogue.get(&prefix) {
            for action in actions {
                let c = format!("{}:{}", prefix, action);
                if action_covered_by(pattern, &c) {
                    concrete.insert(c);
                }
            }
        } else {
            concrete.insert(pattern.clone());
        }
    }
    concrete
}

/// Run Access Analyzer validation on a set of policy documents and return
/// `(error_count, warning_count, suggestion_count)`.
pub(crate) async fn run_access_analyzer_on_policies(
    aa: &AaClient,
    policies: &[Value],
    run_name: &str,
    log_tag: &str,
    lang: &str,
    output_dir: &Path,
    file_prefix: &str,
) -> (usize, usize, usize) {
    let mut aa_error_count: usize = 0;
    let mut aa_warning_count: usize = 0;
    let mut aa_suggestion_count: usize = 0;
    let mut all_aa_findings = Vec::new();

    for policy_doc in policies {
        match validate_policy_with_access_analyzer(aa, policy_doc).await {
            Ok(findings) => {
                for f in &findings {
                    match f.finding_type.to_uppercase().as_str() {
                        "ERROR" => aa_error_count += 1,
                        "WARNING" => aa_warning_count += 1,
                        "SUGGESTION" => aa_suggestion_count += 1,
                        _ => {}
                    }
                }
                all_aa_findings.extend(findings);
            }
            Err(e) => {
                warn!(
                    "[{}][{}][{}] Access Analyzer validation failed: {:#}",
                    run_name, log_tag, lang, e
                );
            }
        }
    }

    info!(
        "[{}][{}][{}] Access Analyzer: {} errors, {} warnings, {} suggestions",
        run_name, log_tag, lang, aa_error_count, aa_warning_count, aa_suggestion_count
    );

    if !all_aa_findings.is_empty() {
        let aa_path = output_dir.join(format!("{}_access_analyzer.json", file_prefix));
        if let Ok(json) = serde_json::to_string_pretty(&all_aa_findings) {
            let _ = fs::write(&aa_path, json);
        }
    }

    (aa_error_count, aa_warning_count, aa_suggestion_count)
}

// ---------------------------------------------------------------------------
// Phase 5b: Per-language autopilot overpermissioning
// ---------------------------------------------------------------------------

/// Phase 5b: Generate autopilot policies for each language, count concrete
/// actions, and run Access Analyzer validation on each generated policy.
///
/// Returns `(language_overpermissioning, language_summaries_for_aggregate)`.
pub(crate) async fn run_autopilot_overpermissioning(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
    output_dir: &Path,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
) -> (Vec<LanguageOverpermissioning>, Vec<RunLanguageSummary>) {
    info!("[{}] === Phase 5b: Per-language autopilot overpermissioning ===", run_name);

    let mut overperm_results: Vec<LanguageOverpermissioning> = Vec::new();
    let mut summaries: Vec<RunLanguageSummary> = Vec::new();

    for lang in &cfg.languages {
        let lang_script_dir = run_dir.join(lang);
        let lang_configs = language_configs();
        let script_filename = lang_configs
            .get(lang.as_str())
            .map(|c| c.script_file)
            .unwrap_or("script");
        let script_path = lang_script_dir.join(script_filename);

        if !script_path.exists() {
            warn!(
                "[{}][Autopilot][{}] Script not found at {:?} — skipping",
                run_name, lang, script_path
            );
            let (o, s) = empty_overperm_result(
                lang, minimal_concrete_actions, managed_policy_concrete_actions, None,
            );
            overperm_results.push(o);
            summaries.push(s);
            continue;
        }

        info!("[{}][Autopilot][{}] Generating autopilot policies for {:?} ...", run_name, lang, script_path);
        let lang_policies = generate_policies(
            &script_path,
            &cfg.autopilot_binary,
            &ctx.region,
            &ctx.account,
        );

        match lang_policies {
            None => {
                warn!("[{}][Autopilot][{}] autopilot policy generation failed", run_name, lang);
                let (o, s) = empty_overperm_result(
                    lang, minimal_concrete_actions, managed_policy_concrete_actions, None,
                );
                overperm_results.push(o);
                summaries.push(s);
            }
            Some(policies) => {
                let lang_concrete = count_concrete_actions_from_policies(&policies, &ctx.catalogue);
                let autopilot_count = lang_concrete.len() as u32;

                info!(
                    "[{}][Autopilot][{}] Autopilot concrete actions: {}, minimal: {}, managed: {}",
                    run_name, lang, autopilot_count, minimal_concrete_actions,
                    managed_policy_concrete_actions
                );

                let autopilot_validation_dir = output_dir.join(format!("{}_autopilot_validation", lang));
                fs::create_dir_all(&autopilot_validation_dir).ok();

                let lang_policy_path = autopilot_validation_dir.join("policy.json");
                if let Ok(json) = serde_json::to_string_pretty(&policies) {
                    let _ = fs::write(&lang_policy_path, json);
                }
                // Also save at the legacy flat path for backwards compatibility.
                let legacy_policy_path = output_dir.join(format!("{}_autopilot_policy.json", lang));
                if let Ok(json) = serde_json::to_string_pretty(&policies) {
                    let _ = fs::write(&legacy_policy_path, json);
                }

                // Access Analyzer validation for autopilot policies
                let (aa_err, aa_warn, aa_suggest) = run_access_analyzer_on_policies(
                    &ctx.aa, &policies, run_name, "Autopilot", lang, output_dir,
                    &format!("{}_autopilot", lang),
                ).await;

                // Live execution validation for autopilot policies
                let validation_success = if !cfg.skip_validation {
                    let lang_configs = language_configs();
                    match lang_configs.get(lang.as_str()) {
                        None => {
                            warn!(
                                "[{}][Autopilot][{}] Unknown language config — skipping live validation",
                                run_name, lang
                            );
                            None
                        }
                        Some(lang_cfg) => {
                            info!(
                                "[{}][Autopilot][{}] Validating autopilot policy with live execution ...",
                                run_name, lang
                            );
                            let summary = run_language_with_policies(
                                lang,
                                run_dir,
                                &autopilot_validation_dir,
                                RunPolicies::InlineDocuments(policies.clone()),
                                &ctx.region,
                                &ctx.account,
                                cfg.no_cleanup_roles,
                                &ctx.iam,
                                &ctx.sts,
                                lang_cfg,
                            )
                            .await;

                            let log_path = autopilot_validation_dir.join("validation_log.json");
                            if let Ok(json) = serde_json::to_string_pretty(&summary) {
                                let _ = fs::write(&log_path, json);
                            }

                            if summary.success {
                                info!(
                                    "[{}][Autopilot][{}] Autopilot policy validation PASSED",
                                    run_name, lang
                                );
                            } else {
                                warn!(
                                    "[{}][Autopilot][{}] Autopilot policy validation FAILED: {:?}",
                                    run_name, lang, summary.failure_reason
                                );
                            }

                            Some(summary.success)
                        }
                    }
                } else {
                    info!("[{}][Autopilot][{}] --skip-validation: skipping live validation", run_name, lang);
                    None
                };

                let (o, s) = build_overperm_result(
                    lang, autopilot_count, minimal_concrete_actions,
                    managed_policy_concrete_actions, validation_success,
                    aa_err, aa_warn, aa_suggest,
                );
                overperm_results.push(o);
                summaries.push(s);
            }
        }
    }

    (overperm_results, summaries)
}

// ---------------------------------------------------------------------------
// Phase 5e: Per-language iamfast overpermissioning (static analysis)
// ---------------------------------------------------------------------------

/// Run `iamfast` on a single source file and return the parsed IAM policy
/// document, or `None` if the command fails or produces unparseable output.
fn run_iamfast_on_file(
    iamfast_binary: &str,
    script_path: &Path,
    run_name: &str,
    lang: &str,
) -> Option<Value> {
    info!(
        "[{}][iamfast][{}] Running iamfast on {:?} ...",
        run_name, lang, script_path
    );

    let output = Command::new("env")
        .arg("NODE_OPTIONS=--experimental-require-module")
        .arg(iamfast_binary)
        .arg(script_path)
        .output();

    match output {
        Err(e) => {
            warn!(
                "[{}][iamfast][{}] Failed to execute iamfast: {}",
                run_name, lang, e
            );
            None
        }
        Ok(out) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                warn!(
                    "[{}][iamfast][{}] iamfast exited with {}: {}",
                    run_name, lang, out.status, stderr
                );
                None
            } else {
                let stdout = String::from_utf8_lossy(&out.stdout);
                match serde_json::from_str::<Value>(&stdout) {
                    Ok(policy_doc) => {
                        if policy_doc.get("Statement").is_some() {
                            info!(
                                "[{}][iamfast][{}] Successfully parsed IAM policy",
                                run_name, lang
                            );
                            Some(policy_doc)
                        } else {
                            warn!(
                                "[{}][iamfast][{}] Output is valid JSON but not an IAM policy",
                                run_name, lang
                            );
                            None
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[{}][iamfast][{}] Failed to parse iamfast output as JSON: {}",
                            run_name, lang, e
                        );
                        None
                    }
                }
            }
        }
    }
}

/// Phase 5e: Run iamfast (static analysis) for each language, count concrete
/// actions, and run Access Analyzer validation on each generated policy.
///
/// Returns `(iamfast_language_overpermissioning, iamfast_language_summaries)`.
pub(crate) async fn run_iamfast_overpermissioning(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
    output_dir: &Path,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
) -> (Vec<LanguageOverpermissioning>, Vec<RunLanguageSummary>) {
    info!("[{}] === Phase 5e: Per-language iamfast overpermissioning ===", run_name);

    let mut overperm_results: Vec<LanguageOverpermissioning> = Vec::new();
    let mut summaries: Vec<RunLanguageSummary> = Vec::new();

    if cfg.skip_iamfast {
        info!("[{}] --skip-iamfast: skipping iamfast policy generation", run_name);
        return (overperm_results, summaries);
    }

    for lang in &cfg.iamfast_languages {
        let lang_script_dir = run_dir.join(lang);
        let lang_configs = language_configs();
        let script_filename = lang_configs
            .get(lang.as_str())
            .map(|c| c.script_file)
            .unwrap_or("script");
        let script_path = lang_script_dir.join(script_filename);

        if !script_path.exists() {
            warn!(
                "[{}][iamfast][{}] Script not found at {:?} — skipping",
                run_name, lang, script_path
            );
            let (o, s) = empty_overperm_result(
                lang, minimal_concrete_actions, managed_policy_concrete_actions, None,
            );
            overperm_results.push(o);
            summaries.push(s);
            continue;
        }

        match run_iamfast_on_file(&cfg.iamfast_binary, &script_path, run_name, lang) {
            None => {
                warn!("[{}][iamfast][{}] iamfast policy generation failed", run_name, lang);
                let (o, s) = empty_overperm_result(
                    lang, minimal_concrete_actions, managed_policy_concrete_actions, None,
                );
                overperm_results.push(o);
                summaries.push(s);
            }
            Some(policy_doc) => {
                let policies = vec![policy_doc.clone()];
                let lang_concrete = count_concrete_actions_from_policies(&policies, &ctx.catalogue);
                let iamfast_count = lang_concrete.len() as u32;

                info!(
                    "[{}][iamfast][{}] iamfast concrete actions: {}, minimal: {}, managed: {}",
                    run_name, lang, iamfast_count, minimal_concrete_actions,
                    managed_policy_concrete_actions
                );

                // Save the policy document.
                let iamfast_validation_dir = output_dir.join(format!("{}_iamfast_validation", lang));
                fs::create_dir_all(&iamfast_validation_dir).ok();
                let lang_policy_path = iamfast_validation_dir.join("policy.json");
                if let Ok(json) = serde_json::to_string_pretty(&policy_doc) {
                    let _ = fs::write(&lang_policy_path, json);
                }
                // Also save at the legacy flat path for backwards compatibility.
                let legacy_policy_path = output_dir.join(format!("{}_iamfast_policy.json", lang));
                if let Ok(json) = serde_json::to_string_pretty(&policy_doc) {
                    let _ = fs::write(&legacy_policy_path, json);
                }

                // Access Analyzer validation for iamfast policies.
                let (aa_err, aa_warn, aa_suggest) = run_access_analyzer_on_policies(
                    &ctx.aa, &policies, run_name, "iamfast", lang, output_dir,
                    &format!("{}_iamfast", lang),
                ).await;

                // Live execution validation for iamfast policies.
                let validation_success = if !cfg.skip_validation {
                    let lang_configs = language_configs();
                    match lang_configs.get(lang.as_str()) {
                        None => {
                            warn!(
                                "[{}][iamfast][{}] Unknown language config — skipping live validation",
                                run_name, lang
                            );
                            None
                        }
                        Some(lang_cfg) => {
                            info!(
                                "[{}][iamfast][{}] Validating iamfast policy with live execution ...",
                                run_name, lang
                            );
                            let summary = run_language_with_policies(
                                lang,
                                run_dir,
                                &iamfast_validation_dir,
                                RunPolicies::InlineDocuments(vec![policy_doc.clone()]),
                                &ctx.region,
                                &ctx.account,
                                cfg.no_cleanup_roles,
                                &ctx.iam,
                                &ctx.sts,
                                lang_cfg,
                            )
                            .await;

                            let log_path = iamfast_validation_dir.join("validation_log.json");
                            if let Ok(json) = serde_json::to_string_pretty(&summary) {
                                let _ = fs::write(&log_path, json);
                            }

                            if summary.success {
                                info!(
                                    "[{}][iamfast][{}] iamfast policy validation PASSED",
                                    run_name, lang
                                );
                            } else {
                                warn!(
                                    "[{}][iamfast][{}] iamfast policy validation FAILED: {:?}",
                                    run_name, lang, summary.failure_reason
                                );
                            }

                            Some(summary.success)
                        }
                    }
                } else {
                    info!("[{}][iamfast][{}] --skip-validation: skipping live validation", run_name, lang);
                    None
                };

                let (o, s) = build_overperm_result(
                    lang, iamfast_count, minimal_concrete_actions,
                    managed_policy_concrete_actions, validation_success,
                    aa_err, aa_warn, aa_suggest,
                );
                overperm_results.push(o);
                summaries.push(s);
            }
        }
    }

    (overperm_results, summaries)
}

// ---------------------------------------------------------------------------
// Phase 5c/5c-cdk/5d: Per-language LLM overpermissioning (unified, with repetitions)
// ---------------------------------------------------------------------------

/// Phase 5c/5d: Generate LLM policies for each language, repeating
/// `cfg.llm_repetitions` times to account for LLM nondeterminism.
///
/// The two LLM experiments share this function:
/// - **Phase 5c** ("LLM"): script-only prompt, no prior messages.
/// - **Phase 5d** ("CTX-LLM"): script-only prompt + context scenarios,
///   prior messages from scenario files.
///
/// All trial results are recorded in [`RunLanguageSummary::llm_trials`].
/// The **median** trial (by `over_permission_ratio_vs_minimal`) is used as
/// the representative value for the summary fields.
///
/// Returns `(llm_language_overpermissioning, llm_language_summaries)`.
pub(crate) async fn run_llm_overpermissioning(
    log_tag: &str,
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    run_dir: &Path,
    output_dir: &Path,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
    strategy: ResourcePromptStrategy,
    prior_messages_fn: impl Fn(&str) -> Vec<BedrockMessage>,
) -> (Vec<LanguageOverpermissioning>, Vec<RunLanguageSummary>) {
    info!("[{}] === {}: Per-language LLM overpermissioning ({} repetitions, strategy={}) ===",
          run_name, log_tag, cfg.llm_repetitions, strategy);

    let mut overperm_results: Vec<LanguageOverpermissioning> = Vec::new();
    let mut summaries: Vec<RunLanguageSummary> = Vec::new();

    if cfg.skip_llm {
        info!("[{}] --skip-llm: skipping {} policy generation", run_name, log_tag);
        return (overperm_results, summaries);
    }

    let n_reps = cfg.llm_repetitions.max(1);

    for lang in &cfg.languages {
        let lang_script_dir = run_dir.join(lang);
        let lang_configs = language_configs();
        let script_filename = lang_configs
            .get(lang.as_str())
            .map(|c| c.script_file)
            .unwrap_or("script");
        let script_path = lang_script_dir.join(script_filename);

        if !script_path.exists() {
            warn!("[{}][{}][{}] Script not found at {:?} — skipping", run_name, log_tag, lang, script_path);
            let (o, mut s) = empty_overperm_result(
                lang, minimal_concrete_actions, managed_policy_concrete_actions, Some(false),
            );
            s.llm_trials = vec![];
            overperm_results.push(o);
            summaries.push(s);
            continue;
        }

        // Run N repetitions and collect all trial outcomes.
        let mut trials: Vec<LlmTrialResult> = Vec::with_capacity(n_reps);

        for trial_idx in 1..=n_reps {
            info!(
                "[{}][{}][{}] Trial {}/{} ...",
                run_name, log_tag, lang, trial_idx, n_reps
            );

            let prior_messages = prior_messages_fn(lang);
            let trial_dir = output_dir.join(format!(
                "{}_{}_trial_{:02}",
                lang,
                log_tag.to_lowercase(),
                trial_idx
            ));
            let outcome = run_llm_for_language(
                log_tag, run_name, lang, &script_path,
                prior_messages,
                None,
                &trial_dir,
                minimal_concrete_actions, managed_policy_concrete_actions,
                &ctx.bedrock, &ctx.aa, &ctx.catalogue, &ctx.bedrock_model_id,
                cfg.skip_validation, run_dir, &ctx.region, &ctx.account,
                cfg.no_cleanup_roles, &ctx.iam, &ctx.sts,
                strategy,
            ).await;

            let trial_result = LlmTrialResult {
                trial: trial_idx,
                policy_generated: outcome.policy_generated,
                concrete_actions: outcome.concrete_actions,
                over_permission_ratio_vs_minimal: outcome.over_permission_ratio_vs_minimal,
                over_permission_ratio_vs_managed: outcome.over_permission_ratio_vs_managed,
                validation_success: outcome.validation_success,
                access_analyzer_error_count: outcome.access_analyzer_error_count,
                access_analyzer_warning_count: outcome.access_analyzer_warning_count,
                access_analyzer_suggestion_count: outcome.access_analyzer_suggestion_count,
                input_tokens: outcome.token_usage.input_tokens,
                output_tokens: outcome.token_usage.output_tokens,
                total_tokens: outcome.token_usage.total_tokens,
            };

            // Persist per-trial metadata for human-readable inspection and
            // as a fallback if run_summary.json is lost.
            let meta_path = trial_dir.join("trial_meta.json");
            if let Ok(json) = serde_json::to_string_pretty(&trial_result) {
                let _ = std::fs::write(&meta_path, json);
            }

            trials.push(trial_result);
        }

        // Pick the median trial (by over_permission_ratio_vs_minimal among
        // trials that generated a policy) as the representative value.
        let generated_trials: Vec<&LlmTrialResult> = trials.iter()
            .filter(|t| t.policy_generated)
            .collect();

        if generated_trials.is_empty() {
            warn!(
                "[{}][{}][{}] All {} trials failed to generate a policy",
                run_name, log_tag, lang, n_reps
            );
            let (o, mut s) = empty_overperm_result(
                lang, minimal_concrete_actions, managed_policy_concrete_actions, Some(false),
            );
            s.llm_trials = trials;
            overperm_results.push(o);
            summaries.push(s);
            continue;
        }

        // Sort by ratio_vs_minimal to find median.
        let mut sorted_by_ratio: Vec<&LlmTrialResult> = generated_trials.clone();
        sorted_by_ratio.sort_by(|a, b| {
            a.over_permission_ratio_vs_minimal
                .partial_cmp(&b.over_permission_ratio_vs_minimal)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let median_trial = sorted_by_ratio[sorted_by_ratio.len() / 2];

        let pass_count = generated_trials.iter().filter(|t| t.validation_success).count();
        let total_generated = generated_trials.len();

        info!(
            "[{}][{}][{}] {} trials: median ratio={:.2}x, pass={}/{}, generated={}/{}",
            run_name, log_tag, lang, n_reps,
            median_trial.over_permission_ratio_vs_minimal,
            pass_count, total_generated, total_generated, n_reps,
        );

        // Compute mean token usage per trial across all trials.
        let n_trials = trials.len() as f64;
        let total_input_tokens: f64 = trials.iter().map(|t| t.input_tokens as f64).sum::<f64>() / n_trials;
        let total_output_tokens: f64 = trials.iter().map(|t| t.output_tokens as f64).sum::<f64>() / n_trials;
        let total_tokens: f64 = trials.iter().map(|t| t.total_tokens as f64).sum::<f64>() / n_trials;

        let (o, mut s) = build_overperm_result(
            lang,
            median_trial.concrete_actions,
            minimal_concrete_actions,
            managed_policy_concrete_actions,
            Some(median_trial.validation_success),
            median_trial.access_analyzer_error_count,
            median_trial.access_analyzer_warning_count,
            median_trial.access_analyzer_suggestion_count,
        );
        s.llm_trials = trials;
        s.total_input_tokens = total_input_tokens;
        s.total_output_tokens = total_output_tokens;
        s.total_tokens = total_tokens;
        overperm_results.push(o);
        summaries.push(s);
    }

    (overperm_results, summaries)
}

// ---------------------------------------------------------------------------
// Shared LLM helper
// ---------------------------------------------------------------------------

/// Run the LLM policy generation + optional live validation for a single language.
///
/// Both Phase 5c (simple prompt, `prior_messages = vec![]`) and Phase 5d
/// (context-filled prompt) share identical logic; only the prior messages and
/// the output subdirectory differ.
#[allow(clippy::too_many_arguments)]
async fn run_llm_for_language(
    log_tag: &str,
    run_name: &str,
    lang: &str,
    script_path: &Path,
    prior_messages: Vec<BedrockMessage>,
    cdk_stack_path: Option<&Path>,
    output_subdir: &Path,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
    bedrock: &BedrockClient,
    aa: &AaClient,
    catalogue: &ServiceCatalogue,
    model_id: &str,
    skip_validation: bool,
    run_dir: &Path,
    region: &str,
    account: &str,
    no_cleanup_roles: bool,
    iam: &IamClient,
    sts: &StsClient,
    strategy: ResourcePromptStrategy,
) -> LlmPolicyOutcome {
    info!(
        "[{}][{}][{}] Generating LLM policy via Bedrock ...",
        run_name, log_tag, lang
    );

    let mut outcome = generate_llm_policy_for_language(
        bedrock,
        aa,
        catalogue,
        lang,
        script_path,
        model_id,
        prior_messages,
        cdk_stack_path,
        minimal_concrete_actions,
        managed_policy_concrete_actions,
        strategy,
    )
    .await;

    // Create the output subdirectory unconditionally so all artefacts land together.
    fs::create_dir_all(output_subdir).ok();

    // Save the policy document.
    if let Some(ref policy_doc) = outcome.policy_document {
        let policy_path = output_subdir.join("policy.json");
        if let Ok(json) = serde_json::to_string_pretty(policy_doc) {
            let _ = fs::write(&policy_path, json);
            info!(
                "[{}][{}][{}] Saved policy to {:?}",
                run_name, log_tag, lang, policy_path
            );
        }
    }

    // Save Access Analyzer findings (only when non-empty).
    if !outcome.access_analyzer_findings.is_empty() {
        let aa_path = output_subdir.join("access_analyzer.json");
        if let Ok(json) = serde_json::to_string_pretty(&outcome.access_analyzer_findings) {
            let _ = fs::write(&aa_path, json);
        }
    }

    // Optionally validate with live execution.
    if !skip_validation && outcome.policy_generated {
        if let Some(ref policy_doc) = outcome.policy_document.clone() {
            let lang_configs = language_configs();
            match lang_configs.get(lang) {
                None => {
                    warn!(
                        "[{}][{}][{}] Unknown language config — skipping validation",
                        run_name, log_tag, lang
                    );
                }
                Some(lang_cfg) => {
                    info!(
                        "[{}][{}][{}] Validating policy with live execution ...",
                        run_name, log_tag, lang
                    );
                    let summary = run_language_with_policies(
                        lang,
                        run_dir,
                        output_subdir,
                        RunPolicies::InlineDocuments(vec![policy_doc.clone()]),
                        region,
                        account,
                        no_cleanup_roles,
                        iam,
                        sts,
                        lang_cfg,
                    )
                    .await;

                    let log_path = output_subdir.join("validation_log.json");
                    if let Ok(json) = serde_json::to_string_pretty(&summary) {
                        let _ = fs::write(&log_path, json);
                    }

                    outcome.validation_success = summary.success;

                    if summary.success {
                        info!(
                            "[{}][{}][{}] Policy validation PASSED",
                            run_name, log_tag, lang
                        );
                    } else {
                        warn!(
                            "[{}][{}][{}] Policy validation FAILED: {:?}",
                            run_name, log_tag, lang, summary.failure_reason
                        );
                    }
                }
            }
        }
    }

    info!(
        "[{}][{}][{}] concrete={}, ratio_vs_minimal={:.2}x, ratio_vs_managed={:.2}x, \
         validation={}, aa_errors={}, aa_warnings={}",
        run_name, log_tag, lang,
        outcome.concrete_actions,
        outcome.over_permission_ratio_vs_minimal,
        outcome.over_permission_ratio_vs_managed,
        outcome.validation_success,
        outcome.access_analyzer_error_count,
        outcome.access_analyzer_warning_count,
    );

    outcome
}
