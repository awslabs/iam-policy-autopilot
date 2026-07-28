//! Human-readable summary printing and report writing for the benchmarker.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use tracing::info;

use crate::aggregate::{AggregateReport, Stats};
use crate::report::BenchmarkReport;

/// Write the benchmark report to `<output_dir>/benchmark_managed_policies.json`.
pub fn write_report(output_dir: &Path, report: &BenchmarkReport) -> Result<()> {
    let report_path = output_dir.join("benchmark_managed_policies.json");
    let json = serde_json::to_string_pretty(report)
        .context("Failed to serialise benchmark report")?;
    fs::write(&report_path, json)
        .with_context(|| format!("Failed to write report to {:?}", report_path))?;
    info!("[saved] {:?}", report_path);
    Ok(())
}

/// Print a human-readable summary to stdout.
pub fn print_summary(report: &BenchmarkReport) {
    println!();
    println!("{}", "=".repeat(70));
    println!("  IAC BENCHMARKER RESULTS");
    println!("{}", "=".repeat(70));
    println!("  Run:                    {}", report.run_name);
    println!("  Validation language:    {}", report.validation_language);
    println!("  Region:                 {}", report.region);
    println!("  Account:                {}", report.account);
    println!("  Timestamp:              {}", report.timestamp);
    println!("{}", "-".repeat(70));
    println!("  Autopilot policy:       {}", if report.autopilot_policy_success { "OK" } else { "FAILED" });
    println!("  All minimal actions:    {}", report.all_minimal_actions_count);
    println!("  Coverable actions:      {}", report.coverable_required_actions_count);
    println!("  Uncoverable actions:    {}", report.uncoverable_actions_count);
    println!("  Candidate policies:     {}", report.candidate_policies_count);
    println!("{}", "-".repeat(70));
    println!("  Selected policies ({}):", report.final_selected_arns.len());
    for p in &report.selected_managed_policies {
        println!(
            "    • {} ({}) — {} actions covered ({} unique), {} total concrete",
            p.name,
            p.arn,
            p.actions_covered.len(),
            p.actions_uniquely_covered.len(),
            p.total_concrete_actions
        );
        if !p.actions_uniquely_covered.is_empty() {
            println!("      Uniquely covers: {:?}", p.actions_uniquely_covered);
        }
    }
    println!("{}", "-".repeat(70));
    println!(
        "  Set-cover coverage:     {:.1}%",
        report.set_cover_coverage_pct
    );
    if !report.set_cover_uncovered_actions.is_empty() {
        println!(
            "  Uncovered by set-cover: {:?}",
            report.set_cover_uncovered_actions
        );
    }
    println!("{}", "-".repeat(70));
    println!(
        "  Validation:             {} (attempts: {})",
        if report.validation_success { "PASSED" } else { "FAILED / SKIPPED" },
        report.validation_attempts
    );
    println!(
        "  Minimal policy valid:   {}",
        if report.minimal_policy_validation_success { "PASSED" } else { "FAILED / SKIPPED" }
    );
    println!("{}", "-".repeat(70));
    println!(
        "  Minimal policy concrete actions:  {}",
        report.minimal_concrete_actions
    );
    println!(
        "  Managed policy concrete actions:  {}",
        report.managed_policy_concrete_actions
    );
    println!(
        "  Managed over-permission ratio:    {:.2}x",
        report.over_permission_ratio
    );
    println!("{}", "-".repeat(90));
    println!("  Autopilot overpermissioning per language:");
    println!(
        "    {:<14}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>8}  {:>8}  {}",
        "Language", "Actions", "Minimal", "Managed", "vs Minimal", "vs Managed",
        "AA Errs", "AA Warns", "Status"
    );
    println!("    {}", "-".repeat(90));
    for lop in &report.language_overpermissioning {
        let status = if lop.policy_generated {
            if lop.over_permission_ratio_vs_minimal <= 1.0 {
                "✓ ≤ minimal"
            } else if lop.over_permission_ratio_vs_managed <= 1.0 {
                "~ < managed"
            } else {
                "⚠ > managed"
            }
        } else {
            "✗ no policy"
        };
        println!(
            "    {:<14}  {:>10}  {:>10}  {:>10}  {:>9.2}x  {:>9.2}x  {:>8}  {:>8}  {}",
            lop.language,
            lop.concrete_actions,
            lop.minimal_concrete_actions,
            lop.managed_policy_concrete_actions,
            lop.over_permission_ratio_vs_minimal,
            lop.over_permission_ratio_vs_managed,
            lop.access_analyzer_error_count,
            lop.access_analyzer_warning_count,
            status
        );
    }
    for (tag, experiments) in &report.llm_experiments {
        if experiments.is_empty() {
            continue;
        }
        println!("{}", "-".repeat(90));
        println!("  {} (Bedrock) overpermissioning per language:", tag);
        println!(
            "    {:<14}  {:>8}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>8}  {:>8}  {}",
            "Language", "LLM", "Minimal", "Managed", "vs Minimal", "vs Managed",
            "Validated", "AA Errs", "AA Warns", "Status"
        );
        println!("    {}", "-".repeat(98));
        for llm in experiments {
            let val_success = llm.validation_success.unwrap_or(false);
            let status = if !llm.policy_generated {
                "✗ no policy"
            } else if val_success {
                if llm.over_permission_ratio_vs_minimal <= 1.0 {
                    "✓ ≤ minimal"
                } else if llm.over_permission_ratio_vs_managed <= 1.0 {
                    "~ < managed"
                } else {
                    "⚠ > managed"
                }
            } else {
                "✗ exec fail"
            };
            println!(
                "    {:<14}  {:>8}  {:>10}  {:>10}  {:>9.2}x  {:>9.2}x  {:>10}  {:>8}  {:>8}  {}",
                llm.language,
                llm.concrete_actions,
                llm.minimal_concrete_actions,
                llm.managed_policy_concrete_actions,
                llm.over_permission_ratio_vs_minimal,
                llm.over_permission_ratio_vs_managed,
                if llm.policy_generated { if val_success { "PASS" } else { "FAIL" } } else { "N/A" },
                llm.access_analyzer_error_count,
                llm.access_analyzer_warning_count,
                status
            );
        }
    }
    if !report.iamfast_language_overpermissioning.is_empty() {
        println!("{}", "-".repeat(90));
        println!("  iamfast (static analysis) overpermissioning per language:");
        println!(
            "    {:<14}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>8}  {:>8}  {}",
            "Language", "Actions", "Minimal", "Managed", "vs Minimal", "vs Managed",
            "AA Errs", "AA Warns", "Status"
        );
        println!("    {}", "-".repeat(90));
        for iamfast in &report.iamfast_language_overpermissioning {
            let status = if iamfast.policy_generated {
                if iamfast.over_permission_ratio_vs_minimal <= 1.0 {
                    "✓ ≤ minimal"
                } else if iamfast.over_permission_ratio_vs_managed <= 1.0 {
                    "~ < managed"
                } else {
                    "⚠ > managed"
                }
            } else {
                "✗ no policy"
            };
            println!(
                "    {:<14}  {:>10}  {:>10}  {:>10}  {:>9.2}x  {:>9.2}x  {:>8}  {:>8}  {}",
                iamfast.language,
                iamfast.concrete_actions,
                iamfast.minimal_concrete_actions,
                iamfast.managed_policy_concrete_actions,
                iamfast.over_permission_ratio_vs_minimal,
                iamfast.over_permission_ratio_vs_managed,
                iamfast.access_analyzer_error_count,
                iamfast.access_analyzer_warning_count,
                status
            );
        }
    }
    println!("{}", "=".repeat(90));
    println!();
}

/// Print a human-readable aggregate summary to stdout.
pub fn print_aggregate_summary(report: &AggregateReport) {
    println!();
    println!("{}", "=".repeat(72));
    println!("  IAC BENCHMARKER — AGGREGATE RESULTS");
    println!("{}", "=".repeat(72));
    println!("  Runs dir:        {}", report.runs_dir);
    println!("  Total runs:      {}", report.total_runs);
    println!("  Successful:      {}", report.successful_runs);
    println!("{}", "-".repeat(72));
    println!("  Minimal policy concrete actions:");
    print_stats("    ", &report.minimal_concrete_actions);
    println!("  Managed policy concrete actions (set-cover):");
    print_stats("    ", &report.managed_concrete_actions);
    println!("  Managed / Minimal ratio:");
    print_stats("    ", &report.managed_vs_minimal_ratio);
    println!("{}", "-".repeat(72));
    println!("  Autopilot overpermissioning (ratio vs minimal):");
    println!(
        "    {:<14}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "Language", "Mean", "Median", "Std", "Min", "Q1-Q3", "Max"
    );
    println!("    {}", "-".repeat(66));
    for la in &report.languages {
        let s = &la.ratio_vs_minimal;
        println!(
            "    {:<14}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>3.2}-{:<3.2}  {:>8.2}",
            la.language, s.mean, s.median, s.std_dev, s.min, s.q1, s.q3, s.max
        );
    }
    for (tag, aggregates) in &report.llm_experiment_aggregates {
        if aggregates.is_empty() {
            continue;
        }
        println!("{}", "-".repeat(72));
        println!("  {} overpermissioning (ratio vs minimal):", tag);
        println!(
            "    {:<14}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
            "Language", "Mean", "Median", "Std", "Min", "Q1-Q3", "Max"
        );
        println!("    {}", "-".repeat(66));
        for la in aggregates {
            let s = &la.ratio_vs_minimal;
            println!(
                "    {:<14}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>3.2}-{:<3.2}  {:>8.2}",
                la.language, s.mean, s.median, s.std_dev, s.min, s.q1, s.q3, s.max
            );
        }
    }
    if !report.iamfast_languages.is_empty() {
        println!("{}", "-".repeat(72));
        println!("  iamfast overpermissioning (ratio vs minimal):");
        println!(
            "    {:<14}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
            "Language", "Mean", "Median", "Std", "Min", "Q1-Q3", "Max"
        );
        println!("    {}", "-".repeat(66));
        for la in &report.iamfast_languages {
            let s = &la.ratio_vs_minimal;
            println!(
                "    {:<14}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>3.2}-{:<3.2}  {:>8.2}",
                la.language, s.mean, s.median, s.std_dev, s.min, s.q1, s.q3, s.max
            );
        }
    }
    println!("{}", "=".repeat(72));
    println!();
}

/// Print a single Stats line with a prefix.
pub fn print_stats(prefix: &str, s: &Stats) {
    println!(
        "{}mean={:.2}  median={:.2}  std={:.2}  min={:.2}  max={:.2}",
        prefix, s.mean, s.median, s.std_dev, s.min, s.max
    );
}
