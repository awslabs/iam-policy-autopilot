use anyhow::{Context, Result};
use std::time::Instant;

use log::{debug, info, trace};

use crate::{
    api::{common::process_source_files, model::GeneratePolicyConfig},
    extraction::SdkMethodCall,
    policy_generation::{merge::PolicyMergerConfig, MethodActionMapping, PolicyWithMetadata},
    EnrichmentEngine, PolicyGenerationEngine,
};

/// Generate polcies for source files
pub async fn generate_policies(
    config: &GeneratePolicyConfig,
) -> Result<(Vec<PolicyWithMetadata>, Vec<MethodActionMapping>)> {
    let pipeline_start = Instant::now();

    debug!(
        "Using AWS context: partition={:?}, region={:?}, account={:?}",
        config.aws_context.partition, config.aws_context.region, config.aws_context.account
    );

    // Create the extractor
    let extractor = crate::ExtractionEngine::new();

    // Process source files to get extracted methods
    let extracted_methods = process_source_files(
        &extractor,
        &config.extract_sdk_calls_config.source_files,
        config.extract_sdk_calls_config.language.as_deref(),
    )
    .await
    .context("Failed to process source files")?;

    let extracted_methods = extracted_methods
        .methods
        .into_iter()
        .collect::<Vec<SdkMethodCall>>();

    debug!(
        "Extracted {} methods, starting enrichment pipeline",
        extracted_methods.len()
    );

    // Handle empty method lists gracefully
    if extracted_methods.is_empty() {
        info!("No methods found to process, returning empty policy list");
        return Ok((vec![], vec![]));
    }

    let mut enrichment_engine = EnrichmentEngine::new(config.disable_file_system_cache)?;

    // Run the complete enrichment pipeline
    let enriched_results = enrichment_engine.enrich_methods(&extracted_methods).await?;

    let enrichment_duration = pipeline_start.elapsed();
    trace!("Enrichment pipeline completed in {:?}", enrichment_duration);

    // Create policy generation engine with AWS context and merger configuration
    let merger_config = PolicyMergerConfig {
        allow_cross_service_merging: config.minimize_policy_size,
    };

    let policy_engine = PolicyGenerationEngine::with_merger_config(
        &config.aws_context.partition,
        &config.aws_context.region,
        &config.aws_context.account,
        merger_config,
    );

    // Generate IAM policies from enriched method calls
    debug!(
        "Generating IAM policies from {} enriched method calls",
        enriched_results.len()
    );
    let policies = policy_engine
        .generate_policies(&enriched_results)
        .context("Failed to generate IAM policies")?;

    let total_duration = pipeline_start.elapsed();
    debug!(
        "Policy generation completed in {:?}, generated {} policies",
        total_duration,
        policies.len()
    );

    let mut final_policies = policies;

    if !config.individual_policies {
        final_policies = policy_engine
            .merge_policies(&final_policies)
            .context("Failed to merge IAM policies")?;
    }

    // Handle policy output based on configuration
    if config.generate_action_mappings {
        // Extract method to action mappings using the core method
        debug!(
            "Extracting method to action mappings from {} enriched method calls",
            enriched_results.len()
        );
        let method_action_mappings = policy_engine
            .extract_action_mappings(&enriched_results)
            .context("Failed to extract method to action mappings")?;

        debug!(
            "Extracted {} method to action mappings",
            method_action_mappings.len()
        );

        return Ok((final_policies, method_action_mappings));
    }

    Ok((final_policies, vec![]))
}
