use crate::tools::policy_autopilot;
use anyhow::Context;
use anyhow::Error;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Input struct matching the updated schema
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Input for generating policies for AccessDenied exceptions")]
pub struct GeneratePolicyForAccessDeniedInput {
    #[schemars(description = "AccessDenied exception message")]
    pub error_message: String,
}

// Output struct for the generated IAM policy
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Output containing the generated IAM policy.")]
pub struct GeneratePolicyForAccessDeniedOutput {
    #[schemars(description = "Proposed policy for AccessDenied fix")]
    pub policy: String,
}

pub async fn generate_policy_for_access_denied(
    input: GeneratePolicyForAccessDeniedInput,
) -> Result<GeneratePolicyForAccessDeniedOutput, Error> {
    let plan = policy_autopilot::plan(&input.error_message)
        .await
        .context("Failed to generate policies")?;

    let policy_str = serde_json::to_string(&plan.policy).context("Failed to serialize policy")?;

    Ok(GeneratePolicyForAccessDeniedOutput { policy: policy_str })
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use iam_policy_autopilot_access_denied::{
        DenialType, ParsedDenial, PlanResult, PolicyDocument,
    };

    #[tokio::test]
    async fn test_generate_policy_for_access_denied() {
        let input = GeneratePolicyForAccessDeniedInput {
            error_message: "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key".to_string(),
        };

        let sample_policy = PolicyDocument {
            version: "2012-10-17".to_string(),
            statement: vec![],
        };

        let plan = PlanResult {
            diagnosis: ParsedDenial::new(
                "arn:aws:iam::123456789012:user/testuser".to_string(),
                "s3:GetObject".to_string(),
                "arn:aws:s3:::my-bucket/my-key".to_string(),
                DenialType::ImplicitIdentity,
            ),
            actions: vec!["s3:GetObject".to_string()],
            policy: sample_policy.clone(),
        };

        policy_autopilot::set_mock_plan_return(Ok(plan));
        let result = generate_policy_for_access_denied(input).await;

        assert!(result.is_ok());
        let output = result.unwrap();

        let expected_policy = serde_json::to_string(&sample_policy).unwrap();
        assert_eq!(output.policy, expected_policy);
    }

    #[tokio::test]
    async fn test_generate_policy_for_access_denied_with_error() {
        let input = GeneratePolicyForAccessDeniedInput {
            error_message: "Invalid error message".to_string(),
        };

        policy_autopilot::set_mock_plan_return(Err(anyhow!("Failed to generate policies")));
        let result = generate_policy_for_access_denied(input).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to generate policies"));
    }

    #[tokio::test]
    async fn test_generate_policy_for_access_denied_serialization_error() {
        let input = GeneratePolicyForAccessDeniedInput {
            error_message: "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key".to_string(),
        };

        let sample_policy = PolicyDocument {
            version: "2012-10-17".to_string(),
            statement: vec![],
        };

        let plan = PlanResult {
            diagnosis: ParsedDenial::new(
                "arn:aws:iam::123456789012:user/testuser".to_string(),
                "s3:GetObject".to_string(),
                "arn:aws:s3:::my-bucket/my-key".to_string(),
                DenialType::ImplicitIdentity,
            ),
            actions: vec!["s3:GetObject".to_string()],
            policy: sample_policy,
        };

        policy_autopilot::set_mock_plan_return(Ok(plan));
        let result = generate_policy_for_access_denied(input).await;

        // Should succeed since PolicyDocument is serializable
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_policy_for_access_denied_input_serialization() {
        let input = GeneratePolicyForAccessDeniedInput {
            error_message: "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:GetObject".to_string(),
        };

        let json = serde_json::to_string(&input).unwrap();

        assert!(json.contains("\"ErrorMessage\":"));
    }

    #[test]
    fn test_generate_policy_for_access_denied_output_serialization() {
        let output = GeneratePolicyForAccessDeniedOutput {
            policy: "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();

        assert!(json.contains("\"Policy\":"));
        assert!(json.contains("Version"));
    }
}
