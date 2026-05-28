use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use log::{debug, info, trace};

use crate::{
    api::{
        common::process_source_files,
        model::{GeneratePoliciesResult, GeneratePolicyConfig},
    },
    enrichment::{
        terraform::{resource_binder::TerraformResourceResolver, ResourceBindingExplanation},
        Explanation, Explanations,
    },
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

/// Check if an ARN matches a glob pattern (supports `*` wildcards).
fn arn_matches_pattern(arn: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Split pattern by `*` and check if ARN contains all parts in order
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // First part must be a prefix
            if !arn[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if let Some(found) = arn[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    // If pattern ends with *, any suffix is ok; otherwise must match to end
    pattern.ends_with('*') || pos == arn.len()
}

/// Filter resource binding explanations by ARN patterns.
///
/// Returns `None` when `filters` is `None` (no `--explain-resources` provided).
/// When filters are present, only explanations with ARNs matching at least one
/// pattern are retained.
fn filter_resource_explanations(
    explanations: Vec<ResourceBindingExplanation>,
    filters: &[String],
) -> Vec<ResourceBindingExplanation> {
    explanations
        .into_iter()
        .filter(|e| {
            filters
                .iter()
                .any(|pattern| arn_matches_pattern(&e.arn, pattern))
        })
        .collect()
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

/// Generate policies for source files, with optional Terraform resource binding.
///
/// When `config.terraform_dir` is set, the pipeline additionally:
/// 1. Parses `.tf` files to discover AWS resources and resolve variables
/// 2. Resolves Terraform resource types to IAM service/resource mappings
/// 3. Substitutes ARN placeholders with concrete Terraform resource names
pub async fn generate_policies(config: &GeneratePolicyConfig) -> Result<GeneratePoliciesResult> {
    let pipeline_start = Instant::now();

    debug!(
        "Using AWS context: partition={:?}, region={:?}, account={:?}",
        config.aws_context.partition, config.aws_context.region, config.aws_context.account
    );

    let mut enrichment_engine = EnrichmentEngine::new(config.disable_file_system_cache)?;

    // --- Optional Terraform resolution ---
    let has_terraform_inputs = config.terraform_dir.is_some()
        || !config.terraform_files.is_empty()
        || !config.tfstate_paths.is_empty();

    let terraform_resolver = if has_terraform_inputs {
        if let Some(ref terraform_dir) = config.terraform_dir {
            debug!("Terraform directory provided: {}", terraform_dir.display());
        }
        if !config.terraform_files.is_empty() {
            debug!(
                "{} individual Terraform files provided",
                config.terraform_files.len()
            );
        }
        if !config.tfstate_paths.is_empty() {
            debug!("{} tfstate files provided", config.tfstate_paths.len());
        }
        let loader = enrichment_engine.service_reference_loader();
        let resolver = TerraformResourceResolver::new(
            config.terraform_dir.as_deref(),
            &config.terraform_files,
            &config.tfstate_paths,
            &config.tfvars_files,
            loader,
        )
        .await
        .context("Failed to resolve Terraform resources")?;
        debug!("Resolved {} resource groups from Terraform", resolver.len());
        Some(resolver)
    } else {
        None
    };

    // --- Determine source files ---
    let all_source_files: Vec<PathBuf> = config.extract_sdk_calls_config.source_files.clone();

    if all_source_files.is_empty() {
        info!("No source files found to process, returning empty policy list");
        return Ok(GeneratePoliciesResult {
            policies: vec![],
            explanations: None,
            resource_binding_explanations: None,
        });
    }

    // Create the extractor
    let extractor = crate::ExtractionEngine::new();

    // Process source files to get extracted methods
    let extracted_methods = process_source_files(
        &extractor,
        &all_source_files,
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
            resource_binding_explanations: None,
        });
    }

    // Run the complete enrichment pipeline
    let enriched_results = enrichment_engine
        .enrich_methods(&extracted_methods, sdk)
        .await?;

    let enrichment_duration = pipeline_start.elapsed();
    trace!("Enrichment pipeline completed in {enrichment_duration:?}");

    // --- Optional Terraform ARN substitution ---
    // ARN substitution always happens when terraform inputs are present.
    // Binding explanations are only included when --explain-resources is provided,
    // and filtered by the ARN patterns.
    let (final_enriched, binding_explanations) = if let Some(ref resolver) = terraform_resolver {
        let bound = resolver.substitute_enriched_calls(&enriched_results);
        let explanations = if let Some(ref filters) = config.explain_resource_filters {
            let all_expl = resolver.build_binding_explanations();
            let total = all_expl.len();
            let filtered = filter_resource_explanations(all_expl, filters);
            debug!(
                "Terraform binding: {} binding explanations (filtered from {total})",
                filtered.len()
            );
            Some(filtered)
        } else {
            None
        };
        debug!("Terraform binding: substituted {} calls", bound.len());
        (bound, explanations)
    } else {
        (enriched_results, None)
    };

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
        final_enriched.len()
    );
    let result = policy_engine
        .generate_policies(&final_enriched)
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

    iam_policy_autopilot_common::telemetry::span::record_result_number(
        "num_policies_generated",
        final_policies.len(),
    );

    Ok(GeneratePoliciesResult {
        policies: final_policies,
        explanations,
        resource_binding_explanations: binding_explanations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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

    // -----------------------------------------------------------------------
    // arn_matches_pattern tests
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("wildcard_all", "arn:aws:s3:::my-bucket", "*", true)]
    #[case(
        "exact_match",
        "arn:aws:s3:::my-bucket",
        "arn:aws:s3:::my-bucket",
        true
    )]
    #[case("prefix_wildcard", "arn:aws:s3:::my-bucket", "arn:aws:s3:::*", true)]
    #[case(
        "service_wildcard",
        "arn:aws:dynamodb:us-east-1:123:table/t",
        "arn:*:dynamodb:*",
        true
    )]
    #[case("no_match", "arn:aws:s3:::my-bucket", "arn:aws:dynamodb:*", false)]
    #[case(
        "partial_no_match",
        "arn:aws:s3:::my-bucket",
        "arn:aws:s3:::other-*",
        false
    )]
    #[case(
        "exact_no_match",
        "arn:aws:s3:::my-bucket",
        "arn:aws:s3:::my-bucke",
        false
    )]
    #[case(
        "middle_wildcard",
        "arn:aws:s3:::my-bucket/key.txt",
        "arn:aws:s3:::*/key.txt",
        true
    )]
    #[case(
        "partition_wildcard",
        "arn:${Partition}:s3:::my-bucket",
        "arn:*:s3:::*",
        true
    )]
    fn test_arn_matches_pattern(
        #[case] _name: &str,
        #[case] arn: &str,
        #[case] pattern: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(
            arn_matches_pattern(arn, pattern),
            expected,
            "[{_name}] arn={arn}, pattern={pattern}"
        );
    }

    // -----------------------------------------------------------------------
    // filter_resource_explanations tests
    // -----------------------------------------------------------------------

    /// Helper: build test explanations with the given ARNs.
    #[cfg(test)]
    fn make_explanations(arns: &[&str]) -> Vec<ResourceBindingExplanation> {
        arns.iter()
            .map(|arn| ResourceBindingExplanation {
                arn: arn.to_string(),
                source: crate::enrichment::terraform::BindingSource::Terraform,
                resource_type: "aws_test".to_string(),
                resource_name: "test".to_string(),
                location: crate::Location::new(std::path::PathBuf::from("main.tf"), (1, 1), (1, 1)),
            })
            .collect()
    }

    #[rstest]
    // Wildcard matches all
    #[case(
        "all",
        &["arn:aws:s3:::bucket-a", "arn:aws:dynamodb:us-east-1:123:table/t"],
        &["*"],
        &["arn:aws:s3:::bucket-a", "arn:aws:dynamodb:us-east-1:123:table/t"]
    )]
    // Filter by S3 service
    #[case(
        "s3_only",
        &["arn:aws:s3:::bucket-a", "arn:aws:dynamodb:us-east-1:123:table/t"],
        &["arn:aws:s3:::*"],
        &["arn:aws:s3:::bucket-a"]
    )]
    // No matches
    #[case(
        "no_match",
        &["arn:aws:s3:::bucket-a"],
        &["arn:aws:lambda:*"],
        &[]
    )]
    // Multiple filters
    #[case(
        "multi_filter",
        &["arn:aws:s3:::b", "arn:aws:dynamodb:us-east-1:123:table/t", "arn:aws:sqs:us-east-1:123:q"],
        &["arn:aws:s3:::*", "arn:aws:sqs:*"],
        &["arn:aws:s3:::b", "arn:aws:sqs:us-east-1:123:q"]
    )]
    fn test_filter_resource_explanations(
        #[case] _name: &str,
        #[case] arns: &[&str],
        #[case] filters: &[&str],
        #[case] expected_arns: &[&str],
    ) {
        let explanations = make_explanations(arns);
        let filter_strings: Vec<String> = filters.iter().map(|s| s.to_string()).collect();
        let result = filter_resource_explanations(explanations, &filter_strings);
        let result_arns: Vec<&str> = result.iter().map(|e| e.arn.as_str()).collect();
        assert_eq!(result_arns, expected_arns, "[{_name}] ARN list mismatch");
    }
}
