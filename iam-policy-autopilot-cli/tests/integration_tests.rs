//! Integration tests for the IAM Policy Autopilot CLI binary.
//!
//! These tests verify the complete CLI functionality including argument parsing,
//! file processing, JSON output formatting, and error handling scenarios.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Helper function to get paths to real test files with specified extension
fn get_test_files(extension: &str) -> Vec<PathBuf> {
    let test_resources_dir = PathBuf::from("tests/resources");

    // Read all files in the test resources directory
    let mut test_files = Vec::new();

    if let Ok(entries) = fs::read_dir(&test_resources_dir) {
        for entry in entries.flatten() {
            let path: PathBuf = entry.path();

            // Only include files (not directories) with the specified extension
            if path.is_file() && path.extension().is_some_and(|ext| ext == extension) {
                test_files.push(path);
            }
        }
    }

    // Sort for consistent ordering in tests
    test_files.sort();
    test_files
}

/// Helper function to get a single test file for simple tests
fn get_simple_test_file(extension: &str) -> PathBuf {
    // Use the first available test file from our dynamic discovery
    let test_files = get_test_files(extension);
    test_files.first().unwrap().clone()
}

/// Helper function to get the CLI binary command
fn cli_command() -> Command {
    Command::new(assert_cmd::cargo::cargo_bin!("iam-policy-autopilot"))
}

/// Helper function to get the CLI binary command with extract-sdk-calls subcommand
fn extract_sdk_calls_command() -> Command {
    let mut cmd = cli_command();
    cmd.arg("extract-sdk-calls");
    cmd
}

/// Helper function to get the CLI binary command with generate-policies subcommand
fn generate_policy_command() -> Command {
    let mut cmd = cli_command();
    cmd.arg("generate-policies");
    cmd
}

#[test]
fn test_cli_help() {
    cli_command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Unified tool that combines IAM policy generation",
        ))
        .stdout(predicate::str::contains("fix-access-denied"))
        .stdout(predicate::str::contains("generate-policies"));
}

#[test]
fn test_cli_version() {
    cli_command()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_cli_no_arguments() {
    cli_command()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"));
}

#[test]
fn test_extract_sdk_calls_help() {
    extract_sdk_calls_command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Extracts AWS SDK method calls from source code files",
        ))
        .stdout(predicate::str::contains("--pretty"))
        .stdout(predicate::str::contains("--full-output"));
}

#[test]
fn test_generate_policy_help() {
    generate_policy_command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Generates complete IAM policy documents from source files",
        ))
        .stdout(predicate::str::contains("--pretty"));
}

#[test]
fn test_extract_sdk_calls_nonexistent_file() {
    extract_sdk_calls_command()
        .arg("/nonexistent/file.py")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Source file does not exist"));
}

#[test]
fn test_generate_policy_nonexistent_file() {
    generate_policy_command()
        .arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg("/nonexistent/file.py")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Source file does not exist"));
}

#[test]
fn test_extract_sdk_calls_simplified_output() {
    let test_file = get_simple_test_file("py");

    // Test with a single file - default simplified output
    let mut cmd = extract_sdk_calls_command();
    cmd.arg(test_file.to_str().unwrap());

    let output = cmd.assert().success();

    // Verify JSON output structure for simplified operations (Vec<OperationWithPossibleServices>)
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an array of operations
    assert!(json.is_array(), "Output should be an array of operations");

    // If there are operations, verify their structure
    if let Some(operations) = json.as_array() {
        for operation in operations {
            // Each operation should have 'Name' and 'PossibleServices' fields
            assert!(
                operation.get("Name").is_some(),
                "Operation should have 'Name' field"
            );
            assert!(
                operation.get("PossibleServices").is_some(),
                "Operation should have 'PossibleServices' field"
            );

            // PossibleServices should be an array
            let possible_services = operation.get("PossibleServices").unwrap();
            assert!(
                possible_services.is_array(),
                "PossibleServices should be an array"
            );
        }
    }
}

#[test]
fn test_extract_sdk_calls_full_output() {
    let test_file = get_simple_test_file("py");

    // Test with --full-output flag
    let mut cmd = extract_sdk_calls_command();
    cmd.arg("--full-output");
    cmd.arg(test_file.to_str().unwrap());

    let output = cmd.assert().success();

    // Verify JSON output structure - with --full-output, should be array with Metadata
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // With --full-output, should be an array of SdkMethodCall with Metadata
    assert!(
        json.is_array(),
        "Output should be an array of method calls with metadata"
    );

    // If there are method calls, verify they have Metadata
    if let Some(methods) = json.as_array() {
        for method in methods {
            assert!(
                method.get("Name").is_some(),
                "Method should have 'Name' field"
            );
            assert!(
                method.get("PossibleServices").is_some(),
                "Method should have 'PossibleServices' field"
            );
            // With --full-output, methods should include Metadata
            assert!(
                method.get("Metadata").is_some(),
                "Method should have 'Metadata' field with --full-output"
            );
        }
    }
}

#[test]
fn test_generate_policy_basic_functionality() {
    let test_file = get_simple_test_file("py");

    // Test with a single file
    let mut cmd = generate_policy_command();
    cmd.arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg(test_file.to_str().unwrap());

    let output = cmd.assert().success();

    // Verify JSON output structure - should be an object with policies array
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an object with policies field
    assert!(json.is_object(), "Output should be an object");
    assert!(
        json.get("Policies").is_some(),
        "Output should have 'policies' field"
    );
    let policies = json.get("Policies").unwrap();
    assert!(
        policies.is_array(),
        "Policies should be an array of IAM policies"
    );
}

#[test]
fn test_extract_sdk_calls_empty_file_simplified() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let empty_file = temp_dir.path().join("empty.py");
    fs::write(&empty_file, "").expect("Failed to create empty file");

    let output = extract_sdk_calls_command()
        .arg(empty_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value =
        serde_json::from_str(&stdout).expect("Should produce valid JSON even for empty files");

    // Should be an empty array for simplified output
    assert!(json.is_array(), "Empty file should produce empty array");
    assert_eq!(
        json.as_array().unwrap().len(),
        0,
        "Empty file should produce empty array"
    );
}

#[test]
fn test_extract_sdk_calls_empty_file_full_output() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let empty_file = temp_dir.path().join("empty.py");
    fs::write(&empty_file, "").expect("Failed to create empty file");

    let output = extract_sdk_calls_command()
        .arg("--full-output")
        .arg(empty_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value =
        serde_json::from_str(&stdout).expect("Should produce valid JSON even for empty files");

    // Should be an empty array for empty file with --full-output
    assert!(
        json.is_array(),
        "Empty file should produce empty array even with --full-output"
    );
    assert_eq!(
        json.as_array().unwrap().len(),
        0,
        "Empty file should produce empty array"
    );
}

#[test]
fn test_generate_policy_empty_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let empty_file = temp_dir.path().join("empty.py");
    fs::write(&empty_file, "").expect("Failed to create empty file");

    let output = generate_policy_command()
        .arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg(empty_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let _json: Value =
        serde_json::from_str(&stdout).expect("Should produce valid JSON even for empty files");
}

#[test]
fn test_comprehensive_real_files_extract_sdk_calls_python() {
    test_comprehensive_real_files_extract_sdk_calls_for_extension("py");
}

#[test]
fn test_comprehensive_real_files_extract_sdk_calls_go() {
    test_comprehensive_real_files_extract_sdk_calls_for_extension("go");
}

#[test]
fn test_comprehensive_real_files_extract_sdk_calls_typescript() {
    test_comprehensive_real_files_extract_sdk_calls_for_extension("ts");
}

#[test]
fn test_comprehensive_real_files_extract_sdk_calls_javascript() {
    test_comprehensive_real_files_extract_sdk_calls_for_extension("js");
}

fn test_comprehensive_real_files_extract_sdk_calls_for_extension(extension: &str) {
    let test_files = get_test_files(extension);

    // Skip test if no files with this extension exist
    if test_files.is_empty() {
        println!("No test files found with extension: {}", extension);
        return;
    }

    // Test extract-sdk-calls with multiple real files
    let mut cmd = extract_sdk_calls_command();
    for file in &test_files {
        cmd.arg(file.to_str().unwrap());
    }

    let output = cmd.assert().success();

    // Verify JSON output structure
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an array of operations
    assert!(json.is_array(), "Output should be an array of operations");

    // Should have found some operations from the real test files
    let operations = json.as_array().unwrap();
    assert!(
        !operations.is_empty(),
        "Should find AWS SDK operations in {} test files",
        extension
    );

    // Verify each operation has the expected structure
    for operation in operations {
        assert!(
            operation.get("Name").is_some(),
            "Operation should have 'Name' field"
        );
        assert!(
            operation.get("PossibleServices").is_some(),
            "Operation should have 'PossibleServices' field"
        );

        let possible_services = operation.get("PossibleServices").unwrap();
        assert!(
            possible_services.is_array(),
            "PossibleServices should be an array"
        );
        assert!(
            !possible_services.as_array().unwrap().is_empty(),
            "Should have at least one possible service"
        );
    }
}

#[test]
fn test_comprehensive_real_files_generate_policy_python() {
    test_comprehensive_real_files_generate_policy_for_extension("py");
}

#[test]
fn test_comprehensive_real_files_generate_policy_go() {
    test_comprehensive_real_files_generate_policy_for_extension("go");
}

#[test]
fn test_comprehensive_real_files_generate_policy_typescript() {
    test_comprehensive_real_files_generate_policy_for_extension("ts");
}

#[test]
fn test_comprehensive_real_files_generate_policy_javascript() {
    test_comprehensive_real_files_generate_policy_for_extension("js");
}

fn test_comprehensive_real_files_generate_policy_for_extension(extension: &str) {
    let test_files = get_test_files(extension);

    // Skip test if no files with this extension exist
    if test_files.is_empty() {
        println!("No test files found with extension: {}", extension);
        return;
    }

    // Test generate-policies with multiple real files
    let mut cmd = generate_policy_command();
    cmd.arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg("--pretty");

    for file in &test_files {
        cmd.arg(file.to_str().unwrap());
    }

    let output = cmd.assert().success();

    // Verify JSON output structure
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an object with policies field
    assert!(json.is_object(), "Output should be an object");
    assert!(
        json.get("Policies").is_some(),
        "Output should have 'policies' field"
    );
    let policies_value = json.get("Policies").unwrap();
    assert!(
        policies_value.is_array(),
        "Policies should be an array of IAM policies"
    );

    // Should have generated at least one policy from the real test files
    let policies = policies_value.as_array().unwrap();
    assert!(
        !policies.is_empty(),
        "Should generate IAM policies from {} test files",
        extension
    );

    // Verify each policy has the expected IAM policy structure
    for policy_with_type in policies {
        assert!(
            policy_with_type.get("Policy").is_some(),
            "Should have 'Policy' field"
        );
        assert!(
            policy_with_type.get("PolicyType").is_some(),
            "Should have 'PolicyType' field"
        );

        let policy = policy_with_type.get("Policy").unwrap();
        assert!(
            policy.get("Version").is_some(),
            "Policy should have 'Version' field"
        );
        assert!(
            policy.get("Statement").is_some(),
            "Policy should have 'Statement' field"
        );

        let statements = policy.get("Statement").unwrap();
        assert!(statements.is_array(), "Statement should be an array");

        // Verify statement structure
        for statement in statements.as_array().unwrap() {
            assert!(
                statement.get("Effect").is_some(),
                "Statement should have 'Effect' field"
            );
            assert!(
                statement.get("Action").is_some(),
                "Statement should have 'Action' field"
            );
            assert!(
                statement.get("Resource").is_some(),
                "Statement should have 'Resource' field"
            );
        }
    }
}

#[test]
fn test_disambiguation_example_file() {
    let disambiguation_file = PathBuf::from("tests/resources/test_disambiguation_example.py");

    // Test extract-sdk-calls with the disambiguation example file
    let mut cmd = extract_sdk_calls_command();
    cmd.arg("--full-output");
    cmd.arg(disambiguation_file.to_str().unwrap());

    let output = cmd.assert().success();

    // Verify JSON output structure
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an array of method calls with Metadata
    assert!(json.is_array(), "Output should be an array of method calls");

    let methods = json.as_array().unwrap();
    if !methods.is_empty() {
        // Verify that methods have Metadata (since we used --full-output)
        for method in methods {
            assert!(
                method.get("Name").is_some(),
                "Method should have 'Name' field"
            );
            assert!(
                method.get("PossibleServices").is_some(),
                "Method should have 'PossibleServices' field"
            );
            assert!(
                method.get("Metadata").is_some(),
                "Method should have 'Metadata' field with --full-output"
            );
        }
    }
}

#[test]
fn test_generate_policy() {
    // Test that verifies condition placeholders like ${region} are properly replaced
    // This test specifically validates the ConditionValueProcessor functionality
    let test_file = PathBuf::from("tests/resources/test_example.py");

    let output = generate_policy_command()
        .arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg("--pretty")
        .arg(test_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Verify the overall structure matches expected output
    assert!(json.is_object(), "Output should be an object");
    assert!(
        json.get("Policies").is_some(),
        "Output should have 'Policies' field"
    );
    let policies_with_types = json.get("Policies").unwrap().as_array().unwrap();
    assert!(
        !policies_with_types.is_empty(),
        "Should generate at least one policy"
    );

    let policy_with_type = &policies_with_types[0];
    let policy = policy_with_type.get("Policy").unwrap();
    assert_eq!(
        policy.get("Version").unwrap().as_str().unwrap(),
        "2012-10-17"
    );

    let statements = policy.get("Statement").unwrap().as_array().unwrap();
    assert!(!statements.is_empty(), "Should have at least one statement");

    // Find the statements with conditions
    let statements_with_conditions: Vec<_> = statements
        .iter()
        .filter(|s| s.get("Condition").is_some())
        .collect();

    println!("{}", serde_json::to_string_pretty(policy).unwrap());
    assert!(
        !statements_with_conditions.is_empty(),
        "Should have at least one statement with conditions"
    );

    // Track if we found the statement with both kms:Decrypt and kms:GenerateDataKey
    let mut found_statement_with_both_actions = false;

    for statement_with_condition in &statements_with_conditions {
        let condition = statement_with_condition.get("Condition").unwrap();
        let string_equals = condition.get("StringEquals").unwrap();
        let kms_via_service = string_equals
            .get("kms:ViaService")
            .unwrap()
            .as_array()
            .unwrap();

        // Verify that the ${region} placeholder was replaced with the actual region
        assert_eq!(
            kms_via_service.len(),
            1,
            "Should have exactly one kms:ViaService condition value"
        );
        let condition_value = kms_via_service[0].as_str().unwrap();

        // The condition value should contain the actual region (us-east-1) instead of ${region}
        assert!(
            condition_value.contains("us-east-1"),
            "Condition value '{}' should contain the actual region 'us-east-1'",
            condition_value
        );
        assert!(
            condition_value.contains("amazonaws.com"),
            "Condition value '{}' should contain 'amazonaws.com'",
            condition_value
        );
        assert!(
            !condition_value.contains("${region}"),
            r"Condition value '{}' should not contain the placeholder '${{region}}'",
            condition_value
        );

        // Get the actions for this statement
        let actions = statement_with_condition
            .get("Action")
            .unwrap()
            .as_array()
            .unwrap();
        let action_strings: Vec<&str> = actions.iter().map(|a| a.as_str().unwrap()).collect();

        // Check if this statement has both kms:Decrypt and kms:GenerateDataKey
        if action_strings.contains(&"kms:Decrypt")
            && action_strings.contains(&"kms:GenerateDataKey")
        {
            found_statement_with_both_actions = true;

            // Verify the expected resources are present for the statement with both actions
            let resources = statement_with_condition
                .get("Resource")
                .unwrap()
                .as_array()
                .unwrap();
            let expected_resources = vec!["arn:aws:kms:us-east-1:123456789012:key/*"];

            assert_eq!(
                resources.len(),
                expected_resources.len(),
                "Should have expected number of resources"
            );
            for expected_resource in &expected_resources {
                assert!(
                    resources
                        .iter()
                        .any(|r| r.as_str().unwrap() == *expected_resource),
                    "Should contain resource: {}",
                    expected_resource
                );
            }
        }

        // All statements with conditions should have at least kms:Decrypt
        assert!(
            action_strings.contains(&"kms:Decrypt"),
            "All statements with conditions should contain kms:Decrypt action"
        );
    }

    // Ensure we found at least one statement with both actions
    assert!(found_statement_with_both_actions,
        "Should have at least one statement with condition that contains both kms:Decrypt and kms:GenerateDataKey actions");
}

#[test]
fn test_generate_policy_us_gov_region() {
    // Test that when a aws-us-gov partition region is specified, the output resources also use it
    let test_file = PathBuf::from("tests/resources/test_example.py");

    let output = generate_policy_command()
        .arg("--region")
        .arg("us-gov-east-1")
        .arg("--account")
        .arg("123456789012")
        .arg("--pretty")
        .arg(test_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    let policies_with_types = json.get("Policies").unwrap().as_array().unwrap();
    let policy_with_type = &policies_with_types[0];
    let policy = policy_with_type.get("Policy").unwrap();
    let statements = policy.get("Statement").unwrap().as_array().unwrap();

    for statement in statements {
        if let Some(condition) = statement.get("Condition") {
            for (_, condition_tests) in condition.as_object().unwrap() {
                for (_, condition_values) in condition_tests.as_object().unwrap() {
                    for condition_value in condition_values.as_array().unwrap() {
                        let condition_value = condition_value.as_str().unwrap();
                        // The service principal name should be for the specified region
                        assert!(condition_value.ends_with(".us-gov-east-1.amazonaws.com"));
                    }
                }
            }
        }

        let resources = statement.get("Resource").unwrap().as_array().unwrap();
        for resource in resources.into_iter().map(|r| r.as_str().unwrap()) {
            if resource.starts_with("arn:") {
                assert!(resource.contains("arn:aws-us-gov:"));
                assert!(
                    resource.contains(":us-gov-east-1:123456789012:") || resource.contains(":::")
                );
            }
        }
    }
}

#[test]
fn test_generate_policy_wildcard_region() {
    // Test that when a wildcard region is specified, the output resources are generic over
    // partitions and regions
    let test_file = PathBuf::from("tests/resources/test_example.py");

    let output = generate_policy_command()
        .arg("--region")
        .arg("*")
        .arg("--account")
        .arg("123456789012")
        .arg("--pretty")
        .arg(test_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    let policies_with_types = json.get("Policies").unwrap().as_array().unwrap();
    let policy_with_type = &policies_with_types[0];
    let policy = policy_with_type.get("Policy").unwrap();
    let statements = policy.get("Statement").unwrap().as_array().unwrap();

    for statement in statements {
        if let Some(condition) = statement.get("Condition") {
            for (_, condition_tests) in condition.as_object().unwrap() {
                for (_, condition_values) in condition_tests.as_object().unwrap() {
                    for condition_value in condition_values.as_array().unwrap() {
                        let condition_value = condition_value.as_str().unwrap();
                        // All service principal names across all partitions end in .amazonaws.com.
                        // Region part should be as we specified (wildcard).
                        // See https://github.com/awslabs/iam-policy-autopilot/pull/103#discussion_r2753125558
                        assert!(condition_value.ends_with(".*.amazonaws.com"));
                    }
                }
            }
        }

        let resources = statement.get("Resource").unwrap().as_array().unwrap();
        for resource in resources.into_iter().map(|r| r.as_str().unwrap()) {
            if resource.starts_with("arn:") {
                // ARN should be generic across all partitions
                assert!(resource.contains("arn:*:"));
                // If ARN specifies a region, it should use what we provided (wildcard)
                assert!(resource.contains(":*:123456789012:") || resource.contains(":::"));
            }
        }
    }
}

#[test]
fn test_generate_policy_wildcard_account() {
    // Test that when a wildcard account ID is specified, the output resources use it.
    let test_file = PathBuf::from("tests/resources/test_example.py");

    let output = generate_policy_command()
        .arg("--region")
        .arg("us-east-1")
        .arg("--account")
        .arg("*")
        .arg("--pretty")
        .arg(test_file.to_str().unwrap())
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    let policies_with_types = json.get("Policies").unwrap().as_array().unwrap();
    let policy_with_type = &policies_with_types[0];
    let policy = policy_with_type.get("Policy").unwrap();
    let statements = policy.get("Statement").unwrap().as_array().unwrap();

    for statement in statements {
        let resources = statement.get("Resource").unwrap().as_array().unwrap();
        for resource in resources.into_iter().map(|r| r.as_str().unwrap()) {
            if resource.starts_with("arn:") {
                // If ARN specifies an account ID, it should use what we provided (wildcard)
                assert!(resource.contains(":us-east-1:*:") || resource.contains(":::"));
            }
        }
    }
}

#[test]
fn test_dictionary_unpacking_file() {
    let unpacking_file = PathBuf::from("tests/resources/test_dictionary_unpacking.py");

    // Test extract-sdk-calls with the dictionary unpacking example file
    let mut cmd = extract_sdk_calls_command();
    cmd.arg(unpacking_file.to_str().unwrap());

    let output = cmd.assert().success();

    // Verify JSON output structure
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("Invalid JSON output");

    // Should be an array of operations
    assert!(json.is_array(), "Output should be an array of operations");

    // The dictionary unpacking file should produce some operations
    let operations = json.as_array().unwrap();
    if !operations.is_empty() {
        // Verify structure
        for operation in operations {
            assert!(
                operation.get("Name").is_some(),
                "Operation should have 'Name' field"
            );
            assert!(
                operation.get("PossibleServices").is_some(),
                "Operation should have 'PossibleServices' field"
            );
        }
    }
}
