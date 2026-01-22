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
    use std::sync::Mutex;
    use std::sync::OnceLock;

    // Simple mock storage - Mutex is needed for static variables even with serial tests
    static MOCK_PLAN_RETURN: OnceLock<Mutex<Option<Result<PlanResult>>>> = OnceLock::new();
    static MOCK_APPLY_RETURN: OnceLock<Mutex<Option<Result<ApplyResult>>>> = OnceLock::new();

    pub async fn plan(_error_message: &str) -> Result<PlanResult> {
        let mutex = MOCK_PLAN_RETURN.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        guard
            .take()
            .expect("Mock plan return value not set. Call set_mock_plan_return() first.")
    }

    pub async fn apply(_plan: &PlanResult, _options: ApplyOptions) -> Result<ApplyResult> {
        let mutex = MOCK_APPLY_RETURN.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        guard
            .take()
            .expect("Mock apply return value not set. Call set_mock_apply_return() first.")
    }

    pub fn set_mock_plan_return(value: Result<PlanResult>) {
        let mutex = MOCK_PLAN_RETURN.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        *guard = Some(value);
    }

    pub fn set_mock_apply_return(value: Result<ApplyResult>) {
        let mutex = MOCK_APPLY_RETURN.get_or_init(|| Mutex::new(None));
        let mut guard = mutex.lock().unwrap();
        *guard = Some(value);
    }
}

pub(crate) use fix_access_denied::*;
