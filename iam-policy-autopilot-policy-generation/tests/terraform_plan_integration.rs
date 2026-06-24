//! Integration tests for Terraform **plan-based** policy generation.
//!
//! These are intentionally separate from `terraform_integration.rs`, which
//! tests resource binding for policies extracted from application *source
//! code*. Plan-based generation has no application source files: it derives IAM
//! actions from a `terraform show -json` plan via the embedded CRUD map +
//! model. Keeping the two harnesses apart avoids overloading either fixture
//! shape.
//!
//! The tests drive the public `generate_policies` API end to end (embedded
//! artifacts included) and assert on the serialized policy JSON, since the
//! `Statement` fields are crate-private.

use std::collections::BTreeSet;
use std::io::Write;

use iam_policy_autopilot_policy_generation::api::generate_policies;
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePoliciesResult, GeneratePolicyConfig,
};
use rstest::rstest;
use serde_json::Value;
use tempfile::NamedTempFile;

/// Write `contents` to a temp file and return it (kept alive by the caller).
fn temp_json(contents: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create temp file");
    file.write_all(contents.as_bytes())
        .expect("write temp file");
    file
}

/// Build a plan-only config. The plan is passed as a positional input file (no
/// dedicated flag) and auto-detected by content; no ARN binding inputs are
/// provided, so resources fall back to wildcards.
fn plan_only_config(plan_path: std::path::PathBuf) -> GeneratePolicyConfig {
    GeneratePolicyConfig {
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: vec![plan_path],
            language: None,
            service_hints: None,
        },
        aws_context: AwsContext::new("us-east-1".to_string(), "123456789012".to_string()).unwrap(),
        individual_policies: false,
        minimize_policy_size: false,
        disable_file_system_cache: false,
        resource_cutoff: iam_policy_autopilot_policy_generation::DEFAULT_RESOURCE_CUTOFF,
        explain_filters: None,
        terraform_dir: None,
        terraform_files: vec![],
        tfstate_paths: vec![],
        tfvars_files: vec![],
        explain_resource_filters: None,
    }
}

/// Collect every `Action` string across all generated policy statements.
fn actions(result: &GeneratePoliciesResult) -> BTreeSet<String> {
    let json = serde_json::to_value(&result.policies).expect("serialize policies");
    let mut actions = BTreeSet::new();
    collect_actions(&json, &mut actions);
    actions
}

fn collect_actions(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(items)) = map.get("Action") {
                for item in items {
                    if let Value::String(s) = item {
                        out.insert(s.clone());
                    }
                }
            }
            for v in map.values() {
                collect_actions(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_actions(v, out);
            }
        }
        _ => {}
    }
}

/// Build a single-resource `aws_accessanalyzer_analyzer` plan with the given
/// planned actions.
fn analyzer_plan(actions: &str) -> String {
    format!(
        r#"{{
            "format_version": "1.2",
            "resource_changes": [
                {{
                    "address": "aws_accessanalyzer_analyzer.example",
                    "type": "aws_accessanalyzer_analyzer",
                    "mode": "managed",
                    "change": {{ "actions": {actions}, "after": {{ "analyzer_name": "example" }} }}
                }}
            ]
        }}"#
    )
}

#[rstest]
// Create exercises the Create + Read handlers → CreateAnalyzer + GetAnalyzer
// (botocore id `accessanalyzer` → IAM prefix `access-analyzer`). The shared
// enrichment layer additionally expands the create into the tagging action it
// requires (TagResource) — the same behavior the source-code path gets,
// confirming the plan path reuses enrichment unchanged.
#[case::create(
    analyzer_plan(r#"["create"]"#),
    &["access-analyzer:CreateAnalyzer", "access-analyzer:GetAnalyzer", "access-analyzer:TagResource"]
)]
// A no-op change still reads state back on apply → Read slot only.
#[case::no_op(analyzer_plan(r#"["no-op"]"#), &["access-analyzer:GetAnalyzer"])]
// An empty plan touches nothing → no actions at all.
#[case::empty(r#"{ "format_version": "1.2", "resource_changes": [] }"#.to_string(), &[])]
#[tokio::test]
async fn plan_emits_expected_actions(#[case] plan: String, #[case] expected: &[&str]) {
    let plan_file = temp_json(&plan);
    let config = plan_only_config(plan_file.path().to_path_buf());

    let result = generate_policies(&config).await.expect("generate policies");

    assert_eq!(
        actions(&result),
        expected
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
}
