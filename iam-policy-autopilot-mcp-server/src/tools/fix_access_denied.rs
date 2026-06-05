use crate::tools::policy_autopilot;
use anyhow::Error;
use anyhow::{bail, Context};
use iam_policy_autopilot_access_denied::{ApplyOptions, ApplyResult};
use log::error;
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
#[derive(
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    iam_policy_autopilot_common::telemetry::TelemetryEventDerive,
)]
#[serde(rename_all = "PascalCase")]
#[schemars(description = "Input for fixing access denied issues")]
#[telemetry(command = "mcp-tool-fix-access-denied")]
pub struct FixAccessDeniedInput {
    #[schemars(
        description = "The original access denied error message to extract principal information"
    )]
    #[telemetry(presence)]
    pub error_message: String,

    #[schemars(
        description = "Optional resource ARN override. When provided, this resource ARN is used in the policy statement instead of the one derived from the error message. Use this to grant narrower access (e.g., a specific S3 object key) or broader access (e.g., a bucket wildcard for object-level APIs)."
    )]
    #[telemetry(presence)]
    pub resource_override: Option<String>,
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
struct UserConfirmation {
    #[schemars(description = "Check the box and accept to confirm")]
    confirmed: bool,
}

// Mark as safe for elicitation
elicit_safe!(UserConfirmation);

async fn elicit_confirmation(
    context: &RequestContext<RoleServer>,
    message: String,
) -> Result<bool, Error> {
    match context.peer.elicit::<UserConfirmation>(message).await {
        Ok(Some(UserConfirmation { confirmed })) => Ok(confirmed),
        Ok(None) | Err(ElicitationError::UserDeclined | ElicitationError::UserCancelled) => {
            Ok(false)
        }
        Err(ElicitationError::CapabilityNotSupported) => {
            bail!("MCP client does not support elicitation; cannot apply policy without user confirmation.");
        }
        Err(e) => {
            error!("Elicitation error {e:#?}");
            bail!("MCP user elicitation failed when applying policies.");
        }
    }
}

pub async fn fix_access_denied(
    context: RequestContext<RoleServer>,
    input: FixAccessDeniedInput,
) -> Result<FixAccessDeniedOutput, Error> {
    let plan = policy_autopilot::plan(&input.error_message, input.resource_override)
        .await
        .context("Failed to parse access denied error message")?;

    let policy_pretty = plan
        .to_policy_json_pretty()
        .context("Failed to serialize policy")?;

    let message = format!(
        "Are you sure you want to apply the following policy to {}?\n\n{}",
        plan.principal_arn(),
        policy_pretty
    );

    let confirmed = elicit_confirmation(&context, message).await?;

    if !confirmed {
        return Ok(FixAccessDeniedOutput {
            policy: policy_pretty,
            fix_result: None,
        });
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
        policy: policy_pretty,
        fix_result: Some(FixResult::from(apply)),
    })
}

#[cfg(test)]
#[serial_test::serial]
mod tests {
    use super::*;
    use anyhow::anyhow;

    // Note: These tests focus on the service layer mocking.
    // Full integration tests with RequestContext would require more complex setup.

    #[tokio::test]
    async fn test_service_plan_success() {
        let plan = create_test_plan();
        policy_autopilot::set_mock_plan_return(Ok(plan.clone()));

        let result = policy_autopilot::plan("test error message", None).await;
        assert!(result.is_ok());

        let returned_plan = result.unwrap();
        assert_eq!(returned_plan.action(), "s3:GetObject");
    }

    #[tokio::test]
    async fn test_service_plan_failure() {
        policy_autopilot::set_mock_plan_return(Err(anyhow!("Failed to generate policies")));

        let result = policy_autopilot::plan("invalid error message", None).await;
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
            error_message: "User: arn:aws:iam::123456789012:user/test is not authorized"
                .to_string(),
            resource_override: Some("arn:aws:s3:::my-bucket/specific-key".to_string()),
        };

        let json = serde_json::to_string(&input).unwrap();

        assert!(json.contains("\"ErrorMessage\":"));
        assert!(json.contains("\"ResourceOverride\":"));
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
        use iam_policy_autopilot_access_denied::{DenialType, ParsedDenial, PlanResult};

        PlanResult::new(
            ParsedDenial::new(
                "arn:aws:iam::123456789012:user/testuser".to_string(),
                "s3:GetObject".to_string(),
                "arn:aws:s3:::my-bucket/my-key".to_string(),
                DenialType::ImplicitIdentity,
            ),
            None,
        )
    }
}
