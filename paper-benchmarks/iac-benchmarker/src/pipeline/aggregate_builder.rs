//! Cross-run aggregate report builder.
//!
//! Produces an [`AggregateReport`] from a collection of per-run summaries,
//! computing per-language statistics across all benchmark runs.
//!
//! For LLM-based approaches with multiple trials per run, the aggregate
//! statistics use the **mean** overpermissioning ratio across all trials
//! (not just the median representative) so that the box plot and stats table
//! reflect the full distribution of LLM nondeterminism.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use tracing::warn;

use crate::aggregate::{
    AggregateReport, LanguageAggregate, RunLanguageSummary, RunSummary, Stats,
};

/// Build a [`LanguageAggregate`] from a slice of [`RunLanguageSummary`] for
/// a given language.  Shared by autopilot, LLM, and context-LLM aggregation.
///
/// When `use_trials` is true and summaries contain [`LlmTrialResult`] data,
/// the aggregate statistics are computed over the **per-run mean** of all
/// generated trials (one data point per run).  This gives each benchmark run
/// equal weight while capturing LLM variance within each run.
///
/// Validation counts use all individual trials when available.
fn build_language_aggregate(
    lang: &str,
    all_summaries: &[&RunLanguageSummary],
    include_validation: bool,
    use_trials: bool,
) -> Option<LanguageAggregate> {
    let matching: Vec<&&RunLanguageSummary> = all_summaries
        .iter()
        .filter(|ls| ls.language == lang)
        .collect();

    let generated: Vec<&&RunLanguageSummary> = matching
        .iter()
        .filter(|ls| ls.policy_generated)
        .copied()
        .collect();

    let generation_failures = matching
        .iter()
        .filter(|ls| !ls.policy_generated)
        .count();

    if generated.is_empty() {
        warn!("[{}] No successful policy generations — skipping language aggregate", lang);
        return None;
    }

    // When trials are available, use per-run mean of all generated trials
    // as the data point for that run.  This gives each benchmark run equal
    // weight while capturing LLM variance.
    let (ratio_vs_minimal_vals, ratio_vs_managed_vals, concrete_vals, aa_error_vals, aa_warning_vals) =
        if use_trials && generated.iter().any(|ls| !ls.llm_trials.is_empty()) {
            let mut rvm = Vec::new();
            let mut rvmg = Vec::new();
            let mut ca = Vec::new();
            let mut aae = Vec::new();
            let mut aaw = Vec::new();

            for ls in &generated {
                let gen_trials: Vec<_> = ls.llm_trials.iter()
                    .filter(|t| t.policy_generated)
                    .collect();
                if gen_trials.is_empty() {
                    // Fall back to the summary-level (median) value.
                    rvm.push(ls.over_permission_ratio_vs_minimal);
                    rvmg.push(ls.over_permission_ratio_vs_managed);
                    ca.push(ls.concrete_actions as f64);
                    aae.push(ls.access_analyzer_error_count as f64);
                    aaw.push(ls.access_analyzer_warning_count as f64);
                } else {
                    let n = gen_trials.len() as f64;
                    rvm.push(gen_trials.iter().map(|t| t.over_permission_ratio_vs_minimal).sum::<f64>() / n);
                    rvmg.push(gen_trials.iter().map(|t| t.over_permission_ratio_vs_managed).sum::<f64>() / n);
                    ca.push(gen_trials.iter().map(|t| t.concrete_actions as f64).sum::<f64>() / n);
                    aae.push(gen_trials.iter().map(|t| t.access_analyzer_error_count as f64).sum::<f64>() / n);
                    aaw.push(gen_trials.iter().map(|t| t.access_analyzer_warning_count as f64).sum::<f64>() / n);
                }
            }

            (rvm, rvmg, ca, aae, aaw)
        } else {
            // No trials — use the single summary value per run.
            let rvm: Vec<f64> = generated.iter().map(|ls| ls.over_permission_ratio_vs_minimal).collect();
            let rvmg: Vec<f64> = generated.iter().map(|ls| ls.over_permission_ratio_vs_managed).collect();
            let ca: Vec<f64> = generated.iter().map(|ls| ls.concrete_actions as f64).collect();
            let aae: Vec<f64> = generated.iter().map(|ls| ls.access_analyzer_error_count as f64).collect();
            let aaw: Vec<f64> = generated.iter().map(|ls| ls.access_analyzer_warning_count as f64).collect();
            (rvm, rvmg, ca, aae, aaw)
        };

    let ratio_vs_minimal = Stats::compute(&ratio_vs_minimal_vals)?;
    let ratio_vs_managed = Stats::compute(&ratio_vs_managed_vals)?;
    let concrete_actions = Stats::compute(&concrete_vals)?;
    let access_analyzer_errors = Stats::compute(&aa_error_vals);
    let access_analyzer_warnings = Stats::compute(&aa_warning_vals);

    // Token usage stats — per-run mean tokens per trial (only meaningful for LLM-based approaches).
    // When trial data is available, recompute the mean from the per-trial token counts
    // so that re-aggregation from old run_summary.json files (which stored sums) produces
    // the correct per-trial mean values.
    let (input_token_vals, output_token_vals, total_token_vals): (Vec<f64>, Vec<f64>, Vec<f64>) =
        if use_trials && matching.iter().any(|ls| !ls.llm_trials.is_empty()) {
            let mut inv = Vec::new();
            let mut outv = Vec::new();
            let mut totv = Vec::new();
            for ls in &matching {
                if !ls.llm_trials.is_empty() {
                    let n = ls.llm_trials.len() as f64;
                    inv.push(ls.llm_trials.iter().map(|t| t.input_tokens as f64).sum::<f64>() / n);
                    outv.push(ls.llm_trials.iter().map(|t| t.output_tokens as f64).sum::<f64>() / n);
                    totv.push(ls.llm_trials.iter().map(|t| t.total_tokens as f64).sum::<f64>() / n);
                } else {
                    // Fallback: use the summary-level value (already mean for new data, sum for old).
                    inv.push(ls.total_input_tokens);
                    outv.push(ls.total_output_tokens);
                    totv.push(ls.total_tokens);
                }
            }
            (inv, outv, totv)
        } else {
            let inv: Vec<f64> = matching.iter().map(|ls| ls.total_input_tokens).collect();
            let outv: Vec<f64> = matching.iter().map(|ls| ls.total_output_tokens).collect();
            let totv: Vec<f64> = matching.iter().map(|ls| ls.total_tokens).collect();
            (inv, outv, totv)
        };
    let has_token_data = total_token_vals.iter().any(|&v| v > 0.0);
    let input_tokens = if has_token_data { Stats::compute(&input_token_vals) } else { None };
    let output_tokens = if has_token_data { Stats::compute(&output_token_vals) } else { None };
    let total_tokens = if has_token_data { Stats::compute(&total_token_vals) } else { None };

    // Validation counts: when trials are available, count across all trials.
    let (validation_successes, validation_attempts) = if include_validation {
        if use_trials && generated.iter().any(|ls| !ls.llm_trials.is_empty()) {
            let mut successes = 0usize;
            let mut attempts = 0usize;
            for ls in &generated {
                if ls.llm_trials.is_empty() {
                    // Single-trial fallback.
                    attempts += 1;
                    if ls.validation_success == Some(true) {
                        successes += 1;
                    }
                } else {
                    let gen_trials: Vec<_> = ls.llm_trials.iter()
                        .filter(|t| t.policy_generated)
                        .collect();
                    attempts += gen_trials.len();
                    successes += gen_trials.iter().filter(|t| t.validation_success).count();
                }
            }
            (Some(successes), Some(attempts))
        } else {
            let successes = generated
                .iter()
                .filter(|ls| ls.validation_success == Some(true))
                .count();
            (Some(successes), Some(generated.len()))
        }
    } else {
        (None, None)
    };

    Some(LanguageAggregate {
        language: lang.to_string(),
        ratio_vs_minimal,
        ratio_vs_managed,
        concrete_actions,
        generation_failures,
        validation_successes,
        validation_attempts,
        access_analyzer_errors,
        access_analyzer_warnings,
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

/// Build the [`AggregateReport`] from a collection of per-run summaries.
pub fn build_aggregate_report(
    runs_dir: &Path,
    total_runs: usize,
    successful_runs: usize,
    run_summaries: Vec<RunSummary>,
) -> Result<AggregateReport> {
    let minimal_vals: Vec<f64> = run_summaries
        .iter()
        .map(|r| r.minimal_concrete_actions as f64)
        .collect();
    let managed_vals: Vec<f64> = run_summaries
        .iter()
        .map(|r| r.managed_policy_concrete_actions as f64)
        .collect();
    let managed_ratio_vals: Vec<f64> = run_summaries
        .iter()
        .map(|r| r.over_permission_ratio_managed_vs_minimal)
        .collect();

    let minimal_stats = Stats::compute(&minimal_vals)
        .ok_or_else(|| anyhow::anyhow!("No minimal action data"))?;
    let managed_stats = Stats::compute(&managed_vals)
        .ok_or_else(|| anyhow::anyhow!("No managed action data"))?;
    let managed_ratio_stats = Stats::compute(&managed_ratio_vals)
        .ok_or_else(|| anyhow::anyhow!("No managed ratio data"))?;

    let all_languages = ["python", "go", "java", "typescript"];

    // Per-language autopilot aggregates (no trials — deterministic).
    let autopilot_refs: Vec<&RunLanguageSummary> = run_summaries
        .iter()
        .flat_map(|r| r.language_summaries.iter())
        .collect();
    let mut language_aggregates: Vec<LanguageAggregate> = Vec::new();
    for lang in &all_languages {
        if let Some(agg) = build_language_aggregate(lang, &autopilot_refs, true, false) {
            language_aggregates.push(agg);
        }
    }

    // Per-experiment LLM aggregates — iterate over all experiment tags found
    // across all runs (use trials when available).
    // Collect all unique experiment tags from all runs.
    let mut all_experiment_tags: Vec<String> = run_summaries
        .iter()
        .flat_map(|r| r.llm_experiment_summaries.keys().cloned())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();
    all_experiment_tags.sort();

    let mut llm_experiment_aggregates: BTreeMap<String, Vec<LanguageAggregate>> = BTreeMap::new();
    for tag in &all_experiment_tags {
        let refs: Vec<&RunLanguageSummary> = run_summaries
            .iter()
            .flat_map(|r| {
                r.llm_experiment_summaries
                    .get(tag)
                    .map(|v| v.iter())
                    .unwrap_or_else(|| [].iter())
            })
            .collect();

        let mut tag_aggregates: Vec<LanguageAggregate> = Vec::new();
        for lang in &all_languages {
            if let Some(agg) = build_language_aggregate(lang, &refs, true, true) {
                tag_aggregates.push(agg);
            }
        }
        if !tag_aggregates.is_empty() {
            llm_experiment_aggregates.insert(tag.clone(), tag_aggregates);
        }
    }

    // Per-language iamfast aggregates (no trials — deterministic).
    let iamfast_refs: Vec<&RunLanguageSummary> = run_summaries
        .iter()
        .flat_map(|r| r.iamfast_language_summaries.iter())
        .collect();
    let mut iamfast_language_aggregates: Vec<LanguageAggregate> = Vec::new();
    for lang in &all_languages {
        if let Some(agg) = build_language_aggregate(lang, &iamfast_refs, true, false) {
            iamfast_language_aggregates.push(agg);
        }
    }

    Ok(AggregateReport {
        timestamp: Utc::now().to_rfc3339(),
        runs_dir: runs_dir.to_string_lossy().into_owned(),
        total_runs,
        successful_runs,
        runs: run_summaries,
        minimal_concrete_actions: minimal_stats,
        managed_concrete_actions: managed_stats,
        managed_vs_minimal_ratio: managed_ratio_stats,
        languages: language_aggregates,
        llm_experiment_aggregates,
        iamfast_languages: iamfast_language_aggregates,
    })
}
