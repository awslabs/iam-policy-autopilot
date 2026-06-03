//! Integration tests for the generate-model CLI subcommand.
//!
//! Requires gopls and the Go toolchain to be installed.
//! Run with: `cargo test -p iam-policy-autopilot-cli --features model-generation --test generate_model_tests -- --ignored`

#![cfg(feature = "model-generation")]

use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use std::process::Command;

const SOURCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../integration-tests/projects/run_001/go/script.go"
);
const ENTRY_POINT: &str = "script.go:125:1";

fn generate_model_cmd(extra_args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("iam-policy-autopilot").unwrap();
    cmd.args([
        "generate-model",
        SOURCE,
        "--entry-point",
        ENTRY_POINT,
        "--library-name",
        "test-lib",
    ]);
    cmd.args(extra_args);
    cmd
}

fn run_and_parse(extra_args: &[&str]) -> Value {
    let output = generate_model_cmd(extra_args).output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore]
fn test_produces_expected_operations() {
    let json = run_and_parse(&["--pretty"]);

    assert_eq!(json["library_name"], "test-lib");
    assert_eq!(json["language"], "go");

    let ops: Vec<&str> = json["call_patterns"][0]["sdk_operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["operation"].as_str().unwrap())
        .collect();

    assert!(ops.contains(&"GetCallerIdentity"));
    assert!(ops.contains(&"ExecuteStatement"));
    assert!(ops.contains(&"DescribeStatement"));
}

#[test]
#[ignore]
fn test_service_hints_filters_results() {
    let json = run_and_parse(&["--service-hints", "sts"]);

    let ops = json["call_patterns"][0]["sdk_operations"]
        .as_array()
        .unwrap();
    assert!(ops.iter().all(|o| o["service"] == "sts"));
}

#[test]
#[ignore]
fn test_invalid_entry_point_shows_available_functions() {
    generate_model_cmd(&[])
        .args(["--entry-point", "script.go:999:1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No function declaration"))
        .stderr(predicate::str::contains("Available functions"));
}
