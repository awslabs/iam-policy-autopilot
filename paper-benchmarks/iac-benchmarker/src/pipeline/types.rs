//! Shared types used across the benchmark pipeline.
//!
//! Contains the [`SharedContext`] (AWS clients + caches), [`PipelineConfig`]
//! (CLI-derived settings), and intermediate result structs passed between phases.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use aws_sdk_accessanalyzer::Client as AaClient;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::llm_policy::ResourcePromptStrategy;
use crate::policy_index::PolicyIndex;
use crate::service_ref::ServiceCatalogue;

// ---------------------------------------------------------------------------
// Shared bootstrap context (built once, reused across all runs in batch mode)
// ---------------------------------------------------------------------------

/// Shared AWS clients, caches, and configuration built once during bootstrap
/// and reused across all runs in batch mode.
pub struct SharedContext {
    pub iam: IamClient,
    pub sts: StsClient,
    pub bedrock: BedrockClient,
    pub aa: AaClient,
    pub account: String,
    pub region: String,
    pub bedrock_model_id: String,
    pub catalogue: ServiceCatalogue,
    pub index: PolicyIndex,
    pub cache_dir: PathBuf,
    /// Resolved path to the scenarios directory (e.g. `iac-benchmarker/scenarios`).
    /// `None` if context-filling is disabled.
    pub context_scenarios_dir: Option<PathBuf>,
    /// Cancellation token for graceful shutdown.  When cancelled (e.g. via
    /// SIGINT), the pipeline skips remaining validation phases but still
    /// runs CDK destroy to clean up AWS resources.
    pub shutdown: CancellationToken,
}

// ---------------------------------------------------------------------------
// CLI config subset needed by the pipeline
// ---------------------------------------------------------------------------

/// Configuration values extracted from the CLI that the pipeline needs.
/// Avoids coupling the pipeline module to the `clap`-derived `Cli` struct.
pub struct PipelineConfig {
    pub language: String,
    pub languages: Vec<String>,
    /// Languages to run iamfast static analysis on (subset of `languages`).
    /// Defaults to `["go", "java"]` because iamfast only supports Go and Java.
    pub iamfast_languages: Vec<String>,
    pub autopilot_binary: String,
    pub skip_validation: bool,
    pub no_cleanup_roles: bool,
    pub cover_mode: String,
    pub cdk_role_arn: Option<String>,
    pub skip_deploy: bool,
    pub skip_destroy: bool,
    pub skip_llm: bool,
    pub skip_iamfast: bool,
    pub iamfast_binary: String,
    /// Number of times to repeat LLM policy generation for each language.
    /// Each repetition generates a fresh policy and optionally validates it.
    /// Results from all repetitions are recorded; the median is used as the
    /// representative value.  Defaults to 10.
    pub llm_repetitions: usize,
    /// Resource prompt strategies to benchmark.
    /// Each strategy is crossed with each context scenario (script-only,
    /// script+context, script+CDK+context) to form the 3×3 matrix.
    pub resource_prompt_strategies: Vec<ResourcePromptStrategy>,
}

// ---------------------------------------------------------------------------
// Intermediate results passed between phases
// ---------------------------------------------------------------------------

/// Output of Phase 1: loading and expanding the minimal policy.
pub(crate) struct MinimalPolicyData {
    pub minimal_policy_docs: Vec<Value>,
    pub minimal_allow_patterns: Vec<String>,
    pub required_action_to_resources: HashMap<String, Vec<String>>,
    pub all_minimal_actions: HashSet<String>,
}

/// Output of Phase 2: pre-filtering candidates + coverable/uncoverable split.
pub(crate) struct CandidateData {
    pub candidates: Vec<String>,
    pub coverable_required_actions: HashSet<String>,
    pub uncoverable_actions: HashSet<String>,
}

/// Output of Phase 5: concrete action counts for managed and minimal policies.
pub(crate) struct ActionCounts {
    pub minimal_concrete_actions: u32,
    pub managed_policy_concrete_actions: u32,
    pub over_permission_ratio: f64,
}
