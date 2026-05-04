//! Autopilot CLI invocation — extract SDK calls, generate policies, and analyze results.

use std::{env, path::Path, process::Command};

use serde_json::{json, Value};
use tracing::{error, info};

use crate::types::{AutopilotPoliciesOutput, AutopilotSdkCall};

// ---------------------------------------------------------------------------
// Autopilot invocation via `cargo run --bin`
// ---------------------------------------------------------------------------

/// Return the path to the `cargo` binary.
///
/// Uses the `CARGO` environment variable (set by Cargo itself when running
/// under `cargo run` / `cargo test`), falling back to the bare string
/// `"cargo"` so that a PATH lookup is performed at exec time.
///
/// This follows the same pattern as cargo-nextest's integration tests.
fn cargo_bin() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

/// Build a [`Command`] that invokes an `iam-policy-autopilot` sub-command
/// via `cargo run --bin iam-policy-autopilot -- <args>`.
///
/// Cargo will automatically (re-)build the binary if it is stale, ensuring
/// the runner always exercises the version matching the current source tree.
fn autopilot_command(args: &[&str]) -> Command {
    let cargo = cargo_bin();
    let mut cmd = Command::new(&cargo);
    cmd.args(["run", "--bin", "iam-policy-autopilot", "--"]);
    cmd.args(args);
    cmd
}

// ---------------------------------------------------------------------------
// SDK call analysis
// ---------------------------------------------------------------------------

#[must_use]
pub(crate) fn analyze_sdk_calls(sdk_calls: &[AutopilotSdkCall]) -> Value {
    let mut single = 0usize;
    let mut multiple = 0usize;
    let mut extra = 0usize;
    let mut breakdown = Vec::new();

    for op in sdk_calls {
        let count = op.possible_services.len();
        if count == 1 {
            single += 1;
        } else if count > 1 {
            multiple += 1;
            extra += count - 1;
        }
        breakdown.push(json!({
            "name": op.name,
            "possible_services": op.possible_services,
            "service_count": count,
        }));
    }

    json!({
        "single_service_operations": single,
        "multiple_service_operations": multiple,
        "total_operations": breakdown.len(),
        "total_additional_services": extra,
        "operations_breakdown": breakdown,
    })
}

// ---------------------------------------------------------------------------
// iam-policy-autopilot helpers
// ---------------------------------------------------------------------------

/// Run `iam-policy-autopilot extract-sdk-calls --pretty <script>` via
/// `cargo run --bin` and return the parsed JSON array, or `None` on failure.
pub(crate) fn extract_sdk_calls(script_path: &Path) -> Option<Vec<AutopilotSdkCall>> {
    let script_str = script_path.to_string_lossy();
    info!(
        "[autopilot] Extracting SDK calls: extract-sdk-calls --pretty {}",
        script_str
    );

    let output = autopilot_command(&["extract-sdk-calls", "--pretty", &script_str]).output();

    match output {
        Err(e) => {
            error!("extract-sdk-calls exec error: {}", e);
            None
        }
        Ok(out) if !out.status.success() => {
            error!(
                "extract-sdk-calls failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Ok(out) => match serde_json::from_slice::<Vec<AutopilotSdkCall>>(&out.stdout) {
            Ok(calls) => Some(calls),
            Err(e) => {
                error!("Failed to parse extract-sdk-calls output: {}", e);
                None
            }
        },
    }
}

/// Run `iam-policy-autopilot generate-policies --region … --account … --pretty <script>`
/// via `cargo run --bin` and return a `Vec` of IAM policy documents, or `None` on failure.
pub(crate) fn generate_policies(
    script_path: &Path,
    region: &str,
    account: &str,
) -> Option<Vec<Value>> {
    let script_str = script_path.to_string_lossy();

    let output = autopilot_command(&[
        "generate-policies",
        "--region",
        region,
        "--account",
        account,
        "--pretty",
        &script_str,
    ])
    .output();

    match output {
        Err(e) => {
            error!("generate-policies exec error: {}", e);
            None
        }
        Ok(out) if !out.status.success() => {
            error!(
                "generate-policies failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Ok(out) => {
            let parsed: AutopilotPoliciesOutput = match serde_json::from_slice(&out.stdout) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to parse generate-policies output: {}", e);
                    return None;
                }
            };
            let policies: Vec<Value> = parsed
                .policies
                .into_iter()
                .map(|item| item.policy)
                .collect();
            if policies.is_empty() {
                error!("No valid policies found in iam-policy-autopilot output");
                return None;
            }
            Some(policies)
        }
    }
}

/// Run `iam-policy-autopilot generate-policies --individual-policies
///      --region … --account … --pretty <script>`
/// via `cargo run --bin` and return a `Vec` of per-SDK-call IAM policy
/// documents, or `None` on failure.
///
/// This is identical to [`generate_policies`] but passes `--individual-policies`
/// to the autopilot binary.  The output is a JSON array of policy documents
/// (one per SDK call), not the `{"Policies": [...]}` envelope.
pub fn generate_individual_policies(
    script_path: &Path,
    region: &str,
    account: &str,
) -> Option<Vec<Value>> {
    let script_str = script_path.to_string_lossy();
    info!("[autopilot] Generating individual IAM policies");

    let output = autopilot_command(&[
        "generate-policies",
        "--individual-policies",
        "--region",
        region,
        "--account",
        account,
        "--pretty",
        &script_str,
    ])
    .output();

    match output {
        Err(e) => {
            error!("generate-policies --individual-policies exec error: {}", e);
            None
        }
        Ok(out) if !out.status.success() => {
            error!(
                "generate-policies --individual-policies failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Ok(out) => {
            // The autopilot binary returns the same {"Policies": [...]} envelope
            // regardless of whether --individual-policies is passed.
            match serde_json::from_slice::<AutopilotPoliciesOutput>(&out.stdout) {
                Ok(parsed) if !parsed.policies.is_empty() => {
                    let docs: Vec<Value> = parsed
                        .policies
                        .into_iter()
                        .map(|item| item.policy)
                        .collect();
                    Some(docs)
                }
                Ok(_) => {
                    error!("generate-policies --individual-policies returned empty Policies array");
                    None
                }
                Err(e) => {
                    error!(
                        "Failed to parse generate-policies --individual-policies output: {}",
                        e
                    );
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AutopilotSdkCall;
    use rstest::rstest;

    /// Helper: build an `AutopilotSdkCall` from a name and service list.
    fn call(name: &str, services: &[&str]) -> AutopilotSdkCall {
        AutopilotSdkCall {
            name: name.to_string(),
            possible_services: services.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A declarative test case for `analyze_sdk_calls` with named fields.
    struct AnalyzeCase {
        /// Input SDK calls to analyze.
        calls: Vec<AutopilotSdkCall>,
        /// Expected total number of operations.
        expect_total: u64,
        /// Expected count of single-service operations.
        expect_single: u64,
        /// Expected count of multi-service operations.
        expect_multiple: u64,
        /// Expected count of additional (extra) services beyond the first.
        expect_extra: u64,
    }

    #[rstest]
    #[case::empty(AnalyzeCase {
        calls: vec![],
        expect_total: 0,
        expect_single: 0,
        expect_multiple: 0,
        expect_extra: 0,
    })]
    #[case::all_single_service(AnalyzeCase {
        calls: vec![call("GetObject", &["s3"]), call("PutItem", &["dynamodb"])],
        expect_total: 2,
        expect_single: 2,
        expect_multiple: 0,
        expect_extra: 0,
    })]
    #[case::all_multi_service(AnalyzeCase {
        calls: vec![
            call("PutObject", &["s3", "s3-object-lambda"]),
            call("GetPartitions", &["glue", "athena", "lakeformation"]),
        ],
        expect_total: 2,
        expect_single: 0,
        expect_multiple: 2,
        expect_extra: 3,  // (2-1) + (3-1) = 3
    })]
    #[case::mixed(AnalyzeCase {
        calls: vec![
            call("GetObject", &["s3"]),
            call("PutObject", &["s3", "s3-object-lambda"]),
            call("CreateTable", &["dynamodb"]),
        ],
        expect_total: 3,
        expect_single: 2,
        expect_multiple: 1,
        expect_extra: 1,
    })]
    #[case::zero_services(AnalyzeCase {
        calls: vec![call("UnknownOp", &[])],
        expect_total: 1,
        expect_single: 0,
        expect_multiple: 0,
        expect_extra: 0,
    })]
    fn analyze_sdk_calls_stats(#[case] case: AnalyzeCase) {
        let result = analyze_sdk_calls(&case.calls);
        assert_eq!(result["total_operations"], case.expect_total);
        assert_eq!(result["single_service_operations"], case.expect_single);
        assert_eq!(result["multiple_service_operations"], case.expect_multiple);
        assert_eq!(result["total_additional_services"], case.expect_extra);
    }

    // ── analyze_sdk_calls — breakdown structure ──────────────────────────────

    #[test]
    fn analyze_operations_breakdown_structure() {
        let calls = vec![call("PutObject", &["s3", "s3-object-lambda"])];
        let result = analyze_sdk_calls(&calls);
        let breakdown = result["operations_breakdown"].as_array().unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0]["name"], "PutObject");
        assert_eq!(breakdown[0]["service_count"], 2);
        assert_eq!(
            breakdown[0]["possible_services"],
            serde_json::json!(["s3", "s3-object-lambda"])
        );
    }
}
