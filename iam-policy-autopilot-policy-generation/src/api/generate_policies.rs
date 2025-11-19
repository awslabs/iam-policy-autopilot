use anyhow::{Context, Result};
use std::time::Instant;

use log::{debug, info, trace};

use crate::{
    api::{
        common::process_source_files,
        model::{GeneratePoliciesResult, GeneratePolicyConfig},
    },
    extraction::SdkMethodCall,
    policy_generation::merge::PolicyMergerConfig,
    EnrichmentEngine, PolicyGenerationEngine,
};

/// Generate policies for source files
pub async fn generate_policies(config: &GeneratePolicyConfig) -> Result<GeneratePoliciesResult> {
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
        config.extract_sdk_calls_config.service_hints.clone(),
    )
    .await
    .context("Failed to process source files")?;

    // Relies on the invariant that all source files must be of the same language, which we
    // enforce in process_source_files
    let sdk = extracted_methods
        .metadata
        .source_files
        .first()
        .map_or(crate::SdkType::Other, |f| f.language.sdk_type());

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
        return Ok(GeneratePoliciesResult {
            policies: vec![],
            explanations: None,
        });
    }

    let mut enrichment_engine = EnrichmentEngine::new(config.disable_file_system_cache)?;

    // Run the complete enrichment pipeline
    let enriched_results = enrichment_engine
        .enrich_methods(&extracted_methods, sdk)
        .await?;

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
    let result = policy_engine
        .generate_policies(&enriched_results)
        .context("Failed to generate IAM policies")?;

    let total_duration = pipeline_start.elapsed();
    debug!(
        "Policy generation completed in {:?}, generated {} policies",
        total_duration,
        result.policies.len()
    );

    let mut final_policies = result.policies;
    let explanations = if config.generate_explanations {
        result.explanations
    } else {
        None
    };

    if !config.individual_policies {
        final_policies = policy_engine
            .merge_policies(&final_policies)
            .context("Failed to merge IAM policies")?;
    }

    Ok(GeneratePoliciesResult {
        policies: final_policies,
        explanations,
    })
}
