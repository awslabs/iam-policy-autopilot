//! IAM execution role management — create, attach policies, and clean up.

use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_iam::{error::SdkError, Client as IamClient};
use serde_json::{json, Value};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::types::RoleInfo;

// ---------------------------------------------------------------------------
// IAM execution role management
// ---------------------------------------------------------------------------

/// Create a temporary IAM role with inline IAM policy documents attached.
/// Returns [`RoleInfo`] on success.
///
/// The trust policy is scoped to `caller_arn` so that only the runner's own
/// identity can assume the role — not any arbitrary principal in the account.
pub(crate) async fn create_execution_role(
    iam: &IamClient,
    policy_docs: Vec<Value>,
    role_suffix: &str,
    account: &str,
    caller_arn: &str,
) -> Result<RoleInfo> {
    let role_name = format!("runner-role-{role_suffix}");

    // ── Clean up any pre-existing role with the same name ──────────────────
    if iam.get_role().role_name(&role_name).send().await.is_ok() {
        warn!("Role {} already exists -- cleaning up first ...", role_name);
        // Detach and delete any attached managed policies.
        if let Ok(attached) = iam
            .list_attached_role_policies()
            .role_name(&role_name)
            .send()
            .await
        {
            for pol in attached.attached_policies() {
                let arn = pol.policy_arn().unwrap_or_default();
                let _ = iam
                    .detach_role_policy()
                    .role_name(&role_name)
                    .policy_arn(arn)
                    .send()
                    .await;
                if arn.contains("runner-") {
                    let _ = iam.delete_policy().policy_arn(arn).send().await;
                }
            }
        }
        // Delete all inline policies — IAM requires this before the role
        // can be deleted.
        if let Ok(inline) = iam.list_role_policies().role_name(&role_name).send().await {
            for policy_name in inline.policy_names() {
                let _ = iam
                    .delete_role_policy()
                    .role_name(&role_name)
                    .policy_name(policy_name)
                    .send()
                    .await;
            }
        }
        iam.delete_role().role_name(&role_name).send().await.ok();
        sleep(Duration::from_secs(2)).await;
    }

    // ── Create role ─────────────────────────────────────────────────────────
    // Trust policy scoped to the runner's own identity (caller_arn) so that
    // only the orchestrator can assume these temporary roles — not any
    // arbitrary principal in the account.
    let trust_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": caller_arn },
            "Action": "sts:AssumeRole"
        }]
    });

    debug!("[IAM] Creating role: {}", role_name);
    let permissions_boundary_arn =
        format!("arn:aws:iam::{account}:policy/runner-role-permissions-boundary");
    iam.create_role()
        .role_name(&role_name)
        .assume_role_policy_document(trust_policy.to_string())
        .permissions_boundary(&permissions_boundary_arn)
        .description(format!("Temporary execution role for runner {role_suffix}"))
        .send()
        .await
        .with_context(|| format!("iam:CreateRole failed for {role_name}"))?;

    // ── Attach inline policies ───────────────────────────────────────────────
    //
    // We use iam:PutRolePolicy (inline role policies) rather than
    // iam:CreatePolicy + iam:AttachRolePolicy.  Inline policies:
    //   • Are not subject to the 10-attached-managed-policies-per-role limit.
    //   • Do not consume the per-account managed-policy quota (default 1500).
    //   • Are deleted automatically when the role is deleted — no separate
    //     cleanup step is required.
    //   • Support up to 10,240 bytes per inline policy document.
    //
    // Merge all individual policy documents into a single combined
    // policy document (one inline policy on the role).  A single
    // inline policy can hold up to 10,240 bytes; for the typical
    // autopilot output this is well within the limit.
    // Merge all statements from every document.  When combining
    // multiple policy documents into one, duplicate Sid values would
    // cause IAM to reject the merged document with
    // MalformedPolicyDocument.  Sids are optional identifiers that
    // carry no semantic weight for permission evaluation, so we strip
    // them during the merge.
    let mut all_statements: Vec<Value> = Vec::new();
    for (i, doc) in policy_docs.iter().enumerate() {
        let stmt_value = doc.get("Statement").ok_or_else(|| {
            anyhow::anyhow!(
                "Policy document {} is malformed: missing \"Statement\" key. Document: {}",
                i,
                serde_json::to_string_pretty(doc).unwrap_or_else(|_| format!("{doc:?}"))
            )
        })?;
        // Accept both array form and single-object shorthand (valid IAM JSON).
        let statements: Vec<&Value> = match stmt_value {
            Value::Array(arr) => arr.iter().collect(),
            Value::Object(_) => vec![stmt_value],
            _ => {
                anyhow::bail!(
                    "Policy document {} is malformed: \"Statement\" is neither an array nor \
                     an object. Document: {}",
                    i,
                    serde_json::to_string_pretty(doc).unwrap_or_else(|_| format!("{doc:?}"))
                );
            }
        };
        for stmt in statements {
            let mut s = stmt.clone();
            if let Some(obj) = s.as_object_mut() {
                obj.remove("Sid");
            }
            all_statements.push(s);
        }
    }

    // IAM rejects a PutRolePolicy call with an empty Statement array.
    // When the minimizer tests the empty set (no policies), we simply
    // create the role with no inline policy attached — the script will
    // fail with AccessDenied, which is the correct "FAIL" signal.
    let policy_names = if all_statements.is_empty() {
        info!("[IAM] No statements — skipping PutRolePolicy (role has no permissions)");
        vec![]
    } else {
        let merged = json!({
            "Version": "2012-10-17",
            "Statement": all_statements,
        });

        let inline_policy_name = format!("runner-inline-{role_suffix}");
        let policy_doc = merged.to_string();
        info!(
            "[IAM] Attaching inline policy '{}' ({} statements, {} bytes) to role {} ...",
            inline_policy_name,
            all_statements.len(),
            policy_doc.len(),
            role_name
        );

        iam.put_role_policy()
            .role_name(&role_name)
            .policy_name(&inline_policy_name)
            .policy_document(&policy_doc)
            .send()
            .await
            .map_err(|e| {
                // Extract the AWS service error code + message when available.
                let detail = match &e {
                    SdkError::ServiceError(svc) => {
                        let code = svc.err().meta().code().unwrap_or("<no code>");
                        let msg = svc.err().meta().message().unwrap_or("<no message>");
                        format!("AWS error code={code} message={msg}")
                    }
                    other => format!("{other:#}"),
                };
                error!("[IAM] iam:PutRolePolicy failed for role {}", role_name);
                debug!(
                    "[IAM] iam:PutRolePolicy failed for role {} (policy '{}', {} bytes): {}",
                    role_name,
                    inline_policy_name,
                    policy_doc.len(),
                    detail
                );
                anyhow::anyhow!("iam:PutRolePolicy failed for role {role_name}")
            })?;

        vec![inline_policy_name]
    };

    // Wait for role + policies to be fully available.
    info!("[IAM] Waiting 10 s for role to be fully available ...");
    sleep(Duration::from_secs(10)).await;

    Ok(RoleInfo {
        role_arn: format!("arn:aws:iam::{account}:role/{role_name}"),
        role_name,
        policy_names,
    })
}

/// Create a temporary deploy role with a single inline policy from `deploy_policy.json`.
///
/// Uses the same pattern as [`create_execution_role`] but with a "deploy-role-" prefix.
/// The deploy role is assumed by the CDK CLI during `cdk deploy` and `cdk destroy`.
pub async fn create_deploy_role(
    iam: &IamClient,
    deploy_policy: Value,
    role_suffix: &str,
    account: &str,
    caller_arn: &str,
) -> Result<RoleInfo> {
    create_execution_role(iam, vec![deploy_policy], role_suffix, account, caller_arn).await
}

/// Delete the execution role and its inline policies.
///
/// IAM requires all inline policies to be removed before `delete_role` will
/// succeed.
pub async fn cleanup_execution_role(iam: &IamClient, role_info: &RoleInfo) {
    info!("[IAM] Cleaning up role {} ...", role_info.role_name);
    // Delete all inline policies — IAM requires this before delete_role.
    if let Ok(inline) = iam
        .list_role_policies()
        .role_name(&role_info.role_name)
        .send()
        .await
    {
        for policy_name in inline.policy_names() {
            let _ = iam
                .delete_role_policy()
                .role_name(&role_info.role_name)
                .policy_name(policy_name)
                .send()
                .await;
        }
    }
    match iam
        .delete_role()
        .role_name(&role_info.role_name)
        .send()
        .await
    {
        Ok(_) => info!("[IAM] Role {} cleaned up", role_info.role_name),
        Err(e) => warn!("Error cleaning up role {}: {}", role_info.role_name, e),
    }
}
