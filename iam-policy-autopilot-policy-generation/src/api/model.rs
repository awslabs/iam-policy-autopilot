//! Defined model for API
use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use crate::enrichment::terraform::ResourceBindingExplanation;
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
    /// Optional Terraform project directory for resource binding.
    /// When provided, .tf files are parsed to discover AWS resources and trace source code.
    /// Source files from the terraform directory supplement the explicit source_files list.
    pub terraform_dir: Option<PathBuf>,
    /// Optional individual Terraform `.tf` files for resource binding.
    /// These are combined with any files discovered from `terraform_dir`.
    pub terraform_files: Vec<PathBuf>,
    /// Optional paths to `terraform.tfstate` files for enhanced ARN resolution.
    /// State-derived ARNs take precedence over HCL-constructed ones.
    pub tfstate_paths: Vec<PathBuf>,
    /// Optional explicit `.tfvars` file paths for variable overrides.
    /// When provided, these take precedence over auto-discovered `.tfvars` files
    /// from the terraform directory. Applied in order (later files override earlier).
    pub tfvars_files: Vec<PathBuf>,
    /// Optional ARN patterns to filter resource binding explanations.
    /// - `None`: No resource binding explanations generated
    /// - `Some(patterns)`: Generate explanations for resources matching the patterns
    ///   (supports wildcards like "arn:aws:s3:::*", "*" for all)
    pub explain_resource_filters: Option<Vec<String>>,
    /// Resource lists with more than this many entries are collapsed to '*' instead of emitting every resource-specific ARN. Use 0 to collapse every non-empty resource list. Default: 4.
    pub resource_cutoff: usize,
}

/// Result of policy generation including policies, action mappings, and explanations
#[derive(Debug, Clone, Serialize, Builder)]
#[serde(rename_all = "PascalCase")]
#[builder(setter(into))]
pub struct GeneratePoliciesResult {
    /// Generated IAM policies
    pub policies: Vec<PolicyWithMetadata>,
    /// Explanations for why actions were added. Empty unless explanations
    /// were requested and at least one action matched the filters.
    #[serde(skip_serializing_if = "Explanations::is_empty")]
    #[builder(default)]
    pub explanations: Explanations,
    /// Explanations for where resource ARNs came from (Terraform bindings)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub resource_binding_explanations: Option<Vec<ResourceBindingExplanation>>,
    /// Warnings about statements that could not be scoped to specific resources
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub warnings: Vec<PolicyWarning>,
}

/// Machine-recognizable category of a [`PolicyWarning`].
///
/// Callers should branch on this type (rather than parsing `message`) to
/// construct their own messages from the warning's `sid` and `actions`
/// metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum PolicyWarningType {
    /// The statement's `Resource` fell back to the `"*"` wildcard because no
    /// resource-specific ARN could be determined (no ARN patterns available,
    /// an empty resource list, or the resource list was collapsed by the
    /// resource cutoff).
    WildcardResource,
}

/// Location of a statement within a generated policy result, expressed as
/// index paths into the result rather than source line/col (which is the
/// role of [`crate::Location`]).
///
/// The location is currently resolved to the statement level. If finer
/// precision is needed later (e.g., a specific action or condition), add
/// further optional index fields here so an existing consumer keeps working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PolicyLocation {
    /// Index of the policy in `policies`
    pub policy_index: usize,
    /// Index of the statement within that policy's statement list
    pub statement_index: usize,
}

/// Warning attached to a generated policy statement that needs review.
///
/// The `warning_type` identifies the warning programmatically, and
/// `location` points at the flagged statement in the result. Callers
/// producing their own messages should branch on `warning_type` and read
/// any details (actions, resources) from the located statement itself;
/// `message` is a convenience English rendering for CLI users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PolicyWarning {
    /// Machine-recognizable warning category
    pub warning_type: PolicyWarningType,
    /// Location of the flagged statement within the generated policies
    pub location: PolicyLocation,
    /// Human-readable description of the warning (English only)
    pub message: String,
}

impl PolicyWarning {
    /// Scan generated policies for statements whose `Resource` contains the
    /// `"*"` wildcard and produce one warning per flagged statement.
    #[must_use]
    pub fn wildcard_resource_warnings(policies: &[PolicyWithMetadata]) -> Vec<Self> {
        let mut warnings = Vec::new();
        for (policy_index, policy_with_metadata) in policies.iter().enumerate() {
            for (statement_index, statement) in
                policy_with_metadata.policy.statements.iter().enumerate()
            {
                if statement.resource.iter().any(|resource| resource == "*") {
                    warnings.push(Self {
                        warning_type: PolicyWarningType::WildcardResource,
                        location: PolicyLocation {
                            policy_index,
                            statement_index,
                        },
                        message: "Statement could not be scoped to specific resources and \
                                  uses Resource \"*\". Review whether broad resource access \
                                  is intended."
                            .to_string(),
                    });
                }
            }
        }
        warnings
    }
}

impl GeneratePoliciesResult {
    /// Create an empty result with no policies or explanations.
    #[must_use]
    pub fn empty() -> Self {
        GeneratePoliciesResultBuilder::default()
            .policies(vec![])
            .build()
            .expect("GeneratePoliciesResultBuilder missing required policies")
    }
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
    use crate::policy_generation::{IamPolicy, PolicyType, Statement};

    fn policy_with_statements(statements: Vec<Statement>) -> PolicyWithMetadata {
        let mut policy = IamPolicy::new();
        for statement in statements {
            policy.add_statement(statement);
        }
        PolicyWithMetadata {
            policy,
            policy_type: PolicyType::Identity,
        }
    }

    /// Expected warning for a statement, matching the message produced by
    /// `wildcard_resource_warnings`.
    fn wildcard_warning(policy_index: usize, statement_index: usize) -> PolicyWarning {
        PolicyWarning {
            warning_type: PolicyWarningType::WildcardResource,
            location: PolicyLocation {
                policy_index,
                statement_index,
            },
            message: "Statement could not be scoped to specific resources and \
                      uses Resource \"*\". Review whether broad resource access \
                      is intended."
                .to_string(),
        }
    }

    /// Property: `wildcard_resource_warnings` flags exactly the statements
    /// whose Resource list contains the bare `"*"` wildcard. Statements
    /// scoped to specific ARNs, or to ARNs with embedded wildcards (e.g.,
    /// `arn:aws:s3:::*/*`), are not flagged.
    #[rstest::rstest]
    // Bare "*" statements are flagged across policies, scoped ones are not
    #[case::flags_bare_wildcards(
        vec![
            policy_with_statements(vec![
                Statement::allow(
                    vec!["s3:GetObject".to_string()],
                    vec!["arn:aws:s3:::my-bucket/*".to_string()],
                ),
                Statement::allow(
                    vec!["s3:ListAllMyBuckets".to_string()],
                    vec!["*".to_string()],
                ),
            ]),
            policy_with_statements(vec![Statement::allow(
                vec!["ec2:DescribeInstances".to_string()],
                vec!["*".to_string()],
            )]),
        ],
        vec![wildcard_warning(0, 1), wildcard_warning(1, 0)]
    )]
    // Fully scoped statements produce no warnings
    #[case::all_scoped(
        vec![policy_with_statements(vec![Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        )])],
        vec![]
    )]
    // ARN-embedded wildcards are scoped to a service and are not the bare
    // "*" fallback this warning targets
    #[case::arn_wildcards_not_flagged(
        vec![policy_with_statements(vec![Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::*/*".to_string()],
        )])],
        vec![]
    )]
    fn test_wildcard_resource_warnings(
        #[case] policies: Vec<PolicyWithMetadata>,
        #[case] expected: Vec<PolicyWarning>,
    ) {
        assert_eq!(
            PolicyWarning::wildcard_resource_warnings(&policies),
            expected
        );
    }

    #[test]
    fn test_policy_warning_serialization() {
        let warning = PolicyWarning {
            warning_type: PolicyWarningType::WildcardResource,
            location: PolicyLocation {
                policy_index: 0,
                statement_index: 1,
            },
            message: "test message".to_string(),
        };

        let json = serde_json::to_value(&warning).unwrap();
        assert_eq!(json["WarningType"], "WildcardResource");
        assert_eq!(json["Location"]["PolicyIndex"], 0);
        assert_eq!(json["Location"]["StatementIndex"], 1);
        assert_eq!(json["Message"], "test message");
    }

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
