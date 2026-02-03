//! IAM Policy Uploader
//!
//! This module provides functionality for uploading IAM policies to AWS using the IAM service.
//! It includes policy name generation with automatic numbering and policy listing capabilities.

use aws_config::BehaviorVersion;
use aws_sdk_iam::operation::create_policy::CreatePolicyError;
use aws_sdk_iam::operation::list_policies::ListPoliciesError;
use aws_sdk_iam::Client as IamClient;
use aws_smithy_runtime_api::client::result::SdkError;
use iam_policy_autopilot_policy_generation::{IamPolicy, PolicyWithMetadata};
use regex::Regex;
use thiserror::Error;

/// Default name constant used for generated policy names
const DEFAULT_NAME: &str = "IamPolicyAutopilotGeneratedPolicy";

/// https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_iam-quotas.html#reference_iam-quotas-entity-length
const MAX_POLICY_NAME_LEN: usize = 128;

/// We estimate a conservative 6 characters for the postfix, i.e., the number after the name
/// This gives us up to 999,999 policies, the maximum quota is currently 5,000:
/// https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_iam-quotas.html#reference_iam-quotas-entities
const POLICY_NAME_POSTFIX: usize = 6;

/// Link to the IAM name specification.
const NAME_SPEC_LINK: &str = "https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_iam-quotas.html#reference_iam-quotas-names";

/// Errors that can occur during policy upload operations
#[derive(Error, Debug)]
pub enum UploaderError {
    /// AWS IAM list policies error
    #[error("AWS IAM list policies error: {0}")]
    ListPolicies(#[from] SdkError<ListPoliciesError, aws_smithy_runtime_api::http::Response>),

    /// AWS IAM create policy error
    #[error("AWS IAM create policy error: {0}")]
    CreatePolicy(#[from] SdkError<CreatePolicyError, aws_smithy_runtime_api::http::Response>),

    /// JSON serialization error
    #[error("JSON serialization error: {0}")]
    JsonSerialization(#[from] serde_json::Error),

    /// AWS configuration error
    #[error("AWS configuration error: {0}")]
    AwsConfig(String),

    /// Invalid policy name error
    #[error("Invalid policy name: '{0}': {1}")]
    InvalidPolicyName(String, String),
}

/// Result type for uploader operations
pub type UploaderResult<T> = Result<T, UploaderError>;

/// IAM Policy Uploader client
pub struct PolicyUploader {
    client: IamClient,
}

/// Response from uploading a policy
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UploadResponse {
    /// The name of the uploaded policy
    pub policy_name: String,
    /// The ARN of the uploaded policy
    pub policy_arn: String,
    /// The unique policy ID
    pub policy_id: String,
}

/// Response from uploading multiple policies
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BatchUploadResponse {
    /// Successfully uploaded policies
    pub successful: Vec<UploadResponse>,
    /// Failed uploads with their error messages
    pub failed: Vec<(usize, String)>,
}

impl PolicyUploader {
    /// Create a new PolicyUploader with default AWS configuration
    pub async fn new() -> UploaderResult<Self> {
        let config = aws_config::defaults(BehaviorVersion::latest()).load().await;

        let client = IamClient::new(&config);

        Ok(Self { client })
    }

    /// Create a new PolicyUploader with custom AWS configuration
    pub fn with_client(client: IamClient) -> Self {
        Self { client }
    }

    /// Validate that a policy name follows AWS naming requirements
    /// Names must be alphanumeric, including: plus (+), equals (=), comma (,),
    /// period (.), at (@), underscore (_), and hyphen (-)
    #[allow(clippy::result_large_err)]
    fn validate_policy_name(name: &str) -> UploaderResult<()> {
        if name.is_empty() {
            return Err(UploaderError::InvalidPolicyName(
                name.to_string(),
                "Name cannot be empty".to_string(),
            ));
        }

        let max_len = MAX_POLICY_NAME_LEN - DEFAULT_NAME.len() - POLICY_NAME_POSTFIX;

        // AWS policy names can be 1-128 characters
        if name.len() > max_len {
            return Err(UploaderError::InvalidPolicyName(
                name.to_string(),
                format!("Name cannot exceed {} characters", max_len),
            ));
        }

        // Check for valid characters: alphanumeric + = , . @ _ -
        let valid_chars = Regex::new(r"^[a-zA-Z0-9+=,.@_-]+$")
            .expect("Valid regex pattern for policy name validation");
        if !valid_chars.is_match(name) {
            return Err(UploaderError::InvalidPolicyName(
                name.to_string(),
                format!(
                    "Names must be alphanumeric and can include: + = , . @ _ - (see {})",
                    NAME_SPEC_LINK
                ),
            ));
        }

        Ok(())
    }

    /// List all IAM policies in the account
    async fn list_policies(&self) -> UploaderResult<Vec<String>> {
        let mut policy_names = Vec::new();
        let mut marker = None;

        loop {
            let mut request = self
                .client
                .list_policies()
                .scope(aws_sdk_iam::types::PolicyScopeType::Local);

            if let Some(m) = marker {
                request = request.marker(m);
            }

            let response = request.send().await?;

            if let Some(policies) = response.policies {
                for policy in policies {
                    if let Some(name) = policy.policy_name {
                        policy_names.push(name);
                    }
                }
            }

            marker = response.marker;
            if !response.is_truncated {
                break;
            }
        }

        Ok(policy_names)
    }

    /// Generate a unique policy name using the specified pattern
    ///
    /// # Arguments
    ///
    /// * `existing_policies` - List of existing policy names
    /// * `custom_name` - Optional custom name prefix (if None, uses TOOL_NAME)
    ///
    /// # Returns
    ///
    /// A unique policy name following the pattern <prefix>Policy<n> where n starts from 1
    #[allow(clippy::result_large_err)]
    fn generate_policy_name(
        existing_policies: &[String],
        custom_name: Option<&str>,
    ) -> UploaderResult<String> {
        let prefix = custom_name.unwrap_or(DEFAULT_NAME);

        // Validate the custom name if provided
        if let Some(name) = custom_name {
            Self::validate_policy_name(name)?;
        }

        // Append the `_` so there is divider in case the custom_name ends in a number
        let prefix = format!("{}_", prefix);

        // Filter existing policies that match our pattern and extract their numbers
        let mut used_numbers = Vec::new();
        for policy_name in existing_policies {
            if let Some(suffix) = policy_name.strip_prefix(&prefix) {
                // Try to parse the remaining part as a number
                if let Ok(num) = suffix.parse::<u32>() {
                    used_numbers.push(num);
                }
            }
        }

        // Sort the numbers to find gaps
        used_numbers.sort_unstable();

        // Find the lowest available number starting from 1
        let mut next_number = 1u32;
        for &used_num in &used_numbers {
            if used_num == next_number {
                next_number += 1;
            } else if used_num > next_number {
                // Found a gap, use next_number
                break;
            }
        }

        Ok(format!("{}{}", prefix, next_number))
    }

    /// Upload multiple IAM policies to AWS
    ///
    /// # Arguments
    ///
    /// * `policies` - Slice of IAM policies to upload
    /// * `custom_name` - Optional custom name prefix for all policies
    ///
    /// # Returns
    ///
    /// A `BatchUploadResponse` containing successful uploads and any failures
    pub async fn upload_policies(
        &self,
        policies: &[PolicyWithMetadata],
        custom_name: Option<&str>,
    ) -> UploaderResult<BatchUploadResponse> {
        if policies.is_empty() {
            return Ok(BatchUploadResponse {
                successful: Vec::new(),
                failed: Vec::new(),
            });
        }

        // Validate custom name if provided
        if let Some(name) = custom_name {
            Self::validate_policy_name(name)?;
        }

        // Get existing policies once for all uploads
        let mut existing_policy_names = self.list_policies().await?;

        let mut successful = Vec::new();
        let mut failed = Vec::new();

        for (index, policy) in policies.iter().enumerate() {
            match self
                .upload_single_policy_with_existing(
                    &policy.policy,
                    custom_name,
                    &mut existing_policy_names,
                )
                .await
            {
                Ok(response) => {
                    // Add the newly created policy name to the existing list to avoid conflicts
                    existing_policy_names.push(response.policy_name.clone());
                    successful.push(response);
                }
                Err(error) => {
                    failed.push((index, error.to_string()));
                }
            }
        }

        Ok(BatchUploadResponse { successful, failed })
    }

    /// Helper method to upload a single policy with a pre-fetched existing policies list
    #[allow(clippy::ptr_arg)]
    async fn upload_single_policy_with_existing(
        &self,
        policy: &IamPolicy,
        custom_name: Option<&str>,
        existing_policies: &mut Vec<String>,
    ) -> UploaderResult<UploadResponse> {
        // Generate a unique policy name
        let policy_name = Self::generate_policy_name(existing_policies, custom_name)?;

        // Serialize the policy to JSON
        let policy_document = serde_json::to_string(policy)?;

        // Upload the policy
        let response = self
            .client
            .create_policy()
            .policy_name(&policy_name)
            .policy_document(policy_document)
            .send()
            .await?;

        let created_policy = response.policy.ok_or_else(|| {
            UploaderError::AwsConfig("No policy returned from CreatePolicy".to_string())
        })?;

        log::info!("Successfully created policy: {}", policy_name);

        Ok(UploadResponse {
            policy_name: created_policy.policy_name.unwrap_or(policy_name),
            policy_arn: created_policy.arn.unwrap_or_default(),
            policy_id: created_policy.policy_id.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iam_policy_autopilot_policy_generation::{IamPolicy, Statement};

    #[test]
    fn test_generate_policy_name_default() {
        let existing = Vec::new();
        let name = PolicyUploader::generate_policy_name(&existing, None).unwrap();
        assert_eq!(name, "IamPolicyAutopilotGeneratedPolicy_1");
    }

    #[test]
    fn test_generate_policy_name_custom() {
        let existing = Vec::new();
        let name = PolicyUploader::generate_policy_name(&existing, Some("CustomName")).unwrap();
        assert_eq!(name, "CustomName_1");
    }

    #[test]
    fn test_generate_policy_name_with_existing() {
        let existing = vec![
            "IamPolicyAutopilotGeneratedPolicy_1".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_2".to_string(),
        ];

        let name = PolicyUploader::generate_policy_name(&existing, None).unwrap();
        assert_eq!(name, "IamPolicyAutopilotGeneratedPolicy_3");
    }

    #[test]
    fn test_generate_policy_name_custom_with_existing() {
        let existing = vec!["MyPolicy_1".to_string(), "MyPolicy_3".to_string()];

        let name = PolicyUploader::generate_policy_name(&existing, Some("MyPolicy")).unwrap();
        assert_eq!(name, "MyPolicy_2");
    }

    #[test]
    fn test_policy_serialization() {
        // Create a sample IAM policy
        let mut policy = IamPolicy::new();
        let statement = Statement::allow(
            vec!["s3:GetObject".to_string(), "s3:PutObject".to_string()],
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        );
        policy.add_statement(statement);

        // Test that we can serialize it to JSON
        let json_result = serde_json::to_string(&policy);
        assert!(json_result.is_ok());

        let json = json_result.unwrap();
        assert!(json.contains("\"Version\":\"2012-10-17\""));
        assert!(json.contains("\"Effect\":\"Allow\""));
        assert!(json.contains("\"Action\":[\"s3:GetObject\",\"s3:PutObject\"]"));
        assert!(json.contains("\"Resource\":[\"arn:aws:s3:::my-bucket/*\"]"));
    }

    #[test]
    fn test_tool_name_constant() {
        assert_eq!(DEFAULT_NAME, "IamPolicyAutopilotGeneratedPolicy");
    }

    #[test]
    fn test_policy_name_generation_with_gaps() {
        // Test that the algorithm finds gaps in the numbering
        let existing = vec![
            "IamPolicyAutopilotGeneratedPolicy_1".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_3".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_5".to_string(),
        ];

        let name = PolicyUploader::generate_policy_name(&existing, None).unwrap();
        assert_eq!(name, "IamPolicyAutopilotGeneratedPolicy_2"); // Should find the gap at 2
    }

    #[test]
    fn test_policy_name_generation_with_non_matching_policies() {
        // Test that non-matching policies are ignored
        let existing = vec![
            "SomeOtherPolicy1".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_1".to_string(),
            "DifferentPolicyName".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_3".to_string(),
        ];

        let name = PolicyUploader::generate_policy_name(&existing, None).unwrap();
        assert_eq!(name, "IamPolicyAutopilotGeneratedPolicy_2"); // Should find the gap at 2
    }

    #[test]
    fn test_policy_name_generation_with_invalid_suffixes() {
        // Test that policies with invalid number suffixes are ignored
        let existing = vec![
            "IamPolicyAutopilotGeneratedPolicy_1".to_string(),
            "IamPolicyAutopilotGeneratedPolicy_ABC".to_string(), // Invalid suffix
            "IamPolicyAutopilotGeneratedPolicy_".to_string(),    // No suffix
            "IamPolicyAutopilotGeneratedPolicy_2".to_string(),
        ];

        let name = PolicyUploader::generate_policy_name(&existing, None).unwrap();
        assert_eq!(name, "IamPolicyAutopilotGeneratedPolicy_3");
    }

    #[test]
    fn test_validate_policy_name_valid() {
        // Test valid names
        assert!(PolicyUploader::validate_policy_name("ValidName123").is_ok());
        assert!(PolicyUploader::validate_policy_name("Name+With=Special,Chars.@_-").is_ok());
        assert!(PolicyUploader::validate_policy_name("a").is_ok());
    }

    #[test]
    fn test_validate_policy_name_invalid() {
        // Test invalid names
        assert!(PolicyUploader::validate_policy_name("").is_err());
        assert!(PolicyUploader::validate_policy_name("Name With Spaces").is_err());
        assert!(PolicyUploader::validate_policy_name("Name#WithHash").is_err());
        assert!(PolicyUploader::validate_policy_name("Name$WithDollar").is_err());
        assert!(PolicyUploader::validate_policy_name("Name%WithPercent").is_err());

        // Test name too long (over 128 characters)
        let long_name = "a".repeat(129);
        assert!(PolicyUploader::validate_policy_name(&long_name).is_err());
    }

    #[test]
    fn test_validate_policy_name_edge_cases() {
        // Test exactly the max allowed length for custom names (considering the validation logic)
        let max_len = MAX_POLICY_NAME_LEN - DEFAULT_NAME.len() - POLICY_NAME_POSTFIX;
        let max_length_name = "a".repeat(max_len);
        assert!(PolicyUploader::validate_policy_name(&max_length_name).is_ok());

        // Test all valid special characters
        assert!(PolicyUploader::validate_policy_name("Test+=,.@_-123").is_ok());
    }

    #[test]
    fn test_generate_policy_name_with_invalid_custom_name() {
        let existing = Vec::new();

        // Should fail with invalid custom name
        let result = PolicyUploader::generate_policy_name(&existing, Some("Invalid Name"));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UploaderError::InvalidPolicyName(_, _)
        ));
    }

    #[test]
    fn test_batch_upload_response_empty() {
        let response = BatchUploadResponse {
            successful: Vec::new(),
            failed: Vec::new(),
        };

        assert_eq!(response.successful.len(), 0);
        assert_eq!(response.failed.len(), 0);
    }

    #[test]
    fn test_batch_upload_response_with_data() {
        let upload_response = UploadResponse {
            policy_name: "TestPolicy1".to_string(),
            policy_arn: "arn:aws:iam::123456789012:policy/TestPolicy1".to_string(),
            policy_id: "ANPAI23HZ27SI6FQMGNQ2".to_string(),
        };

        let response = BatchUploadResponse {
            successful: vec![upload_response],
            failed: vec![(0, "Test error message".to_string())],
        };

        assert_eq!(response.successful.len(), 1);
        assert_eq!(response.failed.len(), 1);
        assert_eq!(response.successful[0].policy_name, "TestPolicy1");
        assert_eq!(response.failed[0].0, 0);
        assert_eq!(response.failed[0].1, "Test error message");

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"Successful\":"));
        assert!(json.contains("\"Failed\":"));
        assert!(json.contains("\"PolicyName\":\"TestPolicy1\""));
        assert!(json.contains("\"PolicyArn\":"));
        assert!(json.contains("\"PolicyId\":"));
    }
}
