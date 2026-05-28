mod fix_access_denied;
mod generate_policy;
mod generate_policy_for_access_denied;

pub(crate) use generate_policy::generate_application_policies;
pub(crate) use generate_policy::{GeneratePoliciesInput, GeneratePoliciesOutput};
pub(crate) use generate_policy_for_access_denied::generate_policy_for_access_denied;
pub(crate) use generate_policy_for_access_denied::{
    GeneratePolicyForAccessDeniedInput, GeneratePolicyForAccessDeniedOutput,
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

pub(crate) use fix_access_denied::fix_access_denied;
pub(crate) use fix_access_denied::{FixAccessDeniedInput, FixAccessDeniedOutput};

#[cfg(test)]
mod telemetry_doc_sync_tests {
    use super::*;
    use iam_policy_autopilot_common::telemetry::{
        parse_doc_fields, TelemetryFieldInfo, ToTelemetryEvent,
    };
    use std::collections::BTreeMap;

    fn collect_mcp_telemetry_fields() -> Vec<TelemetryFieldInfo> {
        let mut fields = Vec::new();
        fields.extend(GeneratePoliciesInput::telemetry_fields());
        fields.extend(GeneratePolicyForAccessDeniedInput::telemetry_fields());
        fields.extend(FixAccessDeniedInput::telemetry_fields());
        fields
    }

    /// Verify that every MCP telemetry field is documented in TELEMETRY.md,
    /// and vice-versa.
    #[test]
    fn test_mcp_telemetry_fields_documented_in_telemetry_md() {
        let fields = collect_mcp_telemetry_fields();

        let telemetry_md =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../TELEMETRY.md"))
                .expect("Failed to read TELEMETRY.md");

        // Direction 1 — code → doc: every code field is documented
        let mut by_command: BTreeMap<String, Vec<&TelemetryFieldInfo>> = BTreeMap::new();
        for field in &fields {
            by_command
                .entry(field.command.clone())
                .or_default()
                .push(field);
        }

        for (command, cmd_fields) in &by_command {
            let header = format!("### MCP: `{command}`");
            assert!(
                telemetry_md.contains(&header),
                "TELEMETRY.md missing section: {header}"
            );

            for field in cmd_fields {
                if field.collection_mode == "not collected" {
                    continue;
                }
                let field_row = format!("| `{}` | {} |", field.field_name, field.collection_mode);
                assert!(
                    telemetry_md.contains(&field_row),
                    "TELEMETRY.md has incorrect or missing row for MCP field `{}` in command `{}`. \
                     Expected row containing: {field_row}",
                    field.field_name,
                    command,
                );
            }
        }

        // Direction 2 — doc → code: every documented field exists in code
        let code_fields: std::collections::HashSet<(String, String)> = fields
            .iter()
            .map(|f| (f.command.clone(), f.field_name.clone()))
            .collect();
        let doc_fields = parse_doc_fields(&telemetry_md, "MCP");

        let stale: Vec<_> = doc_fields.difference(&code_fields).collect();
        assert!(
            stale.is_empty(),
            "TELEMETRY.md documents MCP fields not found in code: {stale:?}. \
             Remove stale rows or add the corresponding #[telemetry] annotations."
        );
    }
}
