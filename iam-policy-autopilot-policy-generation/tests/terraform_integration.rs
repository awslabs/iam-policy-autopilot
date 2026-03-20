//! Integration tests for Terraform resource binding.
//!
//! Each fixture is a directory under `tests/resources/terraform/` containing:
//! - `.tf` files (Terraform configuration)
//! - `app.py` (Python source with SDK calls)
//! - `expected_policies.json` (expected `generate_policies` output)
//! - Optionally `terraform.tfstate`, `variables.tf`, `terraform.tfvars`, `*.auto.tfvars`
//!
//! The test calls `generate_policies` and compares the output against
//! `expected_policies.json`.

use std::path::{Path, PathBuf};

use rstest::rstest;

use iam_policy_autopilot_policy_generation::api::generate_policies;
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePolicyConfig,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/resources/terraform")
}

fn fixture_dir(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

fn discover_source_files(name: &str) -> Vec<PathBuf> {
    let dir = fixture_dir(name);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("[{name}] read dir: {e}"))
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()?.to_str()? == "py").then_some(p)
        })
        .collect();
    files.sort();
    files
}

fn discover_tfstate(name: &str) -> Vec<PathBuf> {
    let p = fixture_dir(name).join("terraform.tfstate");
    if p.exists() { vec![p] } else { vec![] }
}

fn config(name: &str) -> GeneratePolicyConfig {
    GeneratePolicyConfig {
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: discover_source_files(name),
            language: Some("python".to_string()),
            service_hints: None,
        },
        aws_context: AwsContext::new("us-east-1".into(), "123456789012".into()).unwrap(),
        individual_policies: true,
        minimize_policy_size: false,
        disable_file_system_cache: false,
        explain_filters: None,
        terraform_dir: Some(fixture_dir(name)),
        terraform_files: vec![],
        tfstate_paths: discover_tfstate(name),
        tfvars_files: vec![],
    }
}

/// Normalize a JSON value so that two semantically-equal policy outputs
/// compare as equal even when the runtime produces them in different order.
///
/// Two sources of non-determinism in the `generate_policies` output:
///
/// 1. **Policy / statement ordering** — Resources are stored in a `HashMap`
///    internally, so iteration order varies between runs. We sort every JSON
///    array by its serialized form to produce a canonical ordering.
///
/// 2. **Sid values** — Statement Ids include a dedup counter whose value
///    depends on iteration order. We drop `Sid` keys entirely since they
///    carry no policy semantics.
fn normalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let m: serde_json::Map<_, _> = map
                .iter()
                .filter(|(k, _)| k.as_str() != "Sid") // drop Sid — non-deterministic
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect();
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(arr) => {
            let mut v: Vec<_> = arr.iter().map(normalize).collect();
            v.sort_by(|a, b| {
                serde_json::to_string(a)
                    .unwrap_or_default()
                    .cmp(&serde_json::to_string(b).unwrap_or_default())
            });
            serde_json::Value::Array(v)
        }
        other => other.clone(),
    }
}

fn load_expected(name: &str) -> serde_json::Value {
    let path = fixture_dir(name).join("expected_policies.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("[{name}] read expected_policies.json: {e}"));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("[{name}] parse expected_policies.json: {e}"))
}

// ===================================================================
// Fixture tests
// ===================================================================

#[rstest]
#[case("basic_multi_resource")]
#[case("variable_resolution")]
#[case("mixed_providers")]
#[case("sub_resource_fallback")]
#[case("mixed_concrete_and_expression")]
#[case("multi_service_real_world")]
#[case("state_precedence")]
#[case("expression_unresolvable")]
#[case("multi_file_resources")]
#[case("multiple_tfvars")]
#[tokio::test]
async fn test_fixture(#[case] name: &str) {
    let cfg = config(name);
    assert!(
        !cfg.extract_sdk_calls_config.source_files.is_empty(),
        "[{name}] fixture needs at least one .py file"
    );

    let result = generate_policies(&cfg)
        .await
        .unwrap_or_else(|e| panic!("[{name}] generate_policies failed: {e}"));

    let actual = normalize(&serde_json::to_value(&result).unwrap());
    let expected = normalize(&load_expected(name));

    assert_eq!(actual, expected, "[{name}] policy output mismatch");
}
