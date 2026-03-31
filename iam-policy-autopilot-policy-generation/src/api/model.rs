//! Defined model for API
use serde::{Deserialize, Serialize};

use crate::{
    embedded_data::BotocoreData, enrichment::Explanations, policy_generation::PolicyWithMetadata,
};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Configuration for generate_policies API
#[derive(Debug, Clone)]
pub struct GeneratePolicyConfig {
    /// Config used to extract sdk calls for policy generation
    pub extract_sdk_calls_config: ExtractSdkCallsConfig,
    /// AWS Config
    pub aws_context: AwsContext,
    /// Output individual policies
    pub individual_policies: bool,
    /// Enable policy size minimization
    pub minimize_policy_size: bool,
    /// Disable file system caching for service references
    pub disable_file_system_cache: bool,
    /// Generate explanations for why actions were added, filtered by patterns.
    /// - `None`: No explanations generated
    /// - `Some(patterns)`: Generate explanations for actions matching the patterns
    ///   (supports wildcards like "s3:*", "ec2:Get*", "*" for all)
    pub explain_filters: Option<Vec<String>>,
}

/// Result of policy generation including policies, action mappings, and explanations
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GeneratePoliciesResult {
    /// Generated IAM policies
    pub policies: Vec<PolicyWithMetadata>,
    /// Explanations for why actions were added (if requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanations: Option<Explanations>,
}

/// Service hints for filtering SDK method calls
#[derive(Debug, Clone)]
pub struct ServiceHints {
    /// List of AWS service names to filter by
    pub service_names: Vec<String>,
}

/// Configuration for extract_sdk_calls Api
#[derive(Debug, Clone)]
pub struct ExtractSdkCallsConfig {
    /// Enable pretty JSON output formatting
    pub source_files: Vec<PathBuf>,
    /// Override programming language detection
    pub language: Option<String>,
    /// Optional service hints for filtering
    pub service_hints: Option<ServiceHints>,
}

// Todo: Find a better place for this or refactor rest of the code to use model
/// Aws context for policy
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AwsContext {
    /// AWS partition
    pub partition: String,
    /// AWS region
    pub region: String,
    /// AWS account ID
    pub account: String,
}

include!("../shared_submodule_model.rs");

impl AwsContext {
    /// Creates a new AwsContext with the partition automatically derived from the region, using
    /// Botocore data, which includes a regex of possible region names for each partition. This
    /// approach should ensure new regions not known at compilation time are correctly handled.
    ///
    /// If a region of "*" is provided, the partition will also be set to "*", and generated
    /// policies should be generic over all possible regions and partitions where possible.
    ///
    /// # Examples
    /// ```
    /// use iam_policy_autopilot_policy_generation::api::model::AwsContext;
    ///
    /// let ctx = AwsContext::new("us-east-1".to_string(), "123456789012".to_string()).unwrap();
    /// assert_eq!(ctx.partition, "aws");
    ///
    /// let ctx = AwsContext::new("cn-north-1".to_string(), "123456789012".to_string()).unwrap();
    /// assert_eq!(ctx.partition, "aws-cn");
    ///
    /// let ctx = AwsContext::new("us-gov-west-1".to_string(), "123456789012".to_string()).unwrap();
    /// assert_eq!(ctx.partition, "aws-us-gov");
    ///
    /// let ctx = AwsContext::new("eusc-de-east-1".to_string(), "123456789012".to_string()).unwrap();
    /// assert_eq!(ctx.partition, "aws-eusc");
    ///
    /// let ctx = AwsContext::new("*".to_string(), "*".to_string()).unwrap();
    /// assert_eq!(ctx.partition, "*");
    /// ```
    pub fn new(region: String, account: String) -> Result<Self> {
        let partition = if region == "*" {
            "*".to_string()
        } else {
            BotocoreData::get_partitions()?
                .partitions
                .iter()
                .find(|(_, region_regex)| region_regex.is_match(&region))
                .map(|(partition_id, _)| partition_id.clone())
                .ok_or(anyhow!(
                    "could not determine partition of region {region} using botocore data"
                ))?
        };
        Ok(Self {
            partition,
            region,
            account,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_context_partition_derivation() {
        // Test China regions
        let ctx = AwsContext::new("cn-north-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws-cn");

        let ctx =
            AwsContext::new("cn-northwest-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws-cn");

        // Test GovCloud regions
        let ctx = AwsContext::new("us-gov-west-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws-us-gov");

        let ctx = AwsContext::new("us-gov-east-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws-us-gov");

        // Test EU Sovereign Cloud regions
        let ctx =
            AwsContext::new("eusc-de-east-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws-eusc");

        // Test standard AWS regions
        let ctx = AwsContext::new("us-east-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws");

        let ctx = AwsContext::new("us-west-2".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws");

        let ctx = AwsContext::new("eu-west-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws");

        let ctx =
            AwsContext::new("ap-southeast-1".to_string(), "123456789012".to_string()).unwrap();
        assert_eq!(ctx.partition, "aws");

        // Test wildcard
        let ctx = AwsContext::new("*".to_string(), "*".to_string()).unwrap();
        assert_eq!(ctx.partition, "*");
    }

    #[test]
    fn test_aws_context_invalid_partitions() {
        assert!(AwsContext::new("not-a-region".to_string(), "123456789012".to_string()).is_err());
    }
}
