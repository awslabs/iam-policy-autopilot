//! `iac-coverage-analyzer` — reads benchmark results and computes coverage
//! metrics (recall, precision, F1) by comparing generated policies against
//! the minimal (ground-truth) policy for each run.
//!
//! This binary is designed to run **offline** against existing benchmark
//! results without making any AWS API calls (except for fetching the service
//! catalogue, which is cached locally).
//!
//! Outputs `coverage_report.json` which can be consumed by `iac-paper-figures`
//! to render coverage tables and plots.
//!
//! Usage:
//!   iac-coverage-analyzer \
//!     --aggregate-report <path/to/aggregate_report.json> \
//!     --runs-dir <path/to/runs> \
//!     --results-dir <path/to/results-dir>

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

use iac_benchmarker::aggregate::{AggregateReport, RunSummary, Stats};
use iac_benchmarker::pipeline::count_concrete_actions_from_policies;
use iac_benchmarker::service_ref::{load_or_fetch_catalogue, ServiceCatalogue};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "iac-coverage-analyzer",
    about = "Compute coverage/precision/F1 metrics for benchmark results"
)]
struct Cli {
    /// Path to the aggregate_report.json produced by iac-benchmarker --runs-dir.
    #[arg(long, short = 'a')]
    aggregate_report: PathBuf,

    /// Path to the runs directory containing per-run minimal_policy.json files.
    #[arg(long, short = 'r')]
    runs_dir: PathBuf,

    /// Path to the benchmarker results aggregate directory
    /// (e.g. benchmarker_results/20260419T215653/aggregate).
    #[arg(long, short = 'd')]
    results_dir: PathBuf,

    /// Output path for the coverage report JSON.
    /// [default: <results-dir>/coverage_report.json]
    #[arg(long, short = 'o')]
    output: Option<PathBuf>,

    /// Path to the service catalogue cache file. Shared with iac-benchmarker,
    /// so a prior benchmark run's cache is reused.
    /// [default: ~/.iac-benchmarker/service_reference_cache.json]
    #[arg(long)]
    catalogue_cache: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Output types for coverage_report.json
// ---------------------------------------------------------------------------

/// Coverage metrics for a single policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageMetrics {
    /// Number of actions in the minimal (ground-truth) policy.
    pub minimal_actions: usize,
    /// Number of actions in the generated policy.
    pub generated_actions: usize,
    /// Number of minimal actions present in the generated policy (true positives).
    pub covered_actions: usize,
    /// Number of generated actions not in the minimal policy (false positives / excess).
    pub excess_actions: usize,
    /// Number of minimal actions missing from the generated policy (false negatives).
    pub missing_actions: usize,
    /// Recall: covered / minimal (0.0–1.0). 1.0 = all required actions present.
    pub coverage: f64,
    /// Precision: covered / generated (0.0–1.0). 1.0 = no excess actions.
    pub precision: f64,
    /// F1 score: harmonic mean of precision and recall (0.0–1.0).
    pub f1: f64,
    /// The current overpermissioning ratio (generated / minimal) for reference.
    pub overperm_ratio: f64,
}

/// Coverage data for a single LLM trial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialCoverage {
    pub trial: usize,
    pub policy_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<CoverageMetrics>,
}

/// Coverage data for one language within one approach/experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageCoverage {
    pub language: String,
    /// For approaches with multiple trials (LLM), each trial's coverage.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trials: Vec<TrialCoverage>,
    /// Summary metrics (median trial for LLM, single value for deterministic approaches).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<CoverageMetrics>,
}

/// Coverage data for one approach within one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproachCoverage {
    pub approach: String,
    pub languages: Vec<LanguageCoverage>,
}

/// Coverage data for one benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCoverage {
    pub run_name: String,
    pub minimal_action_count: usize,
    /// The concrete actions from the minimal policy (for reference).
    pub minimal_actions: Vec<String>,
    pub approaches: Vec<ApproachCoverage>,
}

/// Per-language aggregate coverage statistics across all runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageCoverageAggregate {
    pub language: String,
    pub coverage: Stats,
    pub precision: Stats,
    pub f1: Stats,
    pub excess_actions: Stats,
    pub missing_actions: Stats,
}

/// Per-approach aggregate coverage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproachCoverageAggregate {
    pub approach: String,
    pub languages: Vec<LanguageCoverageAggregate>,
}

/// The top-level coverage report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageReport {
    pub timestamp: String,
    pub aggregate_report_path: String,
    pub runs_dir: String,
    pub results_dir: String,
    pub total_runs: usize,
    /// Per-run detailed coverage data.
    pub runs: Vec<RunCoverage>,
    /// Aggregate statistics per approach.
    pub aggregates: Vec<ApproachCoverageAggregate>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Resolve output/cache paths that default relative to well-known locations
    // rather than the current working directory (avoids dropping artifacts in
    // the workspace). The catalogue cache defaults to the same file the
    // iac-benchmarker uses, so a prior run's cache is reused.
    let catalogue_cache = cli.catalogue_cache.clone().unwrap_or_else(|| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".iac-benchmarker")
            .join("service_reference_cache.json")
    });
    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| cli.results_dir.join("coverage_report.json"));

    // 1. Load the aggregate report to get run names and experiment structure.
    let report_text = fs::read_to_string(&cli.aggregate_report)
        .with_context(|| format!("Cannot read {:?}", cli.aggregate_report))?;
    let report: AggregateReport = serde_json::from_str(&report_text)
        .with_context(|| format!("Cannot parse {:?}", cli.aggregate_report))?;

    // 2. Load the service catalogue (cached).
    let catalogue = load_or_fetch_catalogue(&catalogue_cache).await
        .context("Failed to load service catalogue")?;

    println!(
        "[info] Loaded service catalogue with {} services",
        catalogue.len()
    );
    println!(
        "[info] Processing {} runs from {:?}",
        report.runs.len(),
        cli.results_dir
    );

    // 3. Process each run.
    let mut run_coverages: Vec<RunCoverage> = Vec::new();

    for run_summary in &report.runs {
        match process_run(
            run_summary,
            &cli.runs_dir,
            &cli.results_dir,
            &catalogue,
        ) {
            Ok(rc) => run_coverages.push(rc),
            Err(e) => {
                eprintln!(
                    "[warn] Skipping run {}: {}",
                    run_summary.run_name, e
                );
            }
        }
    }

    // 4. Compute aggregates.
    let aggregates = compute_aggregates(&run_coverages);

    // 5. Write the coverage report.
    let coverage_report = CoverageReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        aggregate_report_path: cli.aggregate_report.to_string_lossy().into_owned(),
        runs_dir: cli.runs_dir.to_string_lossy().into_owned(),
        results_dir: cli.results_dir.to_string_lossy().into_owned(),
        total_runs: run_coverages.len(),
        runs: run_coverages,
        aggregates,
    };

    let json = serde_json::to_string_pretty(&coverage_report)
        .context("Failed to serialize coverage report")?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&output, &json)
        .with_context(|| format!("Cannot write {:?}", output))?;

    println!("[saved] {:?}", output);

    Ok(())
}

// ---------------------------------------------------------------------------
// Per-run processing
// ---------------------------------------------------------------------------

/// Process a single benchmark run: load minimal policy, find all generated
/// policies, and compute coverage metrics.
fn process_run(
    run_summary: &RunSummary,
    runs_dir: &Path,
    results_dir: &Path,
    catalogue: &ServiceCatalogue,
) -> Result<RunCoverage> {
    let run_name = &run_summary.run_name;

    // Load minimal policy.
    let minimal_policy_path = runs_dir.join(run_name).join("minimal_policy.json");
    let minimal_doc = load_policy_document(&minimal_policy_path)
        .with_context(|| format!("Cannot load minimal policy for {}", run_name))?;

    let minimal_actions = count_concrete_actions_from_policies(&[minimal_doc], catalogue);
    let minimal_action_count = minimal_actions.len();

    println!(
        "[{}] Minimal policy: {} concrete actions",
        run_name, minimal_action_count
    );

    let run_results_dir = results_dir.join(run_name);
    if !run_results_dir.is_dir() {
        anyhow::bail!("Results directory not found: {:?}", run_results_dir);
    }

    let mut approaches: Vec<ApproachCoverage> = Vec::new();

    // --- Autopilot ---
    let autopilot_langs = analyze_autopilot(
        run_name,
        &run_results_dir,
        &minimal_actions,
        catalogue,
    );
    if !autopilot_langs.is_empty() {
        approaches.push(ApproachCoverage {
            approach: "Autopilot".to_string(),
            languages: autopilot_langs,
        });
    }

    // --- LLM experiments ---
    for tag in run_summary.llm_experiment_summaries.keys() {
        let llm_langs = analyze_llm_experiment(
            run_name,
            tag,
            &run_results_dir,
            &minimal_actions,
            catalogue,
            run_summary,
        );
        if !llm_langs.is_empty() {
            approaches.push(ApproachCoverage {
                approach: tag.clone(),
                languages: llm_langs,
            });
        }
    }

    // --- iamfast ---
    let iamfast_langs = analyze_iamfast(
        run_name,
        &run_results_dir,
        &minimal_actions,
        catalogue,
    );
    if !iamfast_langs.is_empty() {
        approaches.push(ApproachCoverage {
            approach: "iamfast".to_string(),
            languages: iamfast_langs,
        });
    }

    // --- Managed policies ---
    let managed_langs = analyze_managed(
        run_name,
        &run_results_dir,
        &minimal_actions,
    );
    if !managed_langs.is_empty() {
        approaches.push(ApproachCoverage {
            approach: "Managed".to_string(),
            languages: managed_langs,
        });
    }

    let mut sorted_minimal: Vec<String> = minimal_actions.into_iter().collect();
    sorted_minimal.sort();

    Ok(RunCoverage {
        run_name: run_name.clone(),
        minimal_action_count,
        minimal_actions: sorted_minimal,
        approaches,
    })
}

// ---------------------------------------------------------------------------
// Approach-specific analyzers
// ---------------------------------------------------------------------------

const LANGUAGES: [&str; 4] = ["python", "go", "java", "typescript"];

/// Analyze autopilot policies for all languages.
fn analyze_autopilot(
    run_name: &str,
    run_results_dir: &Path,
    minimal_actions: &HashSet<String>,
    catalogue: &ServiceCatalogue,
) -> Vec<LanguageCoverage> {
    let mut results = Vec::new();

    for lang in &LANGUAGES {
        let policy_path = run_results_dir
            .join(format!("{}_autopilot_validation", lang))
            .join("policy.json");

        let metrics = if policy_path.exists() {
            match compute_coverage_for_policy(&policy_path, minimal_actions, catalogue) {
                Ok(m) => {
                    println!(
                        "  [{}][Autopilot][{}] coverage={:.2}, precision={:.2}, F1={:.2}, excess={}, missing={}",
                        run_name, lang, m.coverage, m.precision, m.f1, m.excess_actions, m.missing_actions
                    );
                    Some(m)
                }
                Err(e) => {
                    eprintln!(
                        "  [{}][Autopilot][{}] Error: {}",
                        run_name, lang, e
                    );
                    None
                }
            }
        } else {
            None
        };

        results.push(LanguageCoverage {
            language: lang.to_string(),
            trials: vec![],
            summary: metrics,
        });
    }

    results
}

/// Analyze LLM experiment policies for all languages and trials.
fn analyze_llm_experiment(
    run_name: &str,
    tag: &str,
    run_results_dir: &Path,
    minimal_actions: &HashSet<String>,
    catalogue: &ServiceCatalogue,
    run_summary: &RunSummary,
) -> Vec<LanguageCoverage> {
    let mut results = Vec::new();

    // Parse the tag to get the experiment prefix and strategy.
    // Tags look like "CTX-LLM/wildcards", "LLM/resource-star"
    let (scenario_prefix, strategy) = match tag.split_once('/') {
        Some((p, s)) => (p, s),
        None => return results,
    };

    // Map scenario prefix to directory prefix:
    // "LLM" -> "llm", "CTX-LLM" -> "ctx-llm"
    let dir_prefix = scenario_prefix.to_lowercase();

    for lang in &LANGUAGES {
        // Trial directories follow the pattern: {lang}_{dir_prefix}/{strategy}_trial_{NN}
        let lang_dir = run_results_dir.join(format!("{}_{}", lang, dir_prefix));

        let n_trials = run_summary
            .llm_experiment_summaries
            .get(tag)
            .and_then(|sums| sums.iter().find(|ls| ls.language == *lang))
            .map(|ls| ls.llm_trials.len())
            .unwrap_or(5);

        let mut trials: Vec<TrialCoverage> = Vec::new();

        for trial_idx in 1..=n_trials {
            let trial_dir = lang_dir.join(format!(
                "{}_trial_{:02}",
                strategy, trial_idx
            ));
            let policy_path = trial_dir.join("policy.json");

            let (found, metrics) = if policy_path.exists() {
                match compute_coverage_for_policy(&policy_path, minimal_actions, catalogue) {
                    Ok(m) => (true, Some(m)),
                    Err(_) => (false, None),
                }
            } else {
                (false, None)
            };

            trials.push(TrialCoverage {
                trial: trial_idx,
                policy_found: found,
                metrics,
            });
        }

        // Compute summary: use median trial by coverage (among trials with metrics).
        let summary = compute_median_trial_metrics(&trials);

        if let Some(ref s) = summary {
            println!(
                "  [{}][{}][{}] coverage={:.2}, precision={:.2}, F1={:.2}, excess={}, missing={} (median of {} trials)",
                run_name, tag, lang, s.coverage, s.precision, s.f1,
                s.excess_actions, s.missing_actions,
                trials.iter().filter(|t| t.metrics.is_some()).count()
            );
        }

        results.push(LanguageCoverage {
            language: lang.to_string(),
            trials,
            summary,
        });
    }

    results
}

/// Analyze iamfast policies for supported languages.
fn analyze_iamfast(
    run_name: &str,
    run_results_dir: &Path,
    minimal_actions: &HashSet<String>,
    catalogue: &ServiceCatalogue,
) -> Vec<LanguageCoverage> {
    let mut results = Vec::new();
    let iamfast_langs = ["go", "java"];

    for lang in &iamfast_langs {
        let policy_path = run_results_dir
            .join(format!("{}_iamfast_validation", lang))
            .join("policy.json");

        let metrics = if policy_path.exists() {
            match compute_coverage_for_policy(&policy_path, minimal_actions, catalogue) {
                Ok(m) => {
                    println!(
                        "  [{}][iamfast][{}] coverage={:.2}, precision={:.2}, F1={:.2}, excess={}, missing={}",
                        run_name, lang, m.coverage, m.precision, m.f1, m.excess_actions, m.missing_actions
                    );
                    Some(m)
                }
                Err(e) => {
                    eprintln!(
                        "  [{}][iamfast][{}] Error: {}",
                        run_name, lang, e
                    );
                    None
                }
            }
        } else {
            None
        };

        results.push(LanguageCoverage {
            language: lang.to_string(),
            trials: vec![],
            summary: metrics,
        });
    }

    results
}

/// Deserialization helper for the managed-policy benchmark output.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ManagedPolicyBenchmark {
    managed_policy_concrete_actions: usize,
    minimal_concrete_actions: usize,
    #[serde(default)]
    set_cover_coverage_pct: f64,
}

/// Analyze managed policy coverage for a run.
///
/// Managed policies are language-independent: the benchmarker selects a set of
/// AWS managed policies that together cover all required actions.  By design,
/// coverage is 1.0 (or `set_cover_coverage_pct / 100`).  We compute precision
/// from the ratio of minimal to managed concrete actions.
fn analyze_managed(
    run_name: &str,
    run_results_dir: &Path,
    minimal_actions: &HashSet<String>,
) -> Vec<LanguageCoverage> {
    let bmp_path = run_results_dir.join("benchmark_managed_policies.json");
    if !bmp_path.exists() {
        return vec![];
    }

    let text = match fs::read_to_string(&bmp_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  [{}][Managed] Cannot read {:?}: {}", run_name, bmp_path, e);
            return vec![];
        }
    };

    let bmp: ManagedPolicyBenchmark = match serde_json::from_str(&text) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("  [{}][Managed] Cannot parse {:?}: {}", run_name, bmp_path, e);
            return vec![];
        }
    };

    let minimal_count = minimal_actions.len();
    let generated_count = bmp.managed_policy_concrete_actions;

    // Coverage: by design the set-cover algorithm covers all coverable actions.
    // Use set_cover_coverage_pct if available, otherwise assume 1.0.
    let coverage = if bmp.set_cover_coverage_pct > 0.0 {
        bmp.set_cover_coverage_pct / 100.0
    } else {
        1.0
    };

    let covered_count = (coverage * minimal_count as f64).round() as usize;
    let missing_count = minimal_count.saturating_sub(covered_count);
    let excess_count = generated_count.saturating_sub(covered_count);

    let precision = if generated_count > 0 {
        covered_count as f64 / generated_count as f64
    } else {
        0.0
    };

    let f1 = if precision + coverage > 0.0 {
        2.0 * precision * coverage / (precision + coverage)
    } else {
        0.0
    };

    let overperm_ratio = if minimal_count > 0 {
        generated_count as f64 / minimal_count as f64
    } else {
        0.0
    };

    let metrics = CoverageMetrics {
        minimal_actions: minimal_count,
        generated_actions: generated_count,
        covered_actions: covered_count,
        excess_actions: excess_count,
        missing_actions: missing_count,
        coverage,
        precision,
        f1,
        overperm_ratio,
    };

    println!(
        "  [{}][Managed] coverage={:.2}, precision={:.2}, F1={:.2}, excess={}, missing={} (managed={} actions)",
        run_name, metrics.coverage, metrics.precision, metrics.f1,
        metrics.excess_actions, metrics.missing_actions, generated_count
    );

    // Managed policies are language-independent; use "all" as the pseudo-language.
    vec![LanguageCoverage {
        language: "all".to_string(),
        trials: vec![],
        summary: Some(metrics),
    }]
}

// ---------------------------------------------------------------------------
// Core coverage computation
// ---------------------------------------------------------------------------

/// Load a policy document from a JSON file.
fn load_policy_document(path: &Path) -> Result<serde_json::Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Cannot read {:?}", path))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("Cannot parse {:?}", path))?;

    // Handle both single-document and array-of-documents formats.
    // The autopilot sometimes produces an array of policy documents.
    if doc.is_array() {
        // Return as-is; count_concrete_actions_from_policies handles arrays.
        Ok(doc)
    } else {
        Ok(doc)
    }
}

/// Compute coverage metrics for a generated policy against the minimal policy.
fn compute_coverage_for_policy(
    policy_path: &Path,
    minimal_actions: &HashSet<String>,
    catalogue: &ServiceCatalogue,
) -> Result<CoverageMetrics> {
    let doc = load_policy_document(policy_path)?;

    // Handle both single doc and array of docs.
    let policies: Vec<serde_json::Value> = if doc.is_array() {
        doc.as_array().unwrap().clone()
    } else {
        vec![doc]
    };

    let generated_actions = count_concrete_actions_from_policies(&policies, catalogue);

    compute_coverage_from_sets(minimal_actions, &generated_actions)
}

/// Compute coverage metrics from two sets of concrete actions.
fn compute_coverage_from_sets(
    minimal: &HashSet<String>,
    generated: &HashSet<String>,
) -> Result<CoverageMetrics> {
    let covered: HashSet<&String> = minimal.intersection(generated).collect();
    let excess: HashSet<&String> = generated.difference(minimal).collect();
    let missing: HashSet<&String> = minimal.difference(generated).collect();

    let covered_count = covered.len();
    let excess_count = excess.len();
    let missing_count = missing.len();
    let minimal_count = minimal.len();
    let generated_count = generated.len();

    let coverage = if minimal_count > 0 {
        covered_count as f64 / minimal_count as f64
    } else {
        1.0 // If no minimal actions, everything is covered trivially.
    };

    let precision = if generated_count > 0 {
        covered_count as f64 / generated_count as f64
    } else {
        0.0
    };

    let f1 = if precision + coverage > 0.0 {
        2.0 * precision * coverage / (precision + coverage)
    } else {
        0.0
    };

    let overperm_ratio = if minimal_count > 0 {
        generated_count as f64 / minimal_count as f64
    } else {
        0.0
    };

    Ok(CoverageMetrics {
        minimal_actions: minimal_count,
        generated_actions: generated_count,
        covered_actions: covered_count,
        excess_actions: excess_count,
        missing_actions: missing_count,
        coverage,
        precision,
        f1,
        overperm_ratio,
    })
}

/// Compute the median trial metrics (by coverage) from a list of trial results.
fn compute_median_trial_metrics(trials: &[TrialCoverage]) -> Option<CoverageMetrics> {
    let mut with_metrics: Vec<&CoverageMetrics> = trials
        .iter()
        .filter_map(|t| t.metrics.as_ref())
        .collect();

    if with_metrics.is_empty() {
        return None;
    }

    // Sort by coverage (recall) to find median.
    with_metrics.sort_by(|a, b| {
        a.coverage
            .partial_cmp(&b.coverage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Some(with_metrics[with_metrics.len() / 2].clone())
}

// ---------------------------------------------------------------------------
// Aggregate computation
// ---------------------------------------------------------------------------

/// Compute aggregate coverage statistics across all runs, grouped by approach
/// and language.
fn compute_aggregates(run_coverages: &[RunCoverage]) -> Vec<ApproachCoverageAggregate> {
    // Collect all unique approach names.
    let mut approach_names: Vec<String> = run_coverages
        .iter()
        .flat_map(|rc| rc.approaches.iter().map(|a| a.approach.clone()))
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();
    approach_names.sort();

    // Dynamically collect all unique language names across all approaches and runs.
    // This includes "all" (used by Managed policies) plus the per-language entries.
    let all_languages: Vec<String> = run_coverages
        .iter()
        .flat_map(|rc| {
            rc.approaches
                .iter()
                .flat_map(|a| a.languages.iter().map(|l| l.language.clone()))
        })
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();

    let mut aggregates: Vec<ApproachCoverageAggregate> = Vec::new();

    for approach in &approach_names {
        let mut lang_aggregates: Vec<LanguageCoverageAggregate> = Vec::new();

        for lang in &all_languages {
            // Collect all coverage metrics for this approach+language across runs.
            // For LLM approaches with trials, use the per-run mean of all trials
            // (one data point per run) to give each run equal weight.
            let mut coverage_vals: Vec<f64> = Vec::new();
            let mut precision_vals: Vec<f64> = Vec::new();
            let mut f1_vals: Vec<f64> = Vec::new();
            let mut excess_vals: Vec<f64> = Vec::new();
            let mut missing_vals: Vec<f64> = Vec::new();

            for rc in run_coverages {
                if let Some(ac) = rc.approaches.iter().find(|a| a.approach == *approach) {
                    if let Some(lc) = ac.languages.iter().find(|l| l.language == *lang) {
                        if !lc.trials.is_empty() {
                            // LLM approach: compute per-run mean across all trials with metrics.
                            let trial_metrics: Vec<&CoverageMetrics> = lc
                                .trials
                                .iter()
                                .filter_map(|t| t.metrics.as_ref())
                                .collect();
                            if !trial_metrics.is_empty() {
                                let n = trial_metrics.len() as f64;
                                coverage_vals.push(
                                    trial_metrics.iter().map(|m| m.coverage).sum::<f64>() / n,
                                );
                                precision_vals.push(
                                    trial_metrics.iter().map(|m| m.precision).sum::<f64>() / n,
                                );
                                f1_vals.push(
                                    trial_metrics.iter().map(|m| m.f1).sum::<f64>() / n,
                                );
                                excess_vals.push(
                                    trial_metrics
                                        .iter()
                                        .map(|m| m.excess_actions as f64)
                                        .sum::<f64>()
                                        / n,
                                );
                                missing_vals.push(
                                    trial_metrics
                                        .iter()
                                        .map(|m| m.missing_actions as f64)
                                        .sum::<f64>()
                                        / n,
                                );
                            }
                        } else if let Some(ref summary) = lc.summary {
                            // Deterministic approach: use the single summary value.
                            coverage_vals.push(summary.coverage);
                            precision_vals.push(summary.precision);
                            f1_vals.push(summary.f1);
                            excess_vals.push(summary.excess_actions as f64);
                            missing_vals.push(summary.missing_actions as f64);
                        }
                    }
                }
            }

            if coverage_vals.is_empty() {
                continue;
            }

            if let (Some(cov), Some(prec), Some(f1), Some(excess), Some(missing)) = (
                Stats::compute(&coverage_vals),
                Stats::compute(&precision_vals),
                Stats::compute(&f1_vals),
                Stats::compute(&excess_vals),
                Stats::compute(&missing_vals),
            ) {
                lang_aggregates.push(LanguageCoverageAggregate {
                    language: lang.to_string(),
                    coverage: cov,
                    precision: prec,
                    f1,
                    excess_actions: excess,
                    missing_actions: missing,
                });
            }
        }

        if !lang_aggregates.is_empty() {
            aggregates.push(ApproachCoverageAggregate {
                approach: approach.clone(),
                languages: lang_aggregates,
            });
        }
    }

    aggregates
}
