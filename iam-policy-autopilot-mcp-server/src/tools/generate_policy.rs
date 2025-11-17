use anyhow::Context;
use anyhow::Error;
use anyhow::Result;
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePolicyConfig,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(not(test))]
mod api {
    pub use iam_policy_autopilot_policy_generation::api::generate_policies;
}

// Input struct matching the updated schema
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Input for generating IAM policies from source code.")]
pub struct GeneratePoliciesInput {
    #[schemars(description = "Absolute paths to source files to generate IAM Policies for")]
    pub source_files: Vec<String>,

    #[schemars(description = "AWS Region")]
    pub region: Option<String>,

    #[schemars(description = "AWS Account Id")]
    pub account: Option<String>,
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

    let (policies, _) = api::generate_policies(&GeneratePolicyConfig {
        individual_policies: false,
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: input.source_files.into_iter().map(|f| f.into()).collect(),
            // Maybe we should let the llm figure out the language
            language: None,
        },
        aws_context: AwsContext::new(region, account),
        generate_action_mappings: false,
        minimize_policy_size: false,

        // true by default, if we want to allow the user to change it we should
        // accept it as part of the cli input when starting the mcp server
        disable_file_system_cache: true,
    })
    .await?;

    let policies = policies
        .into_iter()
        .map(|policy| serde_json::to_string(&policy.policy).context("Failed to serialize policy"))
        .collect::<Result<Vec<String>, Error>>()?;

    Ok(GeneratePoliciesOutput { policies })
}

// Mock the api call
#[cfg(test)]
mod api {
    use anyhow::Result;
    use iam_policy_autopilot_policy_generation::{
        api::model::GeneratePolicyConfig, policy_generation::PolicyWithMetadata,
        MethodActionMapping,
    };

    // Static mutable return value
    pub static mut MOCK_RETURN_VALUE: Option<
        Result<(Vec<PolicyWithMetadata>, Vec<MethodActionMapping>)>,
    > = None;

    pub async fn generate_policies(
        _config: &GeneratePolicyConfig,
    ) -> Result<(Vec<PolicyWithMetadata>, Vec<MethodActionMapping>)> {
        #[allow(static_mut_refs)]
        unsafe {
            MOCK_RETURN_VALUE.take().unwrap()
        }
    }

    pub fn set_mock_return(value: Result<(Vec<PolicyWithMetadata>, Vec<MethodActionMapping>)>) {
        unsafe { MOCK_RETURN_VALUE = Some(value) }
    }
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use std::vec;

    use super::*;
    use iam_policy_autopilot_policy_generation::{
        IamPolicy, PolicyType, PolicyWithMetadata, Statement,
    };

    use anyhow::anyhow;

    #[tokio::test]
    async fn test_generate_application_policies() {
        // Tests are run under target/deps
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
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

        api::set_mock_return(Ok((vec![policy], vec![])));
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
        };

        api::set_mock_return(Err(anyhow!("Failed to generate policies")));
        let result = generate_application_policies(input).await;

        assert!(result.is_err());
    }
}
