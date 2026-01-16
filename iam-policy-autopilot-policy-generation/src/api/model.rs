//! Defined model for API
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for generate_policies Api
#[derive(Debug, Clone)]
pub struct GeneratePolicyConfig {
    /// Config used to extract sdk calls for policy generation
    pub extract_sdk_calls_config: ExtractSdkCallsConfig,
    /// AWS Config
    pub aws_context: AwsContext,
    /// Generate Action Mappings
    pub generate_action_mappings: bool,
    /// Output individual policies
    pub individual_policies: bool,
    /// Enable policy size minimization
    pub minimize_policy_size: bool,
    /// Disable file system caching for service references
    pub disable_file_system_cache: bool,
    /// Use account resources in policy generation in place of wildcards
    pub use_account_context: bool,
    /// enable terraform support?
    pub use_terraform: bool,
    /// terraform directory to cwd into 
    pub terraform_dir: PathBuf
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
    /// Creates a new AwsContext with the partition automatically derived from the region.
    ///
    /// # Partition Rules
    /// - Regions starting with "cn-" use "aws-cn"
    /// - Regions starting with "us-gov-" use "aws-us-gov"
    /// - All other regions use "aws"
    ///
    /// # Examples
    /// ```
    /// use iam_policy_autopilot_policy_generation::api::model::AwsContext;
    ///
    /// let ctx = AwsContext::new("us-east-1".to_string(), "123456789012".to_string());
    /// assert_eq!(ctx.partition, "aws");
    ///
    /// let ctx = AwsContext::new("cn-north-1".to_string(), "123456789012".to_string());
    /// assert_eq!(ctx.partition, "aws-cn");
    ///
    /// let ctx = AwsContext::new("us-gov-west-1".to_string(), "123456789012".to_string());
    /// assert_eq!(ctx.partition, "aws-us-gov");
    /// ```
    pub fn new(region: String, account: String) -> Self {
        let partition = if region.starts_with("cn-") {
            "aws-cn".to_string()
        } else if region.starts_with("us-gov-") {
            "aws-us-gov".to_string()
        } else {
            "aws".to_string()
        };

        Self {
            partition,
            region,
            account,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_context_partition_derivation() {
        // Test China regions
        let ctx = AwsContext::new("cn-north-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws-cn");

        let ctx = AwsContext::new("cn-northwest-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws-cn");

        // Test GovCloud regions
        let ctx = AwsContext::new("us-gov-west-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws-us-gov");

        let ctx = AwsContext::new("us-gov-east-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws-us-gov");

        // Test standard AWS regions
        let ctx = AwsContext::new("us-east-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws");

        let ctx = AwsContext::new("us-west-2".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws");

        let ctx = AwsContext::new("eu-west-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws");

        let ctx = AwsContext::new("ap-southeast-1".to_string(), "123456789012".to_string());
        assert_eq!(ctx.partition, "aws");

        // Test wildcard
        let ctx = AwsContext::new("*".to_string(), "*".to_string());
        assert_eq!(ctx.partition, "aws");
    }
}
