//! Policy generation from pre-extracted SDK method calls.
//!
//! This entry point accepts already-extracted `SdkMethodCall` data (as JSON),
//! skipping the tree-sitter/ast-grep extraction step entirely. This makes it
//! suitable for WASM builds where extraction is performed in JavaScript using
//! web-tree-sitter, and only the validation/enrichment/generation pipeline
//! runs in Rust.

use anyhow::{Context, Result};
use log::debug;

use crate::{
    api::model::{AwsContext, GeneratePoliciesResult},
    extraction::{sdk_model::ServiceDiscovery, SdkMethodCall},
    policy_generation::merge::PolicyMergerConfig,
    EnrichmentEngine, Language, PolicyGenerationEngine,
};

/// Configuration for policy generation from pre-extracted SDK calls.
#[derive(Debug, Clone)]
pub struct GenerateFromSdkCallsConfig {
    /// Pre-extracted SDK method calls.
    pub sdk_calls: Vec<SdkMethodCall>,
    /// The language of the source code (determines SDK type for enrichment).
    pub language: Language,
    /// AWS context for ARN generation.
    pub aws_context: AwsContext,
    /// Enable minimal policy size by allowing cross-service merging.
    pub minimize_policy_size: bool,
}

/// Generate IAM policies from pre-extracted SDK method calls.
///
/// This skips the extraction step entirely — callers provide `Vec<SdkMethodCall>`
/// directly (e.g. from a JavaScript web-tree-sitter extractor serialized as JSON).
/// The pipeline runs: SDK model validation → enrichment → policy generation.
pub async fn generate_policies_from_sdk_calls(
    config: &GenerateFromSdkCallsConfig,
) -> Result<GeneratePoliciesResult> {
    if config.sdk_calls.is_empty() {
        return Ok(GeneratePoliciesResult::new(vec![], None));
    }

    let sdk = config.language.sdk_type();

    debug!(
        "Generating policies from {} pre-extracted SDK calls",
        config.sdk_calls.len()
    );

    // Resolve PossibleServices when the JS extractor leaves them empty.
    // Load the SDK service index and look up each method name to find
    // which AWS services it belongs to (mirroring the filter_map step
    // in the full extraction pipeline).
    let resolved_calls = resolve_services(&config.sdk_calls, config.language).await?;

    if resolved_calls.is_empty() {
        return Ok(GeneratePoliciesResult::new(vec![], None));
    }

    // Enrich (filesystem cache disabled — no FS in WASM)
    let mut enrichment_engine =
        EnrichmentEngine::new(true).context("Failed to create enrichment engine")?;

    let enriched = enrichment_engine
        .enrich_methods(&resolved_calls, sdk)
        .await
        .context("Failed to enrich methods")?;

    // Generate policies
    let merger_config = PolicyMergerConfig {
        allow_cross_service_merging: config.minimize_policy_size,
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

    let merged = policy_engine
        .merge_policies(&result.policies)
        .context("Failed to merge IAM policies")?;

    Ok(GeneratePoliciesResult::new(merged, None))
}

/// Resolve `possible_services` for SDK calls that have empty service lists.
///
/// When the JS extractor provides method names without service information,
/// this function loads the AWS SDK service index and looks up each method
/// name to determine which services it could belong to. Calls that don't
/// match any known SDK method are filtered out.
async fn resolve_services(
    sdk_calls: &[SdkMethodCall],
    language: Language,
) -> Result<Vec<SdkMethodCall>> {
    // Check if any calls need resolution
    let needs_resolution = sdk_calls.iter().any(|c| c.possible_services.is_empty());
    if !needs_resolution {
        return Ok(sdk_calls.to_vec());
    }

    debug!("Resolving services for SDK calls with empty PossibleServices");

    let service_index = ServiceDiscovery::load_service_index(language)
        .await
        .context("Failed to load SDK service index for service resolution")?;

    let mut resolved = Vec::with_capacity(sdk_calls.len());

    for call in sdk_calls {
        if !call.possible_services.is_empty() {
            resolved.push(call.clone());
            continue;
        }

        // Look up the method name in the service index
        if let Some(service_refs) = service_index.method_lookup.get(&call.name) {
            let services: Vec<String> = service_refs
                .iter()
                .map(|r| r.service_name.clone())
                .collect();

            debug!(
                "Resolved method '{}' to services: {:?}",
                call.name, services
            );

            resolved.push(SdkMethodCall {
                name: call.name.clone(),
                possible_services: services,
                metadata: call.metadata.clone(),
            });
        } else {
            debug!(
                "Method '{}' not found in SDK service index — skipping",
                call.name
            );
        }
    }

    Ok(resolved)
}
