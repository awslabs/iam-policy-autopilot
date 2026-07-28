//! Phase 6: Build the per-run benchmark report.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use anyhow::Result;
use tracing::info;

use crate::printing::{print_summary, write_report};
use crate::report::{BenchmarkReport, LanguageOverpermissioning, SelectedPolicyInfo};
use crate::set_cover::{actions_covered_by_policy_with_resources, SetCoverResult};

use super::types::{ActionCounts, PipelineConfig, SharedContext};

/// Phase 6: Build the per-run report, write it to disk, and print a summary.
pub(crate) fn build_report(
    run_name: &str,
    cfg: &PipelineConfig,
    ctx: &SharedContext,
    output_dir: &Path,
    cover_result: &SetCoverResult,
    coverable_required_actions: &HashSet<String>,
    uncoverable_actions: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
    all_minimal_actions_count: usize,
    candidates_count: usize,
    set_cover_coverage_pct: f64,
    validation_success: bool,
    validation_attempts: usize,
    minimal_policy_validation_success: bool,
    action_counts: &ActionCounts,
    language_overpermissioning: Vec<LanguageOverpermissioning>,
    llm_experiments: BTreeMap<String, Vec<LanguageOverpermissioning>>,
    iamfast_language_overpermissioning: Vec<LanguageOverpermissioning>,
    timestamp: &str,
) -> Result<()> {
    info!("[{}] === Phase 6: Write report ===", run_name);

    let per_policy_covered: Vec<(String, Vec<String>)> = cover_result
        .selected_arns
        .iter()
        .map(|arn| {
            let covered = actions_covered_by_policy_with_resources(
                &ctx.index,
                arn,
                coverable_required_actions,
                required_action_to_resources,
            );
            let mut v: Vec<String> = covered.iter().map(|s| s.to_string()).collect();
            v.sort();
            (arn.clone(), v)
        })
        .collect();

    let selected_managed_policies: Vec<SelectedPolicyInfo> = per_policy_covered
        .iter()
        .map(|(arn, actions_covered)| {
            let name = ctx
                .index
                .policy_arn_to_name
                .get(arn)
                .cloned()
                .unwrap_or_else(|| arn.split('/').last().unwrap_or(arn).to_string());

            let actions_uniquely_covered: Vec<String> = actions_covered
                .iter()
                .filter(|action| {
                    !per_policy_covered
                        .iter()
                        .any(|(other_arn, other_covered)| {
                            other_arn != arn && other_covered.contains(action)
                        })
                })
                .cloned()
                .collect();

            let total_concrete_actions = ctx
                .index
                .policy_arn_to_concrete_action_count
                .get(arn)
                .copied()
                .unwrap_or(0);

            SelectedPolicyInfo {
                arn: arn.clone(),
                name,
                actions_covered: actions_covered.clone(),
                actions_uniquely_covered,
                total_concrete_actions,
            }
        })
        .collect();

    let mut coverable_sorted: Vec<String> = coverable_required_actions.iter().cloned().collect();
    coverable_sorted.sort();

    let mut uncoverable_sorted: Vec<String> = uncoverable_actions.iter().cloned().collect();
    uncoverable_sorted.sort();

    let mut set_cover_uncovered: Vec<String> =
        cover_result.uncovered_actions.iter().cloned().collect();
    set_cover_uncovered.sort();

    let report = BenchmarkReport {
        run_name: run_name.to_string(),
        validation_language: cfg.language.clone(),
        cover_mode: cfg.cover_mode.clone(),
        timestamp: timestamp.to_string(),
        region: ctx.region.clone(),
        account: ctx.account.clone(),
        autopilot_policy_success: true,
        all_minimal_actions_count,
        coverable_required_actions_count: coverable_required_actions.len(),
        coverable_required_actions: coverable_sorted,
        uncoverable_actions_count: uncoverable_actions.len(),
        uncoverable_actions: uncoverable_sorted,
        candidate_policies_count: candidates_count,
        selected_managed_policies,
        set_cover_uncovered_actions: set_cover_uncovered,
        set_cover_coverage_pct,
        validation_success,
        validation_attempts,
        minimal_policy_validation_success,
        final_selected_arns: cover_result.selected_arns.clone(),
        minimal_concrete_actions: action_counts.minimal_concrete_actions,
        managed_policy_concrete_actions: action_counts.managed_policy_concrete_actions,
        over_permission_ratio: action_counts.over_permission_ratio,
        language_overpermissioning,
        llm_experiments,
        iamfast_language_overpermissioning,
        cache_dir: ctx.cache_dir.to_string_lossy().into_owned(),
    };

    write_report(output_dir, &report)?;
    print_summary(&report);

    Ok(())
}
