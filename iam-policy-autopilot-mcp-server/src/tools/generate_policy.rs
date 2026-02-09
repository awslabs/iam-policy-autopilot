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

    #[schemars(
        description = "If set to true, the tool will return detailed explanations for why each permission was added. Defaults to false."
    )]
    pub explain: Option<bool>,
}

// Output struct for the generated IAM policy
#[derive(Debug, Serialize, JsonSchema, Eq, PartialEq)]
#[schemars(description = "Output containing the generated IAM policies with type information.")]
#[serde(rename_all = "PascalCase")]
pub struct GeneratePoliciesOutput {
    #[schemars(description = "List of policies with their associated types.")]
    pub policies: Vec<String>,

    #[schemars(
        description = "Detailed explanations for why specific actions were included in the policy."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanations: Option<String>,
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

    // If input.explain is true, ask for ALL explanations ("*"). If false/missing, ask for None.
    let explain_filters = if input.explain.unwrap_or(false) {
        Some(vec!["*".to_string()])
    } else {
        None
    };

    let result = api::generate_policies(&GeneratePolicyConfig {
        individual_policies: false,
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: input.source_files.into_iter().map(|f| f.into()).collect(),
            // Maybe we should let the llm figure out the language
            language: None,
            service_hints,
        },
        aws_context: AwsContext::new(region, account)?,
        minimize_policy_size: false,

        // true by default, if we want to allow the user to change it we should
        // accept it as part of the cli input when starting the mcp server
        disable_file_system_cache: true,

        explain_filters,
    })
    .await?;

    let policies = result
        .policies
        .into_iter()
        .map(|policy| serde_json::to_string(&policy.policy).context("Failed to serialize policy"))
        .collect::<Result<Vec<String>, Error>>()?;

    // Extract and serialize explanations if they exist
    let explanations = result
        .explanations
        .map(|e| serde_json::to_string(&e).context("Failed to serialize explanations"))
        .transpose()?;

    Ok(GeneratePoliciesOutput {
        policies,
        explanations,
    })
}

// Mock the api call
#[cfg(test)]
mod api {
    use anyhow::Result;
    use iam_policy_autopilot_policy_generation::api::model::{
        GeneratePoliciesResult, GeneratePolicyConfig,
    };
    use std::sync::Mutex;
    use std::sync::OnceLock;

    // Simple mock storage - Mutex is needed for static variables even with serial tests
    static MOCK_RETURN_VALUE: OnceLock<Mutex<Option<Result<GeneratePoliciesResult>>>> =
        OnceLock::new();

    pub async fn generate_policies(
        _config: &GeneratePolicyConfig,
    ) -> Result<GeneratePoliciesResult> {
        let mutex = MOCK_RETURN_VALUE.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        guard
            .take()
            .expect("Mock return value not set. Call set_mock_return() first.")
    }

    pub fn set_mock_return(value: Result<GeneratePoliciesResult>) {
        let mutex = MOCK_RETURN_VALUE.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        *guard = Some(value);
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
    // Need to import Explanations to construct the mock data
    use iam_policy_autopilot_policy_generation::enrichment::{Explanation, Explanations};
    use std::collections::BTreeMap;

    use anyhow::anyhow;

    #[tokio::test]
    async fn test_generate_application_policies() {
        // Tests are run under target/deps
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: None,
            explain: None,
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
            explain: None,
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
            explain: None,
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
            explanations: None,
        };

        let json = serde_json::to_string(&output).unwrap();

        assert!(json.contains("\"Policies\":"));
        assert!(json.contains("[\"{"));
        assert!(!json.contains("\"Explanations\":"));
    }

    #[tokio::test]
    async fn test_generate_application_policies_with_service_hints() {
        let input = GeneratePoliciesInput {
            source_files: vec!["path/to/source/file".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: Some(vec!["s3".to_string(), "dynamodb".to_string()]),
            explain: None,
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

    #[tokio::test]
    async fn test_generate_application_policies_with_explanations() {
        use iam_policy_autopilot_policy_generation::{Operation, OperationSource, Reason};
        use std::sync::Arc;

        // 1. INPUT
        let input = GeneratePoliciesInput {
            source_files: vec!["app.py".to_string()],
            region: Some("us-east-1".to_string()),
            account: Some("123456789012".to_string()),
            service_hints: None,
            explain: Some(true),
        };

        // 2. SETUP POLICY
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

        // 3. MOCK DATA (Realistic Structure)
        // Create a 'Provided' operation (simulating a manual or inferred permission)
        let operation = Arc::new(Operation::new(
            "s3".to_string(),
            "ListBucket".to_string(),
            OperationSource::Provided,
        ));

        let reason = Reason::new(vec![operation]);

        let mut explanation_map = BTreeMap::new();
        explanation_map.insert(
            "s3:ListBucket".to_string(),
            Explanation {
                reasons: vec![reason],
            },
        );

        let explanations = Explanations {
            explanation_for_action: explanation_map,
            documentation: vec![
                "https://docs.aws.amazon.com/IAM/latest/UserGuide/access_forward_access_sessions.html",
            ],
        };

        // 4. INJECT MOCK
        api::set_mock_return(Ok(GeneratePoliciesResult {
            policies: vec![policy],
            explanations: Some(explanations),
        }));

        // 5. EXECUTE
        let result = generate_application_policies(input).await;

        // 6. VERIFY
        assert!(result.is_ok());
        let output = result.unwrap();

        assert!(output.explanations.is_some());
        let explanation_str = output.explanations.unwrap();

        println!("Final Output: {}", explanation_str);

        // Verify Deep Serialization
        assert!(explanation_str.contains("s3:ListBucket"));
        assert!(explanation_str.contains("Provided")); // Proves OperationSource serialized
    }
}