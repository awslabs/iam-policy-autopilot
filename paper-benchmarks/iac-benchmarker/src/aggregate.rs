//! Shared data structures for the aggregate benchmark report.
//!
//! `iac-benchmarker --runs-dir <dir>` writes one `AggregateReport` as
//! `aggregate_report.json`.  The separate `iac-paper-figures` binary reads
//! that file and produces LaTeX artefacts without re-running any AWS calls.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Per-trial result for repeated LLM runs
// ---------------------------------------------------------------------------

/// Result of a single LLM trial (one of N repetitions for a given language
/// within a single benchmark run).
///
/// Stored in [`RunLanguageSummary::llm_trials`] so that downstream consumers
/// (paper-figures, aggregate builder) can compute statistics over all trials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmTrialResult {
    /// 1-based trial index.
    pub trial: usize,
    /// Whether the LLM returned a parseable IAM policy document.
    pub policy_generated: bool,
    /// Concrete IAM actions allowed by the generated policy.
    pub concrete_actions: u32,
    /// concrete_actions / minimal_concrete_actions.
    pub over_permission_ratio_vs_minimal: f64,
    /// concrete_actions / managed_policy_concrete_actions.
    pub over_permission_ratio_vs_managed: f64,
    /// Whether the generated policy allowed the script to run successfully.
    pub validation_success: bool,
    /// Number of Access Analyzer errors.
    pub access_analyzer_error_count: usize,
    /// Number of Access Analyzer warnings.
    pub access_analyzer_warning_count: usize,
    /// Number of Access Analyzer suggestions.
    pub access_analyzer_suggestion_count: usize,
    /// Number of input tokens consumed by the Bedrock Converse call.
    #[serde(default)]
    pub input_tokens: u32,
    /// Number of output tokens produced by the Bedrock Converse call.
    #[serde(default)]
    pub output_tokens: u32,
    /// Total tokens (input + output) for the Bedrock Converse call.
    #[serde(default)]
    pub total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Per-run summary (extracted from BenchmarkReport)
// ---------------------------------------------------------------------------

/// Overpermissioning data for one language within one benchmark run.
///
/// This unified struct is used for all policy-generation approaches:
/// - **Autopilot** (`iam-policy-autopilot`)
/// - **LLM** (AWS Bedrock, simple prompt)
/// - **LLM with context** (AWS Bedrock, context-filled prompt)
/// - **iamfast** (static analysis via `iamfast` CLI)
///
/// Fields that only apply to certain approaches (e.g. `validation_success`
/// for LLM) use `Option` with `#[serde(default)]` for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLanguageSummary {
    /// Programming language identifier (python, go, java, typescript).
    pub language: String,
    /// Whether the tool successfully generated a policy.
    pub policy_generated: bool,
    /// Concrete IAM actions allowed by the generated policy.
    /// For LLM approaches with multiple trials, this is the **median** trial value.
    pub concrete_actions: u32,
    /// concrete_actions / minimal_concrete_actions.
    /// For LLM approaches with multiple trials, this is the **median** trial value.
    pub over_permission_ratio_vs_minimal: f64,
    /// concrete_actions / managed_policy_concrete_actions.
    /// For LLM approaches with multiple trials, this is the **median** trial value.
    pub over_permission_ratio_vs_managed: f64,
    /// Whether the generated policy allowed the script to run successfully.
    /// `None` for approaches that don't perform live validation (e.g. autopilot)
    /// or when `--skip-validation` is set.
    /// For LLM approaches with multiple trials, this reflects the **median** trial.
    #[serde(default)]
    pub validation_success: Option<bool>,
    /// Number of Access Analyzer errors.
    #[serde(default)]
    pub access_analyzer_error_count: usize,
    /// Number of Access Analyzer warnings.
    #[serde(default)]
    pub access_analyzer_warning_count: usize,
    /// Number of Access Analyzer suggestions.
    #[serde(default)]
    pub access_analyzer_suggestion_count: usize,
    /// Per-trial results for LLM-based approaches (simple prompt and context-filled).
    /// Empty for non-LLM approaches (autopilot, iamfast) or when only 1 trial was run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub llm_trials: Vec<LlmTrialResult>,
    /// Mean input tokens per trial across all LLM trials for this language/run.
    #[serde(default)]
    pub total_input_tokens: f64,
    /// Mean output tokens per trial across all LLM trials for this language/run.
    #[serde(default)]
    pub total_output_tokens: f64,
    /// Mean total tokens (input + output) per trial across all LLM trials for this language/run.
    #[serde(default)]
    pub total_tokens: f64,
}

/// All benchmark metrics extracted from one `run_XXX` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Directory name, e.g. `run_001-3478634b`.
    pub run_name: String,
    /// Concrete IAM actions in the minimal (hand-crafted) policy.
    pub minimal_concrete_actions: u32,
    /// Concrete IAM actions allowed by the set-cover managed-policy set.
    pub managed_policy_concrete_actions: u32,
    /// managed_policy_concrete_actions / minimal_concrete_actions.
    pub over_permission_ratio_managed_vs_minimal: f64,
    /// Number of AWS managed policies selected by set-cover.
    pub selected_managed_policy_count: usize,
    /// Set-cover coverage percentage (coverable actions covered).
    pub set_cover_coverage_pct: f64,
    /// Per-language autopilot overpermissioning metrics.
    pub language_summaries: Vec<RunLanguageSummary>,
    /// Per-language LLM overpermissioning metrics, keyed by composite experiment
    /// tag (e.g. "LLM/bare", "CTX-LLM/wildcards", "CTX-LLM/resource-star").
    #[serde(default)]
    pub llm_experiment_summaries: BTreeMap<String, Vec<RunLanguageSummary>>,
    /// Per-language iamfast overpermissioning metrics (static analysis).
    #[serde(default)]
    pub iamfast_language_summaries: Vec<RunLanguageSummary>,
}

// ---------------------------------------------------------------------------
// Aggregate report (written once, read by paper-figures)
// ---------------------------------------------------------------------------

/// Summary statistics computed over a slice of f64 values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub min: f64,
    pub q1: f64,
    pub q3: f64,
    pub max: f64,
    /// All individual values, sorted ascending — needed for box+jitter plots.
    pub values: Vec<f64>,
}

impl Stats {
    /// Compute summary statistics from a non-empty slice.
    /// Returns `None` if the slice is empty.
    pub fn compute(raw: &[f64]) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        let n = raw.len();
        let mut sorted = raw.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let mean = sorted.iter().sum::<f64>() / n as f64;
        let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        let std_dev = variance.sqrt();

        let median = percentile(&sorted, 50.0);
        let q1 = percentile(&sorted, 25.0);
        let q3 = percentile(&sorted, 75.0);

        Some(Stats {
            n,
            mean,
            median,
            std_dev,
            min: sorted[0],
            q1,
            q3,
            max: *sorted.last().unwrap(),
            values: sorted,
        })
    }
}

/// Linear-interpolation percentile on a sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = p / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = rank - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

/// Per-language aggregate statistics across all runs.
///
/// This unified struct is used for all policy-generation approaches:
/// - **Autopilot** (`iam-policy-autopilot`)
/// - **LLM** (AWS Bedrock, simple prompt)
/// - **LLM with context** (AWS Bedrock, context-filled prompt)
/// - **iamfast** (static analysis via `iamfast` CLI)
///
/// Fields that only apply to certain approaches (e.g. `validation_successes`
/// for LLM) use `Option` with `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageAggregate {
    pub language: String,
    /// Stats for `over_permission_ratio_vs_minimal` across all runs where
    /// `policy_generated == true`.
    pub ratio_vs_minimal: Stats,
    /// Stats for `over_permission_ratio_vs_managed` across all runs where
    /// `policy_generated == true`.
    pub ratio_vs_managed: Stats,
    /// Stats for raw `concrete_actions`.
    pub concrete_actions: Stats,
    /// Number of runs where the tool failed to generate a policy.
    pub generation_failures: usize,
    /// Number of runs where the generated policy passed live execution validation.
    /// `None` for approaches that don't perform live validation (e.g. autopilot)
    /// or when `--skip-validation` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_successes: Option<usize>,
    /// Total number of runs attempted (for validation rate calculation).
    /// `None` for approaches that don't perform live validation
    /// or when `--skip-validation` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_attempts: Option<usize>,
    /// Stats for Access Analyzer error counts across runs.
    #[serde(default)]
    pub access_analyzer_errors: Option<Stats>,
    /// Stats for Access Analyzer warning counts across runs.
    #[serde(default)]
    pub access_analyzer_warnings: Option<Stats>,
    /// Stats for mean input tokens per trial across runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<Stats>,
    /// Stats for mean output tokens per trial across runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<Stats>,
    /// Stats for mean total tokens (input + output) per trial across runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<Stats>,
}

/// The top-level aggregate report written by `iac-benchmarker --runs-dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateReport {
    /// ISO-8601 timestamp when this report was generated.
    pub timestamp: String,
    /// Path to the runs directory that was scanned.
    pub runs_dir: String,
    /// Total number of run directories found.
    pub total_runs: usize,
    /// Number of runs that had a `benchmark_managed_policies.json`.
    pub successful_runs: usize,
    /// Individual per-run summaries (one per run directory).
    pub runs: Vec<RunSummary>,
    /// Stats for `minimal_concrete_actions` across all runs.
    pub minimal_concrete_actions: Stats,
    /// Stats for `managed_policy_concrete_actions` across all runs.
    pub managed_concrete_actions: Stats,
    /// Stats for `over_permission_ratio_managed_vs_minimal` across all runs.
    pub managed_vs_minimal_ratio: Stats,
    /// Per-language aggregates for autopilot-generated policies.
    pub languages: Vec<LanguageAggregate>,
    /// Per-language aggregates for LLM experiments, keyed by composite experiment
    /// tag (e.g. "LLM/bare", "CTX-LLM/wildcards", "CTX-LLM/resource-star").
    #[serde(default)]
    pub llm_experiment_aggregates: BTreeMap<String, Vec<LanguageAggregate>>,
    /// Per-language aggregates for iamfast-generated policies (static analysis).
    #[serde(default)]
    pub iamfast_languages: Vec<LanguageAggregate>,
}
