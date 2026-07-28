//! Report types used by the benchmarker for per-run output.
//!
//! These structs are serialised to `benchmark_managed_policies.json` and are
//! separate from the aggregate types in [`crate::aggregate`] which are used
//! for cross-run statistics.

use std::collections::BTreeMap;

use serde::Serialize;

/// Information about a single managed policy selected by the set-cover algorithm.
#[derive(Debug, Serialize)]
pub struct SelectedPolicyInfo {
    pub arn: String,
    pub name: String,
    pub actions_covered: Vec<String>,
    pub actions_uniquely_covered: Vec<String>,
    pub total_concrete_actions: u32,
}

/// Overpermissioning metrics for a single language's generated policy compared
/// to both the minimal policy (Java-derived) and the minimum managed-policy set.
///
/// This unified struct is used for all policy-generation approaches:
/// - **Autopilot** (`iam-policy-autopilot`)
/// - **LLM** (AWS Bedrock, simple prompt)
/// - **LLM with context** (AWS Bedrock, context-filled prompt)
/// - **iamfast** (static analysis via `iamfast` CLI)
///
/// The `concrete_actions` field holds the action count regardless of the
/// generation approach.  Fields that only apply to certain approaches (e.g.
/// `validation_success` for LLM) use `Option` or sensible defaults.
#[derive(Debug, Clone, Serialize)]
pub struct LanguageOverpermissioning {
    /// Programming language (python, go, java, typescript).
    pub language: String,
    /// Whether the tool successfully generated a policy for this language.
    pub policy_generated: bool,
    /// Number of concrete IAM actions allowed by the generated policy.
    pub concrete_actions: u32,
    /// Number of concrete IAM actions allowed by the minimal policy (same for all languages).
    pub minimal_concrete_actions: u32,
    /// Number of concrete IAM actions allowed by the minimum managed-policy set (same for all languages).
    pub managed_policy_concrete_actions: u32,
    /// concrete_actions / minimal_concrete_actions.
    /// Values > 1 indicate overpermissioning relative to the minimal policy.
    pub over_permission_ratio_vs_minimal: f64,
    /// concrete_actions / managed_policy_concrete_actions.
    /// Values < 1 mean the generated policy is tighter than the managed-policy set.
    pub over_permission_ratio_vs_managed: f64,
    /// Whether the generated policy allowed the script to run successfully.
    /// `None` for approaches that don't perform live validation (e.g. autopilot)
    /// or when `--skip-validation` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_success: Option<bool>,
    /// Number of Access Analyzer errors for the generated policy.
    pub access_analyzer_error_count: usize,
    /// Number of Access Analyzer warnings for the generated policy.
    pub access_analyzer_warning_count: usize,
    /// Number of Access Analyzer suggestions for the generated policy.
    pub access_analyzer_suggestion_count: usize,
}

/// The full benchmark report written for a single run.
#[derive(Debug, Serialize)]
pub struct BenchmarkReport {
    pub run_name: String,
    pub validation_language: String,
    pub cover_mode: String,
    pub timestamp: String,
    pub region: String,
    pub account: String,
    pub autopilot_policy_success: bool,
    pub all_minimal_actions_count: usize,
    pub coverable_required_actions: Vec<String>,
    pub coverable_required_actions_count: usize,
    pub uncoverable_actions: Vec<String>,
    pub uncoverable_actions_count: usize,
    pub candidate_policies_count: usize,
    pub selected_managed_policies: Vec<SelectedPolicyInfo>,
    pub set_cover_uncovered_actions: Vec<String>,
    pub set_cover_coverage_pct: f64,
    pub validation_success: bool,
    pub validation_attempts: usize,
    /// Whether the minimal_policy.json itself allowed the script to run successfully.
    pub minimal_policy_validation_success: bool,
    pub final_selected_arns: Vec<String>,
    /// Concrete actions allowed by the minimal policy (Java-derived baseline).
    pub minimal_concrete_actions: u32,
    /// Concrete actions allowed by the minimum set of managed policies found by set-cover.
    pub managed_policy_concrete_actions: u32,
    /// managed_policy_concrete_actions / minimal_concrete_actions.
    pub over_permission_ratio: f64,
    /// Per-language overpermissioning of iam-policy-autopilot generated policies
    /// compared to the minimal policy.
    pub language_overpermissioning: Vec<LanguageOverpermissioning>,
    /// Per-language overpermissioning of LLM-generated policies, keyed by
    /// composite experiment tag (e.g. "LLM/bare", "CTX-LLM/wildcards",
    /// "CTX-LLM/resource-star").
    pub llm_experiments: BTreeMap<String, Vec<LanguageOverpermissioning>>,
    /// Per-language overpermissioning of iamfast-generated policies (static analysis).
    pub iamfast_language_overpermissioning: Vec<LanguageOverpermissioning>,
    pub cache_dir: String,
}
