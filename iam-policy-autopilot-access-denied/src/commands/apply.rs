//! Apply logic for IAM Policy Autopilot service

use crate::aws::iam_client::{find_canonical_policy, put_inline_policy};
use crate::aws::policy_naming::{build_canonical_policy_name, build_statement_sid, POLICY_PREFIX};
use crate::aws::principal::resolve_principal;
use crate::aws::sts::caller_account_id;
use crate::synthesis::build_single_statement;
use crate::synthesis::policy_builder::{merge_statements, sort_statements};
use crate::types::{
    ApplyError, ApplyOptions, ApplyResult, ApplyResultWithError, DenialType, PlanResult,
};

impl super::service::IamPolicyAutopilotService {
    /// Apply a policy fix: validates denial type, principal, and account; merges into canonical policy.
    /// Requires exactly one action for audit trail.
    pub async fn apply(&self, plan: &PlanResult, _options: ApplyOptions) -> ApplyResultWithError {
        if !matches!(plan.diagnosis.denial_type, DenialType::ImplicitIdentity) {
            return Err(ApplyError::UnsupportedDenialType);
        }

        let principal_info = resolve_principal(&plan.diagnosis.principal_arn)
            .map_err(ApplyError::UnsupportedPrincipal)?;

        let principal_account = extract_account_from_arn(&plan.diagnosis.principal_arn)
            .ok_or_else(|| {
                ApplyError::UnsupportedPrincipal(
                    "could not extract account id from principal ARN".to_string(),
                )
            })?;

        let caller_account = caller_account_id(&self.sts_client)
            .await
            .map_err(ApplyError::Aws)?;

        if caller_account != principal_account {
            return Err(ApplyError::AccountMismatch {
                principal_account,
                caller_account,
            });
        }

        let canonical_policy_name =
            build_canonical_policy_name(&principal_info.kind, &principal_info.name);

        let date = chrono::Utc::now().format("%Y%m%d").to_string();

        if plan.actions.len() != 1 {
            return Err(ApplyError::MultiActionError(plan.actions.len()));
        }
        let action = plan.actions[0].clone();

        let existing_policy =
            find_canonical_policy(&self.iam_client, &principal_info.kind, &principal_info.name)
                .await
                .map_err(ApplyError::Aws)?;

        let (final_policy, is_new_policy) = if let Some(existing) = existing_policy {
            let existing_sids: Vec<String> = existing
                .document
                .statement
                .iter()
                .map(|s| s.sid.clone())
                .collect();

            let sid = build_statement_sid(&action, &date, &existing_sids);

            let new_stmt =
                build_single_statement(action.clone(), plan.diagnosis.resource.clone(), sid);

            let new_key = new_stmt.to_key();
            for existing_stmt in &existing.document.statement {
                if existing_stmt.to_key() == new_key {
                    return Err(ApplyError::DuplicateStatement {
                        action: action.clone(),
                        resource: plan.diagnosis.resource.clone(),
                    });
                }
            }

            let mut merged_statements = merge_statements(existing.document.statement, new_stmt);

            sort_statements(&mut merged_statements);

            let policy_doc = crate::types::PolicyDocument {
                id: existing
                    .document
                    .id
                    .or_else(|| Some(POLICY_PREFIX.to_string())),
                version: "2012-10-17".to_string(),
                statement: merged_statements,
            };

            (policy_doc, false)
        } else {
            let sid = build_statement_sid(&action, &date, &[]);
            let stmt = build_single_statement(action.clone(), plan.diagnosis.resource.clone(), sid);

            let policy_doc = crate::types::PolicyDocument {
                id: Some(POLICY_PREFIX.to_string()),
                version: "2012-10-17".to_string(),
                statement: vec![stmt],
            };

            (policy_doc, true)
        };

        let statement_count = final_policy.statement.len();
        put_inline_policy(
            &self.iam_client,
            &principal_info.kind,
            &principal_info.name,
            &canonical_policy_name,
            &final_policy,
        )
        .await
        .map_err(ApplyError::Aws)?;

        Ok(ApplyResult {
            success: true,
            policy_name: canonical_policy_name,
            principal_kind: format!("{:?}", principal_info.kind),
            principal_name: principal_info.name,
            is_new_policy,
            statement_count,
            error: None,
        })
    }
}

/// Extract 12-digit account ID from ARN (field 5 in colon-delimited format)
pub fn extract_account_from_arn(arn: &str) -> Option<String> {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 6 {
        let account_id = parts[4];
        if !account_id.is_empty()
            && account_id.len() == 12
            && account_id.chars().all(|c| c.is_ascii_digit())
        {
            return Some(account_id.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_account_from_arn_valid() {
        let arn = "arn:aws:iam::123456789012:role/MyRole";
        assert_eq!(
            extract_account_from_arn(arn),
            Some("123456789012".to_string())
        );

        let arn = "arn:aws:iam::987654321098:user/john";
        assert_eq!(
            extract_account_from_arn(arn),
            Some("987654321098".to_string())
        );
    }

    #[test]
    fn test_extract_account_from_arn_invalid() {
        let arn = "not-an-arn";
        assert_eq!(extract_account_from_arn(arn), None);

        let arn = "arn:aws:iam";
        assert_eq!(extract_account_from_arn(arn), None);

        let arn = "arn:aws:iam::::";
        assert_eq!(extract_account_from_arn(arn), None);
    }

    #[test]
    fn test_extract_account_from_arn_non_numeric() {
        // Account ID contains non-numeric characters
        let arn = "arn:aws:iam::12345678901a:role/MyRole";
        assert_eq!(extract_account_from_arn(arn), None);

        let arn = "arn:aws:iam::abc123456789:user/john";
        assert_eq!(extract_account_from_arn(arn), None);
    }

    #[test]
    fn test_extract_account_from_arn_wrong_length() {
        // Account ID too short (11 digits)
        let arn = "arn:aws:iam::12345678901:role/MyRole";
        assert_eq!(extract_account_from_arn(arn), None);

        // Account ID too long (13 digits)
        let arn = "arn:aws:iam::1234567890123:role/MyRole";
        assert_eq!(extract_account_from_arn(arn), None);

        // Account ID way too short
        let arn = "arn:aws:iam::123:role/MyRole";
        assert_eq!(extract_account_from_arn(arn), None);
    }
}
