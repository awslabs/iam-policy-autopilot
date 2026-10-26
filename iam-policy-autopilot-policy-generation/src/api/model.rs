//! Defined model for API
use derive_builder::Builder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::enrichment::terraform::ResourceBindingExplanation;
use crate::{
    embedded_data::BotocoreData, enrichment::Explanations, policy_generation::PolicyWithMetadata,
};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Configuration for generate_policies API
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct GeneratePolicyConfig {
    /// Config used to extract sdk calls for policy generation
    pub extract_sdk_calls_config: ExtractSdkCallsConfig,
    /// AWS Config
    pub aws_context: AwsContext,
    /// Output individual policies
    pub individual_policies: bool,
    /// Enable policy size minimization
    pub minimize_policy_size: bool,
    /// Maximum generated policy size in non-whitespace JSON characters.
    ///
    /// Statements that do not fit start a new policy. Oversized
    /// single-statement policies are still emitted with a warning; generation
    /// continues.
    ///
    /// Use [`crate::DEFAULT_MAX_POLICY_SIZE`] for the AWS IAM managed policy
    /// default.
    pub max_policy_size: usize,
    /// Best-effort total output size target that may be met by introducing
    /// overpermissive action wildcards.
    pub allow_overpermissive_wildcards_to_limit_total_output_size: Option<usize>,
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
    /// Warnings about generated statements and output sizing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub warnings: Vec<PolicyWarning>,
}

/// Compact JSON size used by IAM policy limits.
#[must_use]
pub(crate) fn compact_policy_size(policy: &crate::policy_generation::IamPolicy) -> usize {
    serde_json::to_string(policy)
        .expect("IAM policies are always serializable")
        .chars()
        .filter(|character| !character.is_whitespace())
        .count()
}

/// Machine-recognizable category of a [`PolicyWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[non_exhaustive]
pub enum PolicyWarningType {
    /// The statement's `Resource` fell back to the `"*"` wildcard because no
    /// resource-specific ARN could be determined (no ARN patterns available,
    /// an empty resource list, or the resource list was collapsed by the
    /// resource cutoff).
    WildcardResource,
    /// The statement contains an action wildcard and may grant actions that
    /// were not expected by the caller.
    WildcardAction,
    /// A policy could not be split further without splitting a statement.
    PolicyExceedsMaximumSize,
    /// Wildcard optimization could not reach the requested total size.
    TotalOutputSizeTargetNotMet,
}

/// Location of a statement within a generated policy result, expressed as
/// index paths into the result rather than source line/col (which is the
/// role of [`crate::Location`]).
///
/// The location is currently resolved to the statement level. If finer
/// precision is needed later (e.g., a specific action or condition), add
/// further optional index fields here so an existing consumer keeps working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct PolicyLocation {
    /// Index of the policy in `policies`
    pub policy_index: usize,
    /// Index of the statement within that policy's statement list
    pub statement_index: usize,
}

/// Warning attached to generated policy output.
///
/// Statement warnings include a `Location`, oversized-policy warnings include
/// a `PolicyIndex`, and total-output warnings require no separate location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "WarningType")]
#[non_exhaustive]
pub enum PolicyWarning {
    /// A statement uses the broad resource wildcard.
    WildcardResource {
        /// Location of the flagged statement.
        #[serde(rename = "Location")]
        location: PolicyLocation,
        /// Human-readable description.
        #[serde(rename = "Message")]
        message: String,
    },
    /// A statement contains an action wildcard.
    WildcardAction {
        /// Location of the flagged statement.
        #[serde(rename = "Location")]
        location: PolicyLocation,
        /// Human-readable description.
        #[serde(rename = "Message")]
        message: String,
    },
    /// A policy could not be split further without splitting a statement.
    PolicyExceedsMaximumSize {
        /// Index of the oversized policy in the result.
        #[serde(rename = "PolicyIndex")]
        policy_index: usize,
        /// Human-readable description, including configured and actual sizes.
        #[serde(rename = "Message")]
        message: String,
    },
    /// Wildcard optimization could not reach the requested total size.
    TotalOutputSizeTargetNotMet {
        /// Human-readable description, including target and actual sizes.
        #[serde(rename = "Message")]
        message: String,
    },
}

impl PolicyWarning {
    /// Return the machine-recognizable warning category.
    #[must_use]
    pub fn warning_type(&self) -> PolicyWarningType {
        match self {
            Self::WildcardResource { .. } => PolicyWarningType::WildcardResource,
            Self::WildcardAction { .. } => PolicyWarningType::WildcardAction,
            Self::PolicyExceedsMaximumSize { .. } => PolicyWarningType::PolicyExceedsMaximumSize,
            Self::TotalOutputSizeTargetNotMet { .. } => {
                PolicyWarningType::TotalOutputSizeTargetNotMet
            }
        }
    }

    /// Return the statement location for statement-scoped warnings.
    #[must_use]
    pub fn location(&self) -> Option<&PolicyLocation> {
        match self {
            Self::WildcardResource { location, .. } | Self::WildcardAction { location, .. } => {
                Some(location)
            }
            Self::PolicyExceedsMaximumSize { .. } | Self::TotalOutputSizeTargetNotMet { .. } => {
                None
            }
        }
    }

    /// Scan generated policies for all statements that need wildcard review.
    ///
    /// Resource warnings are emitted before action warnings for a statement
    /// that contains both kinds of wildcard.
    #[must_use]
    pub fn warnings_for_policies(policies: &[PolicyWithMetadata]) -> Vec<Self> {
        let mut warnings = Vec::new();
        for (policy_index, policy_with_metadata) in policies.iter().enumerate() {
            for (statement_index, statement) in
                policy_with_metadata.policy.statements.iter().enumerate()
            {
                if statement.resource.iter().any(|resource| resource == "*") {
                    warnings.push(Self::wildcard_resource_warning(
                        policy_index,
                        statement_index,
                    ));
                }
                if statement.action.iter().any(|action| action.contains('*')) {
                    warnings.push(Self::wildcard_action_warning(policy_index, statement_index));
                }
            }
        }
        warnings
    }

    /// Build all ordered warnings for the final generated output.
    ///
    /// Statement warnings are followed by oversized-policy warnings ordered by
    /// policy index, then by an unmet total-output warning when applicable.
    #[must_use]
    pub(crate) fn warnings_for_generation(
        policies: &[PolicyWithMetadata],
        maximum_size: usize,
        total_output_target: Option<usize>,
    ) -> Vec<Self> {
        let mut warnings = Self::warnings_for_policies(policies);
        let mut total_size = 0;

        for (policy_index, policy) in policies.iter().enumerate() {
            let actual_size = compact_policy_size(&policy.policy);
            total_size += actual_size;
            if actual_size > maximum_size {
                warnings.push(Self::PolicyExceedsMaximumSize {
                    policy_index,
                    message: format!(
                        "Policy {policy_index} is {actual_size} characters, exceeding the configured maximum of {maximum_size}, because it contains a statement that cannot be split further."
                    ),
                });
            }
        }

        if let Some(target_size) = total_output_target.filter(|target| total_size > *target) {
            warnings.push(Self::TotalOutputSizeTargetNotMet {
                message: format!(
                    "The total output size target of {target_size} characters could not be met; the smallest best-effort result is {total_size} characters."
                ),
            });
        }

        warnings
    }

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
                    warnings.push(Self::wildcard_resource_warning(
                        policy_index,
                        statement_index,
                    ));
                }
            }
        }
        warnings
    }

    /// Scan generated policies for statements containing `*` in any action
    /// and produce one warning per flagged statement.
    #[must_use]
    pub fn wildcard_action_warnings(policies: &[PolicyWithMetadata]) -> Vec<Self> {
        let mut warnings = Vec::new();
        for (policy_index, policy_with_metadata) in policies.iter().enumerate() {
            for (statement_index, statement) in
                policy_with_metadata.policy.statements.iter().enumerate()
            {
                if statement.action.iter().any(|action| action.contains('*')) {
                    warnings.push(Self::wildcard_action_warning(policy_index, statement_index));
                }
            }
        }
        warnings
    }

    fn wildcard_resource_warning(policy_index: usize, statement_index: usize) -> Self {
        Self::WildcardResource {
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

    fn wildcard_action_warning(policy_index: usize, statement_index: usize) -> Self {
        Self::WildcardAction {
            location: PolicyLocation {
                policy_index,
                statement_index,
            },
            message: "Statement contains a wildcard in Action and may be more permissive \
                      than expected. Review whether broad action access is intended."
                .to_string(),
        }
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
        vec![
            PolicyWarning::wildcard_resource_warning(0, 1),
            PolicyWarning::wildcard_resource_warning(1, 0)
        ]
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

    #[rstest::rstest]
    #[case::embedded_wildcard("s3:Get*", true)]
    #[case::bare_wildcard("*", true)]
    #[case::exact_action("s3:GetObject", false)]
    fn test_wildcard_action_warnings(#[case] action: &str, #[case] should_warn: bool) {
        let policies = vec![policy_with_statements(vec![Statement::allow(
            vec![action.to_string()],
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        )])];

        let expected = if should_warn {
            vec![PolicyWarning::wildcard_action_warning(0, 0)]
        } else {
            vec![]
        };
        assert_eq!(PolicyWarning::wildcard_action_warnings(&policies), expected);
    }

    #[test]
    fn test_warnings_for_policies_includes_resource_and_action_warnings() {
        let policies = vec![policy_with_statements(vec![Statement::allow(
            vec!["s3:Get*".to_string()],
            vec!["*".to_string()],
        )])];

        assert_eq!(
            PolicyWarning::warnings_for_policies(&policies),
            vec![
                PolicyWarning::wildcard_resource_warning(0, 0),
                PolicyWarning::wildcard_action_warning(0, 0)
            ]
        );
    }

    #[test]
    fn test_policy_warning_serialization() {
        let warning = PolicyWarning::WildcardResource {
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
    fn test_wildcard_action_warning_serialization() {
        let warning = PolicyWarning::WildcardAction {
            location: PolicyLocation {
                policy_index: 0,
                statement_index: 1,
            },
            message: "test message".to_string(),
        };

        let json = serde_json::to_value(&warning).unwrap();
        assert_eq!(json["WarningType"], "WildcardAction");
        assert_eq!(json["Location"]["PolicyIndex"], 0);
        assert_eq!(json["Location"]["StatementIndex"], 1);
        assert_eq!(json["Message"], "test message");
    }

    #[test]
    fn test_oversized_policy_warning() {
        let policies = vec![policy_with_statements(vec![Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        )])];
        let actual_size = compact_policy_size(&policies[0].policy);

        let warnings = PolicyWarning::warnings_for_generation(&policies, actual_size - 1, None);

        assert_eq!(
            warnings,
            vec![PolicyWarning::PolicyExceedsMaximumSize {
                policy_index: 0,
                message: format!(
                    "Policy 0 is {actual_size} characters, exceeding the configured maximum of {}, because it contains a statement that cannot be split further.",
                    actual_size - 1
                ),
            }]
        );
    }

    #[test]
    fn test_warnings_are_ordered_and_serialized() {
        let policies = vec![
            policy_with_statements(vec![Statement::allow(
                vec!["s3:GetObject".to_string()],
                vec!["*".to_string()],
            )]),
            policy_with_statements(vec![Statement::allow(
                vec!["s3:PutObject".to_string()],
                vec!["*".to_string()],
            )]),
        ];

        let warnings = PolicyWarning::warnings_for_generation(&policies, 50, Some(50));
        assert_eq!(warnings.len(), 5);
        assert!(matches!(
            warnings[0],
            PolicyWarning::WildcardResource { .. }
        ));
        assert!(matches!(
            warnings[1],
            PolicyWarning::WildcardResource { .. }
        ));
        assert!(matches!(
            warnings[2],
            PolicyWarning::PolicyExceedsMaximumSize {
                policy_index: 0,
                ..
            }
        ));
        assert!(matches!(
            warnings[3],
            PolicyWarning::PolicyExceedsMaximumSize {
                policy_index: 1,
                ..
            }
        ));
        assert!(matches!(
            warnings[4],
            PolicyWarning::TotalOutputSizeTargetNotMet { .. }
        ));

        let json = serde_json::to_value(&warnings).unwrap();
        let first_policy_size = compact_policy_size(&policies[0].policy);
        let total_size = policies
            .iter()
            .map(|policy| compact_policy_size(&policy.policy))
            .sum::<usize>();
        assert_eq!(
            json[2],
            serde_json::json!({
                "WarningType": "PolicyExceedsMaximumSize",
                "PolicyIndex": 0,
                "Message": format!(
                    "Policy 0 is {first_policy_size} characters, exceeding the configured maximum of 50, because it contains a statement that cannot be split further."
                )
            })
        );
        assert_eq!(
            json[4],
            serde_json::json!({
                "WarningType": "TotalOutputSizeTargetNotMet",
                "Message": format!(
                    "The total output size target of 50 characters could not be met; the smallest best-effort result is {total_size} characters."
                )
            })
        );
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
