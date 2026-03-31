//! Integration tests for Terraform resource binding.
//!
//! Each fixture is a directory under `tests/resources/terraform/` containing:
//! - `fixture.json` — list of test cases with inputs and expected output file
//! - `.tf` files, `.py` source files, and expected JSON output files
//!
//! The harness auto-discovers all `fixture.json` files via `rstest`'s `#[files]`
//! glob, creating one named test per fixture. Each test runs all cases from
//! the fixture's `fixture.json`.
//!
//! To add a new fixture: create a directory with source files + `.tf` files +
//! `fixture.json` — no Rust code changes needed.

use std::path::{Path, PathBuf};

use rstest::rstest;
use serde::Deserialize;

use iam_policy_autopilot_policy_generation::api::generate_policies;
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePolicyConfig,
};

// ---------------------------------------------------------------------------
// Fixture configuration types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TestCase {
    name: String,
    inputs: TestInputs,
    expected_output: String,
}

#[derive(Debug, Deserialize)]
struct TestInputs {
    source_files: Vec<String>,
    language: String,
    region: String,
    account: String,
    individual_policies: bool,
    tf_dir: bool,
    tf_files: Vec<String>,
    tfvars: Vec<String>,
    tfstate: Vec<String>,
    explain_resource_filters: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_fixture(fixture_dir: &Path) -> Vec<TestCase> {
    let path = fixture_dir.join("fixture.json");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn build_config(fixture_dir: &Path, inputs: &TestInputs) -> GeneratePolicyConfig {
    let resolve = |f: &str| fixture_dir.join(f);

    GeneratePolicyConfig {
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: inputs.source_files.iter().map(|f| resolve(f)).collect(),
            language: Some(inputs.language.clone()),
            service_hints: None,
        },
        aws_context: AwsContext::new(inputs.region.clone(), inputs.account.clone()).unwrap(),
        individual_policies: inputs.individual_policies,
        minimize_policy_size: false,
        disable_file_system_cache: false,
        explain_filters: None,
        terraform_dir: if inputs.tf_dir {
            Some(fixture_dir.to_path_buf())
        } else {
            None
        },
        terraform_files: inputs.tf_files.iter().map(|f| resolve(f)).collect(),
        tfstate_paths: inputs.tfstate.iter().map(|f| resolve(f)).collect(),
        tfvars_files: inputs.tfvars.iter().map(|f| resolve(f)).collect(),
        explain_resource_filters: inputs.explain_resource_filters.clone(),
    }
}

/// Normalize a JSON value for deterministic, path-independent comparison.
///
/// - Drops `Sid` keys (non-deterministic dedup counters)
/// - Drops `Explanations` key (action explanations tested separately)
/// - Sorts arrays by serialized form (non-deterministic HashMap iteration)
/// - Strips absolute fixture directory prefix from all string values
fn normalize(v: &serde_json::Value, fixture_dir: &Path) -> serde_json::Value {
    let prefix = fixture_dir.to_str().unwrap_or("");

    match v {
        serde_json::Value::String(s) => {
            let stripped = s.strip_prefix(prefix).unwrap_or(s);
            let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
            serde_json::Value::String(stripped.to_string())
        }
        serde_json::Value::Object(map) => {
            let m: serde_json::Map<_, _> = map
                .iter()
                .filter(|(k, _)| {
                    let key = k.as_str();
                    key != "Sid" && key != "Explanations"
                })
                .map(|(k, v)| (k.clone(), normalize(v, fixture_dir)))
                .collect();
            serde_json::Value::Object(m)
        }
        serde_json::Value::Array(arr) => {
            let mut v: Vec<_> = arr.iter().map(|x| normalize(x, fixture_dir)).collect();
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

/// Run a single test case from a fixture.
async fn run_test_case(fixture_name: &str, fixture_dir: &Path, test_case: &TestCase) {
    let cfg = build_config(fixture_dir, &test_case.inputs);
    assert!(
        !cfg.extract_sdk_calls_config.source_files.is_empty(),
        "[{fixture_name}:{name}] needs at least one source file",
        name = test_case.name
    );

    let result = generate_policies(&cfg).await.unwrap_or_else(|e| {
        panic!(
            "[{fixture_name}:{name}] generate_policies failed: {e}",
            name = test_case.name
        )
    });

    let actual = normalize(&serde_json::to_value(&result).unwrap(), fixture_dir);

    let expected_path = fixture_dir.join(&test_case.expected_output);
    let expected_raw: serde_json::Value = {
        let raw = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
            panic!(
                "[{fixture_name}:{name}] read {file}: {e}",
                name = test_case.name,
                file = test_case.expected_output
            )
        });
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            panic!(
                "[{fixture_name}:{name}] parse {file}: {e}",
                name = test_case.name,
                file = test_case.expected_output
            )
        })
    };
    let expected = normalize(&expected_raw, fixture_dir);

    assert_eq!(
        actual,
        expected,
        "[{fixture_name}:{name}] output mismatch for {file}",
        name = test_case.name,
        file = test_case.expected_output
    );
}

// ===================================================================
// Auto-discovered fixture tests
//
// rstest's #[files] glob discovers all fixture.json files at compile time,
// creating one named test per fixture directory. Each test runs all cases
// from the fixture's fixture.json sequentially.
//
// Test output:
//   test_fixture::basic_multi_resource ... ok
//   test_fixture::variable_resolution ... ok
//   test_fixture::multi_file_resources ... ok
//   ...
// ===================================================================

#[rstest]
#[tokio::test]
async fn test_fixture(#[files("tests/resources/terraform/*/fixture.json")] fixture_json: PathBuf) {
    let fixture_dir = fixture_json.parent().unwrap();
    let fixture_name = fixture_dir.file_name().unwrap().to_str().unwrap();

    let test_cases = load_fixture(fixture_dir);
    assert!(
        !test_cases.is_empty(),
        "[{fixture_name}] fixture.json has no test cases"
    );

    for test_case in &test_cases {
        run_test_case(fixture_name, fixture_dir, test_case).await;
    }
}
