//! In-memory policy generation API — no filesystem access required.
//!
//! This entry point accepts source code as strings rather than file paths,
//! making it suitable for WASM and other environments without filesystem access.
//!
//! Requires the `tree-sitter` feature for source code extraction.

use anyhow::{Context, Result};
use log::debug;

use crate::{
    api::generate_policies::{load_action_catalogs, validate_size_configuration},
    api::model::{
        compact_policy_size, AwsContext, GeneratePoliciesResult, GeneratePoliciesResultBuilder,
        PolicyWarning,
    },
    policy_generation::merge::PolicyMergerConfig,
    EnrichmentEngine, ExtractionEngine, Language, PolicyGenerationEngine, SourceFile,
};

/// Configuration for in-memory policy generation.
#[derive(Debug, Clone)]
pub struct GenerateFromSourceConfig {
    /// Pre-built in-memory source files (path is used only for location metadata).
    pub source_files: Vec<SourceFile>,
    /// The language of all source files (must be uniform).
    pub language: Language,
    /// AWS context for ARN generation.
    pub aws_context: AwsContext,
    /// Enable minimal policy size by allowing cross-service merging.
    pub minimize_policy_size: bool,
    /// Maximum generated policy size in non-whitespace JSON characters.
    ///
    /// Statements that do not fit start a new policy. Oversized
    /// single-statement policies are still emitted with a warning; generation
    /// continues.
    pub max_policy_size: usize,
    /// Best-effort total output size target that may be met by introducing
    /// overpermissive action wildcards.
    pub allow_overpermissive_wildcards_to_limit_total_output_size: Option<usize>,
}

/// Generate IAM policies from in-memory source code.
///
/// This is the WASM-friendly counterpart of [`super::generate_policies`].
/// It skips filesystem I/O entirely — source content is provided directly
/// and the Service Reference filesystem cache is disabled.
pub async fn generate_policies_from_source(
    config: &GenerateFromSourceConfig,
) -> Result<GeneratePoliciesResult> {
    validate_size_configuration(
        config.max_policy_size,
        config.allow_overpermissive_wildcards_to_limit_total_output_size,
        false,
    )?;
    if config.source_files.is_empty() {
        return Ok(GeneratePoliciesResult::empty());
    }

    // 1. Extract SDK calls
    let extractor = ExtractionEngine::new();
    let extracted = extractor
        .extract_sdk_method_calls(config.language, config.source_files.clone())
        .await
        .context("Failed to extract SDK method calls")?;

    if extracted.methods.is_empty() {
        return Ok(GeneratePoliciesResult::empty());
    }

    let sdk = config.language.sdk_type();

    debug!(
        "Extracted {} methods, starting enrichment",
        extracted.methods.len()
    );

    // 2. Enrich (filesystem cache disabled — no FS in WASM)
    let mut enrichment_engine = EnrichmentEngine::new(true, crate::DEFAULT_RESOURCE_CUTOFF)
        .context("Failed to create enrichment engine")?;

    let enriched = enrichment_engine
        .enrich_methods(&extracted.methods, sdk)
        .await
        .context("Failed to enrich methods")?;

    // 3. Generate policies
    let merger_config = PolicyMergerConfig {
        allow_cross_service_merging: config.minimize_policy_size,
        max_policy_size: config.max_policy_size,
    };
    let policy_engine = PolicyGenerationEngine::with_merger_config(
        &config.aws_context.partition,
        &config.aws_context.region,
        &config.aws_context.account,
        merger_config,
    );

    let result = policy_engine
        .generate_policies(&enriched)
        .context("Failed to generate IAM policies")?;

    let mut merged = policy_engine
        .merge_policies(&result.policies)
        .context("Failed to merge IAM policies")?;
    if let Some(target_size) = config.allow_overpermissive_wildcards_to_limit_total_output_size {
        let exact_size: usize = merged
            .iter()
            .map(|policy| compact_policy_size(&policy.policy))
            .sum();
        if exact_size > target_size {
            let catalogs = load_action_catalogs(&enrichment_engine, &merged).await;
            merged = policy_engine
                .optimize_total_output_size(&merged, &catalogs, target_size)
                .context("Failed to optimize total policy output size")?;
        }
    }
    let warnings = PolicyWarning::warnings_for_generation(
        &merged,
        config.max_policy_size,
        config.allow_overpermissive_wildcards_to_limit_total_output_size,
    );

    // Explanations are omitted in the WASM/from-source path: the browser consumer only
    // needs the final policies, and including per-action explanations would significantly
    // increase the JSON payload size over the wire. Explanations remain available in the
    // CLI/MCP paths via generate_policies().
    Ok(GeneratePoliciesResultBuilder::default()
        .policies(merged)
        .warnings(warnings)
        .build()
        .expect("GeneratePoliciesResultBuilder missing required policies"))
}
