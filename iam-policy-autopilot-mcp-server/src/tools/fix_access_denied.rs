use crate::tools::policy_autopilot;
use anyhow::Error;
use anyhow::{bail, Context};
use iam_policy_autopilot_access_denied::{ApplyOptions, ApplyResult};
use log::{debug, error, warn};
use rmcp::{
    elicit_safe,
    service::{ElicitationError, RequestContext},
    RoleServer,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// It's the exact same struct as ApplyResult
// But we temporarily create a copy to derive some traits
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Result of applying an IAM policy fix")]
pub struct FixResult {
    #[schemars(description = "Whether the policy was successfully applied")]
    pub success: bool,
    #[schemars(description = "Name of the IAM policy that was created or updated")]
    pub policy_name: String,
    #[schemars(description = "Type of IAM principal (User, Role, etc)")]
    pub principal_kind: String,
    #[schemars(description = "Name of the IAM principal")]
    pub principal_name: String,
    #[schemars(description = "Whether a new policy was created")]
    pub is_new_policy: bool,
    #[schemars(description = "Number of policy statements in the applied policy")]
    pub statement_count: usize,
    #[schemars(description = "Error message if the policy application failed")]
    pub error: Option<String>,
}

impl From<ApplyResult> for FixResult {
    fn from(value: ApplyResult) -> Self {
        Self {
            success: value.success,
            policy_name: value.policy_name,
            principal_kind: value.principal_kind,
            principal_name: value.principal_name,
            is_new_policy: value.is_new_policy,
            statement_count: value.statement_count,
            error: value.error,
        }
    }
}

// Input struct matching the updated schema
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Input for fixing access denied issues")]
pub struct FixAccessDeniedInput {
    #[schemars(
        description = "The IAM Policy JSON to fix access denied that was generated through generate_policy_for_access_denied tool"
    )]
    pub access_denied_fix_policy: String,
    #[schemars(
        description = "The original access denied error message to extract principal information"
    )]
    pub error_message: String,
}

// Output struct for the generated IAM policy
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Output containing the result for fixing access denied issue")]
pub struct FixAccessDeniedOutput {
    #[schemars(description = "Applied policy", with = "FixResult")]
    pub fix_result: Option<FixResult>,

    #[schemars(
        description = "IAM Policy that was attached to the principal for access denied fix"
    )]
    pub policy: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Confirmation from the user")]
struct UserConfirmation(bool);

// Mark as safe for elicitation
elicit_safe!(UserConfirmation);

pub async fn fix_access_denied(
    context: RequestContext<RoleServer>,
    input: FixAccessDeniedInput,
) -> Result<FixAccessDeniedOutput, Error> {
    // Parse the error message to extract principal ARN using the plan method
    let plan = policy_autopilot::plan(&input.error_message)
        .await
        .context("Failed to generate access denied fix policy from error message")?;

    let policy_str = serde_json::to_string(&plan.policy).context("Failed to serialize policy")?;

    debug!("Generated policy: {policy_str}");

    // Get confirmation from the user
    let elicit_result = context
        .peer
        .elicit::<UserConfirmation>(format!(
            "Are you sure you want to apply the following policy to {}? (yes/no):\n\n{}",
            plan.diagnosis.principal_arn, input.access_denied_fix_policy
        ))
        .await;

    match elicit_result {
        Err(ElicitationError::CapabilityNotSupported) => {
            warn!("Elicitation capability not supported");
            // Continue with the apply
        }
        Err(e) => {
            error!("Elicitation error {e:#?}");
            bail!("MCP user elicitation failed when applying policies.");
        }
        Ok(Some(UserConfirmation(true))) => { /* no-op */ }
        Ok(Some(UserConfirmation(false)) | None) => {
            return Ok(FixAccessDeniedOutput {
                policy: policy_str,
                fix_result: None,
            });
        }
    }

    let apply = policy_autopilot::apply(
        &plan,
        ApplyOptions {
            skip_confirmation: true,
            skip_tty_check: true,
        },
    )
    .await
    .context("Failed to apply access denied fix")?;

    Ok(FixAccessDeniedOutput {
        policy: policy_str,
        fix_result: Some(FixResult::from(apply)),
    })
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use iam_policy_autopilot_access_denied::aws::policy_naming::POLICY_PREFIX;

    // Note: These tests focus on the service layer mocking.
    // Full integration tests with RequestContext would require more complex setup.

    #[tokio::test]
    async fn test_service_plan_success() {
        let plan = create_test_plan();
        policy_autopilot::set_mock_plan_return(Ok(plan.clone()));

        let result = policy_autopilot::plan("test error message").await;
        assert!(result.is_ok());

        let returned_plan = result.unwrap();
        assert_eq!(returned_plan.actions, vec!["s3:GetObject".to_string()]);
    }

    #[tokio::test]
    async fn test_service_plan_failure() {
        policy_autopilot::set_mock_plan_return(Err(anyhow!("Failed to generate policies")));

        let result = policy_autopilot::plan("invalid error message").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to generate policies"));
    }

    #[tokio::test]
    async fn test_service_apply_success() {
        let plan = create_test_plan();
        let apply_result = ApplyResult {
            success: true,
            policy_name: "test-policy".to_string(),
            principal_kind: "User".to_string(),
            principal_name: "test-user".to_string(),
            is_new_policy: true,
            statement_count: 1,
            error: None,
        };

        policy_autopilot::set_mock_apply_return(Ok(apply_result.clone()));

        let result = policy_autopilot::apply(
            &plan,
            ApplyOptions {
                skip_confirmation: true,
                skip_tty_check: true,
            },
        )
        .await;

        assert!(result.is_ok());
        let returned_result = result.unwrap();
        assert!(returned_result.success);
        assert_eq!(returned_result.policy_name, "test-policy");
    }

    #[tokio::test]
    async fn test_service_apply_failure() {
        let plan = create_test_plan();
        policy_autopilot::set_mock_apply_return(Err(anyhow!("Failed to apply policy")));

        let result = policy_autopilot::apply(
            &plan,
            ApplyOptions {
                skip_confirmation: true,
                skip_tty_check: true,
            },
        )
        .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to apply policy"));
    }

    #[tokio::test]
    async fn test_fix_result_conversion() {
        let apply_result = ApplyResult {
            success: true,
            policy_name: "test-policy".to_string(),
            principal_kind: "User".to_string(),
            principal_name: "test-user".to_string(),
            is_new_policy: false,
            statement_count: 2,
            error: Some("test error".to_string()),
        };

        let fix_result = FixResult::from(apply_result.clone());

        assert_eq!(fix_result.success, apply_result.success);
        assert_eq!(fix_result.policy_name, apply_result.policy_name);
        assert_eq!(fix_result.principal_kind, apply_result.principal_kind);
        assert_eq!(fix_result.principal_name, apply_result.principal_name);
        assert_eq!(fix_result.is_new_policy, apply_result.is_new_policy);
        assert_eq!(fix_result.statement_count, apply_result.statement_count);
        assert_eq!(fix_result.error, apply_result.error);
    }

    #[test]
    fn test_fix_access_denied_input_serialization() {
        let input = FixAccessDeniedInput {
            access_denied_fix_policy: "{\"Version\":\"2012-10-17\"}".to_string(),
            error_message: "User: arn:aws:iam::123456789012:user/test is not authorized"
                .to_string(),
        };

        let json = serde_json::to_string(&input).unwrap();

        assert!(json.contains("\"AccessDeniedFixPolicy\":"));
        assert!(json.contains("\"ErrorMessage\":"));
    }

    #[test]
    fn test_fix_access_denied_output_serialization() {
        let fix_result = FixResult {
            success: true,
            policy_name: "TestPolicy".to_string(),
            principal_kind: "User".to_string(),
            principal_name: "testuser".to_string(),
            is_new_policy: true,
            statement_count: 1,
            error: None,
        };

        let output = FixAccessDeniedOutput {
            fix_result: Some(fix_result),
            policy: "{\"Version\":\"2012-10-17\"}".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();

        assert!(json.contains("\"FixResult\":"));
        assert!(json.contains("\"Policy\":"));
        assert!(json.contains("\"Success\":true"));
        assert!(json.contains("\"PolicyName\":\"TestPolicy\""));
        assert!(json.contains("\"PrincipalKind\":\"User\""));
        assert!(json.contains("\"IsNewPolicy\":true"));
        assert!(json.contains("\"StatementCount\":1"));
    }

    // Test helper function to create a minimal plan for testing
    fn create_test_plan() -> iam_policy_autopilot_access_denied::PlanResult {
        use iam_policy_autopilot_access_denied::{
            DenialType, ParsedDenial, PlanResult, PolicyDocument,
        };

        PlanResult {
            diagnosis: ParsedDenial::new(
                "arn:aws:iam::123456789012:user/testuser".to_string(),
                "s3:GetObject".to_string(),
                "arn:aws:s3:::my-bucket/my-key".to_string(),
                DenialType::ImplicitIdentity,
            ),
            actions: vec!["s3:GetObject".to_string()],
            policy: PolicyDocument {
                id: Some(POLICY_PREFIX.to_string()),
                version: "2012-10-17".to_string(),
                statement: vec![],
            },
        }
    }
}
