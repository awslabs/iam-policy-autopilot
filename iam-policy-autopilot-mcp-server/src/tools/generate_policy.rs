use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePolicyConfig, ServiceHints,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(not(test))]
mod api {
    pub use iam_policy_autopilot_policy_generation::api::generate_policies;
}

// Input struct matching the updated schema
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Input for generating IAM policies from source code.")]
pub struct GeneratePoliciesInput {
    #[schemars(description = "Absolute paths to source files to generate IAM Policies for")]
    pub source_files: Vec<String>,

    #[schemars(description = "AWS Region")]
    pub region: Option<String>,

    #[schemars(description = "AWS Account Id")]
    pub account: Option<String>,

    #[schemars(
        description = "List of AWS service names to filter SDK calls by (e.g., ['s3', 'dynamodb']). When provided, the result of source code analysis will be restricted to the provided services. The generated policy may still contain actions from a service not provided as a hint, if IAM Policy Autopilot determines that the action may be needed for the SDK call."
    )]
    pub service_hints: Option<Vec<String>>,
}

// Output struct for the generated IAM policy
#[derive(Debug, Serialize, JsonSchema, Eq, PartialEq)]
#[schemars(description = "Output containing the generated IAM policies with type information.")]
#[serde(rename_all = "PascalCase")]
pub struct GeneratePoliciesOutput {
    #[schemars(description = "List of policies with their associated types.")]
    pub policies: Vec<String>,
}

pub async fn generate_application_policies(
    input: GeneratePoliciesInput,
) -> Result<GeneratePoliciesOutput, Error> {
    let region = input.region.unwrap_or("*".to_string());
    let account = input.account.unwrap_or("*".to_string());

    // Convert service_hints from Vec<String> to ServiceHints if provided
    let service_hints = input.service_hints.map(|hints| ServiceHints {
        service_names: hints,
    });

    let result = api::generate_policies(&GeneratePolicyConfig {
        individual_policies: false,
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: input.source_files.into_iter().map(|f| f.into()).collect(),
            // Maybe we should let the llm figure out the language
            language: None,
            service_hints,
        },
        aws_context: AwsContext::new(region, account),
        minimize_policy_size: false,

        // true by default, if we want to allow the user to change it we should
        // accept it as part of the cli input when starting the mcp server
        disable_file_system_cache: true,
        // No explanations for MCP server by default
        explain_filters: None,
    })
    .await?;

    let policies = result
        .policies
        .into_iter()
        .map(|policy| serde_json::to_string(&policy.policy).context("Failed to serialize policy"))
        .collect::<Result<Vec<String>, Error>>()?;

    Ok(GeneratePoliciesOutput { policies })
}

// Mock the api call
#[cfg(test)]
mod api {
    use anyhow::Result;
    use iam_policy_autopilot_policy_generation::api::model::{
        GeneratePoliciesResult, GeneratePolicyConfig,
    };

    // Static mutable return value
    pub static mut MOCK_RETURN_VALUE: Option<Result<GeneratePoliciesResult>> = None;

    pub async fn generate_policies(
        _config: &GeneratePolicyConfig,
    ) -> Result<GeneratePoliciesResult> {
        #[allow(static_mut_refs)]
        unsafe {
            MOCK_RETURN_VALUE.take().unwrap()
        }
    }

    pub fn set_mock_return(value: Result<GeneratePoliciesResult>) {
        unsafe { MOCK_RETURN_VALUE = Some(value) }
    }
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use std::vec;

    use super::*;
    use iam_policy_autopilot_policy_generation::{
        api::model::GeneratePoliciesResult, IamPolicy, PolicyType, PolicyWithMetadata, Statement,
    };

    use anyhow::anyhow;

    #[tokio::test]
    async fn test_generate_application_policies() {
        // Tests are run under target/deps
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: None,
        };

        let expected_output = include_str!("../testdata/test_generate_application_policy");

        // deserialize from json into IamPolicy
        let mut iam_policy = IamPolicy::new();
        iam_policy.add_statement(Statement::new(
            iam_policy_autopilot_policy_generation::Effect::Allow,
            vec!["s3:ListBucket".to_string()],
            vec!["resource".to_string()],
        ));

        let policy = PolicyWithMetadata {
            policy: iam_policy,
            policy_type: PolicyType::Identity,
        };

        use iam_policy_autopilot_policy_generation::api::model::GeneratePoliciesResult;

        api::set_mock_return(Ok(GeneratePoliciesResult {
            policies: vec![policy],
            explanations: None,
        }));
        let result = generate_application_policies(input).await;

        println!("{result:?}");
        assert!(result.is_ok());

        let output = serde_json::to_string_pretty(&result.unwrap()).unwrap();

        assert_eq!(output, expected_output);
    }

    #[tokio::test]
    async fn test_generate_application_policies_with_invalid_input() {
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: None,
        };

        api::set_mock_return(Err(anyhow!("Failed to generate policies")));
        let result = generate_application_policies(input).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_generate_policies_input_serialization() {
        let input = GeneratePoliciesInput {
            source_files: vec!["/path/to/file.py".to_string()],
            region: Some("us-west-2".to_string()),
            account: Some("987654321098".to_string()),
            service_hints: None,
        };

        let json = serde_json::to_string(&input).unwrap();

        assert!(json.contains("\"SourceFiles\":"));
        assert!(json.contains("\"Region\":\"us-west-2\""));
        assert!(json.contains("\"Account\":\"987654321098\""));
    }

    #[test]
    fn test_generate_policies_output_serialization() {
        let output = GeneratePoliciesOutput {
            policies: vec![
                "{\"Version\":\"2012-10-17\"}".to_string(),
                "{\"Version\":\"2012-10-17\"}".to_string(),
            ],
        };

        let json = serde_json::to_string(&output).unwrap();

        assert!(json.contains("\"Policies\":"));
        assert!(json.contains("[\"{"));
    }

    #[tokio::test]
    async fn test_generate_application_policies_with_service_hints() {
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: Some(vec!["s3".to_string(), "dynamodb".to_string()]),
        };

        let expected_output = include_str!("../testdata/test_generate_application_policy");

        let mut iam_policy = IamPolicy::new();
        iam_policy.add_statement(Statement::new(
            iam_policy_autopilot_policy_generation::Effect::Allow,
            vec!["s3:ListBucket".to_string()],
            vec!["resource".to_string()],
        ));

        let policy = PolicyWithMetadata {
            policy: iam_policy,
            policy_type: PolicyType::Identity,
        };

        api::set_mock_return(Ok(GeneratePoliciesResult {
            policies: vec![policy],
            explanations: None,
        }));
        let result = generate_application_policies(input).await;

        assert!(result.is_ok());

        let output = serde_json::to_string_pretty(&result.unwrap()).unwrap();
        assert_eq!(output, expected_output);
    }
}
