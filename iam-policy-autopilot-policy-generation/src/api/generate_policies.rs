use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::time::Instant;

use log::{debug, info, trace};

use crate::{
    api::{
        common::process_source_files,
        model::{GeneratePoliciesResult, GeneratePolicyConfig},
    },
    enrichment::{Explanation, Explanations},
    extraction::SdkMethodCall,
    policy_generation::merge::PolicyMergerConfig,
    EnrichmentEngine, PolicyGenerationEngine,
};

/// Check if an action matches a pattern with wildcard support.
/// Patterns can include `*` which matches any sequence of characters.
///
/// # Examples
/// - `"s3:PutObject"` matches `"s3:PutObject"` (exact match)
/// - `"s3:*"` matches `"s3:PutObject"`, `"s3:GetObject"`, etc.
/// - `"ec2:Describe*"` matches `"ec2:DescribeInstances"`, `"ec2:DescribeVolumes"`, etc.
/// - `"*:Get*"` matches `"s3:GetObject"`, `"ec2:GetConsoleOutput"`, etc.
///
/// # Algorithm
/// The algorithm splits the pattern by `*` into literal parts, then verifies all parts
/// appear in the action string in the correct order:
///
/// 1. **First part**: Must be at the start of the action (anchored)
/// 2. **Last part**: Must be at the end of the action (anchored)
/// 3. **Middle parts**: Must appear somewhere in order
///
/// ## Order Preservation
/// After finding each part, `remaining` is advanced past that match. This ensures
/// subsequent parts can only be found in what remains, enforcing left-to-right order.
///
/// Example: Pattern `"*item*get*"` vs Action `"s3:getitemversion"`
/// - parts = `["", "item", "get", ""]`
/// - Find `"item"` at position 6, remaining becomes `"version"`
/// - Find `"get"` in `"version"` → NOT FOUND (even though "get" exists before "item")
/// - Result: NO MATCH ✓
fn action_matches_pattern(action: &str, pattern: &str) -> bool {
    let pattern_lower = pattern.to_lowercase();
    let action_lower = action.to_lowercase();

    // Split pattern by '*' to get literal parts that must appear in order
    // e.g., "ec2:Describe*" → ["ec2:describe", ""]
    // e.g., "*:Get*" → ["", ":get", ""]
    let parts: Vec<&str> = pattern_lower.split('*').collect();

    if parts.len() == 1 {
        // No wildcard, exact match required
        return action_lower == pattern_lower;
    }

    // Track the remaining portion of the action string to search
    // After finding each part, we advance past it to enforce order
    let mut remaining = action_lower.as_str();

    for (i, part) in parts.iter().enumerate() {
        // Skip empty parts (from consecutive wildcards or leading/trailing wildcards)
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            // First non-empty part must be at the start (no wildcard before it)
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            // Last non-empty part must be at the end (no wildcard after it)
            if !remaining.ends_with(part) {
                return false;
            }
        } else {
            // Middle parts: find first occurrence and advance past it
            // This enforces order - subsequent parts can only match what remains
            match remaining.find(part) {
                Some(pos) => {
                    remaining = &remaining[pos + part.len()..];
                }
                None => return false,
            }
        }
    }

    true
}

/// Filter explanations to only include actions matching the given patterns.
fn filter_explanations(
    explanations: Option<Explanations>,
    filters: &[String],
) -> Option<Explanations> {
    let explanations = explanations?;

    let filtered_map: BTreeMap<String, Explanation> = explanations
        .explanation_for_action
        .into_iter()
        .filter(|(action, _)| {
            filters
                .iter()
                .any(|pattern| action_matches_pattern(action, pattern))
        })
        .collect();

    if filtered_map.is_empty() {
        None
    } else {
        Some(Explanations::new(filtered_map))
    }
}

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

    // Generate explanations only if explain_filters is provided
    let explanations = match &config.explain_filters {
        Some(filters) => filter_explanations(result.explanations, filters),
        None => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_matches_pattern_exact_match() {
        assert!(action_matches_pattern("s3:PutObject", "s3:PutObject"));
        assert!(action_matches_pattern("s3:PutObject", "S3:putobject")); // case insensitive
        assert!(!action_matches_pattern("s3:PutObject", "s3:GetObject"));
    }

    #[test]
    fn test_action_matches_pattern_wildcard_suffix() {
        assert!(action_matches_pattern("s3:PutObject", "s3:*"));
        assert!(action_matches_pattern("s3:GetObject", "s3:*"));
        assert!(action_matches_pattern("s3:DeleteBucket", "s3:*"));
        assert!(!action_matches_pattern("ec2:DescribeInstances", "s3:*"));
    }

    #[test]
    fn test_action_matches_pattern_wildcard_prefix() {
        assert!(action_matches_pattern("s3:GetObject", "*:GetObject"));
        assert!(action_matches_pattern("ec2:GetObject", "*:GetObject"));
        assert!(!action_matches_pattern("s3:PutObject", "*:GetObject"));
    }

    #[test]
    fn test_action_matches_pattern_wildcard_middle() {
        assert!(action_matches_pattern(
            "ec2:DescribeInstances",
            "ec2:Describe*"
        ));
        assert!(action_matches_pattern(
            "ec2:DescribeVolumes",
            "ec2:Describe*"
        ));
        assert!(action_matches_pattern(
            "ec2:DescribeSecurityGroups",
            "ec2:Describe*"
        ));
        assert!(!action_matches_pattern("ec2:RunInstances", "ec2:Describe*"));
    }

    #[test]
    fn test_action_matches_pattern_multiple_wildcards() {
        assert!(action_matches_pattern("s3:GetObject", "*:Get*"));
        assert!(action_matches_pattern("ec2:GetConsoleOutput", "*:Get*"));
        assert!(action_matches_pattern("dynamodb:GetItem", "*:Get*"));
        assert!(!action_matches_pattern("s3:PutObject", "*:Get*"));
    }

    #[test]
    fn test_action_matches_pattern_full_wildcard() {
        assert!(action_matches_pattern("s3:PutObject", "*"));
        assert!(action_matches_pattern("ec2:DescribeInstances", "*"));
        assert!(action_matches_pattern("anything:here", "*"));
    }

    #[test]
    fn test_filter_explanations_full_wildcard() {
        // When filter is "*", should return all explanations
        let mut map = BTreeMap::new();
        map.insert("s3:PutObject".to_string(), Explanation::default());
        map.insert("ec2:DescribeInstances".to_string(), Explanation::default());
        let explanations = Some(Explanations::new(map));

        let result = filter_explanations(explanations, &["*".to_string()]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().explanation_for_action.len(), 2);
    }

    #[test]
    fn test_filter_explanations_with_filters() {
        let mut map = BTreeMap::new();
        map.insert("s3:PutObject".to_string(), Explanation::default());
        map.insert("s3:GetObject".to_string(), Explanation::default());
        map.insert("ec2:DescribeInstances".to_string(), Explanation::default());
        map.insert("dynamodb:GetItem".to_string(), Explanation::default());
        let explanations = Some(Explanations::new(map));

        // Filter to only s3 actions
        let result = filter_explanations(explanations, &["s3:*".to_string()]);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.explanation_for_action.len(), 2);
        assert!(result.explanation_for_action.contains_key("s3:PutObject"));
        assert!(result.explanation_for_action.contains_key("s3:GetObject"));
    }

    #[test]
    fn test_filter_explanations_multiple_patterns() {
        let mut map = BTreeMap::new();
        map.insert("s3:PutObject".to_string(), Explanation::default());
        map.insert("ec2:DescribeInstances".to_string(), Explanation::default());
        map.insert("dynamodb:GetItem".to_string(), Explanation::default());
        let explanations = Some(Explanations::new(map));

        // Filter to s3 and dynamodb actions
        let result = filter_explanations(
            explanations,
            &["s3:*".to_string(), "dynamodb:*".to_string()],
        );
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.explanation_for_action.len(), 2);
        assert!(result.explanation_for_action.contains_key("s3:PutObject"));
        assert!(result
            .explanation_for_action
            .contains_key("dynamodb:GetItem"));
    }

    #[test]
    fn test_filter_explanations_no_matches() {
        let mut map = BTreeMap::new();
        map.insert("s3:PutObject".to_string(), Explanation::default());
        let explanations = Some(Explanations::new(map));

        // Filter to ec2 actions (no matches)
        let result = filter_explanations(explanations, &["ec2:*".to_string()]);
        assert!(result.is_none());
    }

    #[test]
    fn test_filter_explanations_none_input() {
        let result = filter_explanations(None, &["s3:*".to_string()]);
        assert!(result.is_none());
    }
}
