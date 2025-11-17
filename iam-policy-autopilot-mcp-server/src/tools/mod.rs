mod fix_access_denied;
mod generate_policy;
mod generate_policy_for_access_denied;

pub(crate) use generate_policy::{
    generate_application_policies, GeneratePoliciesInput, GeneratePoliciesOutput,
};
pub(crate) use generate_policy_for_access_denied::{
    generate_policy_for_access_denied, GeneratePolicyForAccessDeniedInput,
    GeneratePolicyForAccessDeniedOutput,
};

/// Wrapper for iam_policy_autopilot_policy_generation::commands::IamPolicyAutopilotService
/// we mock this implementation with #[cfg(test)] to help with unit testing
#[cfg(not(test))]
pub(crate) mod policy_autopilot {
    use anyhow::{Context, Result};
    use iam_policy_autopilot_access_denied::{
        commands::IamPolicyAutopilotService, ApplyOptions, ApplyResult, PlanResult,
    };

    pub async fn plan(error_message: &str) -> Result<PlanResult> {
        let policy_service = IamPolicyAutopilotService::new()
            .await
            .context("Failed to initialize IamPolicyAutopilot")?;
        policy_service
            .plan(error_message)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn apply(plan: &PlanResult, options: ApplyOptions) -> Result<ApplyResult> {
        let policy_service = IamPolicyAutopilotService::new()
            .await
            .context("Failed to initialize IamPolicyAutopilot")?;
        policy_service
            .apply(plan, options)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod policy_autopilot {
    use anyhow::Result;
    use iam_policy_autopilot_access_denied::{ApplyOptions, ApplyResult, PlanResult};

    pub static mut MOCK_PLAN_RETURN: Option<Result<PlanResult>> = None;
    pub static mut MOCK_APPLY_RETURN: Option<Result<ApplyResult>> = None;

    pub async fn plan(_error_message: &str) -> Result<PlanResult> {
        #[allow(static_mut_refs)]
        unsafe {
            MOCK_PLAN_RETURN.take().unwrap()
        }
    }

    pub async fn apply(_plan: &PlanResult, _options: ApplyOptions) -> Result<ApplyResult> {
        #[allow(static_mut_refs)]
        unsafe {
            MOCK_APPLY_RETURN.take().unwrap()
        }
    }

    pub fn set_mock_plan_return(value: Result<PlanResult>) {
        unsafe { MOCK_PLAN_RETURN = Some(value) }
    }

    pub fn set_mock_apply_return(value: Result<ApplyResult>) {
        unsafe { MOCK_APPLY_RETURN = Some(value) }
    }
}

pub(crate) use fix_access_denied::*;
