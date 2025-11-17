use anyhow::{Context, Result};
use iam_policy_autopilot_access_denied::{DenialType, PlanResult};
use iam_policy_autopilot_policy_generation::policy_generation::{
    MethodActionMapping, PolicyWithMetadata,
};
use iam_policy_autopilot_tools::BatchUploadResponse;
use log::debug;
use std::io::{self, Write};

pub(crate) fn note(msg: &str) {
    let _ = writeln!(io::stderr(), "iam-policy-autopilot: {}", msg);
}

pub(crate) fn warn(msg: &str) {
    let _ = writeln!(io::stderr(), "iam-policy-autopilot (warning): {}", msg);
}

pub(crate) fn print_plan(plan: &PlanResult) {
    let stderr = io::stderr();
    let mut w = stderr.lock();
    let _ = writeln!(w, "IAM Policy Autopilot Plan");
    let _ = writeln!(w, "Principal: {}", plan.diagnosis.principal_arn);
    let _ = writeln!(w, "Action:    {}", plan.diagnosis.action);
    let _ = writeln!(w, "Resource:  {}", plan.diagnosis.resource);
    let _ = writeln!(w, "Denial:    {:?}", plan.diagnosis.denial_type);
    let _ = writeln!(w);
    let _ = writeln!(w, "Proposed permissions:");
    for a in &plan.actions {
        let _ = writeln!(w, "  - {}", a);
    }
    let _ = writeln!(w);
    if !matches!(plan.diagnosis.denial_type, DenialType::ImplicitIdentity) {
        let _ = writeln!(w, "Note: explain-only; not eligible for apply in V1.");
        let _ = writeln!(w);
    }
}

pub(crate) fn prompt_apply_once() {
    let _ = write!(io::stderr(), "Apply this fix now? [y/N] ");
    let _ = io::stderr().flush();
}

pub(crate) fn print_apply_success(policy_name: &str, principal_kind: &str, principal_name: &str) {
    let _ = writeln!(
        io::stderr(),
        "Applied inline policy '{}' to {}/{}",
        policy_name,
        principal_kind,
        principal_name
    );
}

pub(crate) fn print_apply_refused(reason_code: &str, hint: &str) {
    let _ = writeln!(
        io::stderr(),
        "iam-policy-autopilot: apply refused ({}) — {}",
        reason_code,
        hint
    );
}

pub(crate) fn print_statement_added(
    policy_name: &str,
    principal_kind: &str,
    principal_name: &str,
    statement_count: usize,
) {
    let _ = writeln!(
        io::stderr(),
        "Added statement to policy '{}' on {}/{} (now {} statements total)",
        policy_name,
        principal_kind,
        principal_name,
        statement_count
    );
}

pub(crate) fn print_duplicate_statement(action: &str, resource: &str) {
    let _ = writeln!(
        io::stderr(),
        "iam-policy-autopilot: duplicate statement detected"
    );
    let _ = writeln!(
        io::stderr(),
        "The canonical policy already contains permission for:"
    );
    let _ = writeln!(io::stderr(), "  Action:   {}", action);
    let _ = writeln!(io::stderr(), "  Resource: {}", resource);
}

pub(crate) fn print_resource_policy_fix(action: &str, resource: &str, statement_json: &str) {
    let stderr = io::stderr();
    let mut w = stderr.lock();
    let _ = writeln!(w, "iam-policy-autopilot: ResourcePolicy denial detected");
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "This access denial is caused by a resource-based policy."
    );
    let _ = writeln!(w, "The resource owner must manually update the policy on:");
    let _ = writeln!(w);
    let _ = writeln!(w, "  Action:   {}", action);
    let _ = writeln!(w, "  Resource: {}", resource);
    let _ = writeln!(w);
    let _ = writeln!(w, "Add this statement to the resource policy:");
    let _ = writeln!(w);
    let _ = writeln!(w, "{}", statement_json);
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "Note: This tool cannot automatically apply resource policy changes."
    );
    let _ = writeln!(
        w,
        "The resource owner must apply this change through the AWS Console or CLI."
    );
}

pub(crate) fn print_explicit_deny_explanation() {
    let stderr = io::stderr();
    let mut w = stderr.lock();
    let _ = writeln!(w, "iam-policy-autopilot: ExplicitIdentity denial detected");
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "This access is blocked by an explicit Deny statement in an identity-based policy."
    );
    let _ = writeln!(
        w,
        "Explicit denies override all Allow statements and cannot be automatically fixed."
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "To resolve this, you must:");
    let _ = writeln!(w, "  1. Locate the policy with the explicit Deny statement");
    let _ = writeln!(
        w,
        "  2. Either remove the Deny statement or modify its conditions"
    );
    let _ = writeln!(
        w,
        "  3. Ensure your Allow policies grant the required permissions"
    );
}

pub(crate) fn print_unsupported_denial(denial_type: &DenialType, reason: &str) {
    let stderr = io::stderr();
    let mut w = stderr.lock();
    let _ = writeln!(w, "iam-policy-autopilot: Unsupported denial type");
    let _ = writeln!(w);
    let _ = writeln!(w, "Denial Type: {:?}", denial_type);
    let _ = writeln!(w, "Reason: {}", reason);
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "This type of access denial cannot be automatically fixed by this tool."
    );
}

// ========== IAM Policy Output Functions (for IAM Policy Autopilot CLI integration) ==========

/// Standard policy output structure
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyOutput {
    /// The generated policies with type information
    policies: Vec<PolicyWithMetadata>,
    /// Upload results (only present when --upload-policies is used)
    #[serde(skip_serializing_if = "Option::is_none")]
    upload_result: Option<BatchUploadResponse>,
}

/// Combined output structure when showing action mappings alongside policies
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct CombinedPolicyOutput {
    /// Method to action mappings
    method_action_mappings: Vec<MethodActionMapping>,
    /// The generated policies with type information
    policies: Vec<PolicyWithMetadata>,
    /// Upload results (only present when --upload-policies is used)
    #[serde(skip_serializing_if = "Option::is_none")]
    upload_result: Option<BatchUploadResponse>,
}

/// Output combined policy and mappings as JSON to stdout
pub(crate) fn output_combined_policy_mappings(
    method_action_mappings: Vec<MethodActionMapping>,
    policies: Vec<PolicyWithMetadata>,
    pretty: bool,
) -> Result<()> {
    debug!(
        "Formatting combined policy and mappings output as JSON (pretty: {})",
        pretty
    );

    let combined_output = CombinedPolicyOutput {
        method_action_mappings,
        policies,
        upload_result: None,
    };

    let json_output = if pretty {
        iam_policy_autopilot_policy_generation::JsonProvider::stringify_pretty(&combined_output)
            .context("Failed to serialize combined output to pretty JSON")?
    } else {
        iam_policy_autopilot_policy_generation::JsonProvider::stringify(&combined_output)
            .context("Failed to serialize combined output to JSON")?
    };

    // Output to stdout (not using println! to avoid extra newline in compact mode)
    print!("{}", json_output);
    if pretty {
        println!(); // Add newline for pretty output
    }

    debug!("Combined policy and mappings JSON output written to stdout");
    Ok(())
}

/// Output IAM policies as JSON to stdout
pub(crate) fn output_iam_policies(
    policies: Vec<PolicyWithMetadata>,
    upload_result: Option<BatchUploadResponse>,
    pretty: bool,
) -> Result<()> {
    debug!(
        "Formatting IAM policies output as JSON (pretty: {})",
        pretty
    );

    let policy_output = PolicyOutput {
        policies,
        upload_result,
    };

    let json_output = if pretty {
        iam_policy_autopilot_policy_generation::JsonProvider::stringify_pretty(&policy_output)
            .context("Failed to serialize policy output to pretty JSON")?
    } else {
        iam_policy_autopilot_policy_generation::JsonProvider::stringify(&policy_output)
            .context("Failed to serialize policy output to JSON")?
    };

    // Output to stdout (not using println! to avoid extra newline in compact mode)
    print!("{}", json_output);
    if pretty {
        println!(); // Add newline for pretty output
    }

    debug!("Policy output JSON written to stdout");
    Ok(())
}
