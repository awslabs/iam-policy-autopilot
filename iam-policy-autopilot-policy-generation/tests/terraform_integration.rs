//! Fixture-based integration tests for the Terraform resource binding feature.
//!
//! Each test scenario is a directory under `tests/resources/terraform_fixtures/`
//! containing:
//! - `.tf` files (the Terraform configuration input)
//! - `expected.json` (the expected resolver bindings and ARN substitutions)
//! - Optionally `terraform.tfstate` (for state-based tests)
//! - Optionally `variables.tf`, `terraform.tfvars` (for variable resolution tests)
//!
//! The test harness loads the fixture, builds a `TerraformResourceResolver`,
//! and asserts against the expected output using `assert_eq` on typed structures.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rstest::rstest;
use serde::Deserialize;

use iam_policy_autopilot_policy_generation::enrichment::terraform::resource_binder::TerraformResourceResolver;
use iam_policy_autopilot_policy_generation::ServiceReferenceLoader;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixture data model — deserialized from expected.json
// ---------------------------------------------------------------------------

/// Top-level fixture definition. All fields except `description` and `note`
/// are validated by the test harness — there are no silently-ignored fields.
#[derive(Debug, Deserialize)]
struct ExpectedFixture {
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    use_state_file: bool,
    /// Required — every fixture must declare expected resolver bindings.
    expected_resolver: ExpectedResolver,
    /// Required — completeness check ensures every binding has an ARN substitution entry.
    #[serde(default)]
    expected_arn_substitutions: Vec<ExpectedArnSubstitution>,
    #[serde(default)]
    expected_state_parse: Option<ExpectedStateParse>,
    #[serde(default)]
    expected_binding_explanations: Option<ExpectedBindingExplanations>,
}

#[derive(Debug, Deserialize)]
struct ExpectedResolver {
    resource_group_count: usize,
    bindings: Vec<ExpectedBinding>,
}

#[derive(Debug, Deserialize)]
struct ExpectedBinding {
    service: String,
    resource_type: String,
    binding_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedArnSubstitution {
    service: String,
    resource_type: String,
    input_patterns: Vec<String>,
    /// null means substitution should return None
    expected_output: Option<Vec<String>>,
    #[allow(dead_code)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedStateParse {
    managed_resource_count: usize,
    /// List of "type.name" keys that should NOT be present (data sources).
    #[serde(default)]
    excluded_keys: Vec<String>,
    /// Exact ARN values keyed by "type.name".
    #[serde(default)]
    specific_arns: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedBindingExplanations {
    /// Exact list of expected explanation ARNs (order-independent).
    /// When present and non-empty, the test asserts the actual ARN set equals this set exactly.
    expected_arns: Vec<String>,
    /// Expected locations keyed by ARN, in GNU format (e.g., "main.tf:5.1-5.39").
    /// Paths are relative to the fixture directory.
    #[serde(default)]
    expected_locations: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Mock service reference loader
// ---------------------------------------------------------------------------

/// Build a mock loader with ARN patterns for common AWS services.
async fn mock_loader() -> (MockServer, ServiceReferenceLoader) {
    let mock_server = MockServer::start().await;
    let url = mock_server.uri();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"service": "s3",         "url": format!("{url}/s3.json")},
            {"service": "dynamodb",   "url": format!("{url}/dynamodb.json")},
            {"service": "sqs",        "url": format!("{url}/sqs.json")},
            {"service": "lambda",     "url": format!("{url}/lambda.json")},
            {"service": "iam",        "url": format!("{url}/iam.json")},
            {"service": "kinesis",    "url": format!("{url}/kinesis.json")},
            {"service": "sns",        "url": format!("{url}/sns.json")}
        ])))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET")).and(path("/s3.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "s3", "Actions": [], "Resources": [
                {"Name": "bucket", "ARNFormats": ["arn:${Partition}:s3:::${BucketName}"]},
                {"Name": "object", "ARNFormats": ["arn:${Partition}:s3:::${BucketName}/${ObjectName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/dynamodb.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "dynamodb", "Actions": [], "Resources": [
                {"Name": "table", "ARNFormats": ["arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/sqs.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "sqs", "Actions": [], "Resources": [
                {"Name": "queue", "ARNFormats": ["arn:${Partition}:sqs:${Region}:${Account}:${QueueName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/lambda.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "lambda", "Actions": [], "Resources": [
                {"Name": "function", "ARNFormats": ["arn:${Partition}:lambda:${Region}:${Account}:function:${FunctionName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/iam.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "iam", "Actions": [], "Resources": [
                {"Name": "role", "ARNFormats": ["arn:${Partition}:iam::${Account}:role/${RoleName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/kinesis.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "kinesis", "Actions": [], "Resources": [
                {"Name": "stream", "ARNFormats": ["arn:${Partition}:kinesis:${Region}:${Account}:stream/${StreamName}"]}
            ]
        })))
        .mount(&mock_server).await;

    Mock::given(method("GET")).and(path("/sns.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "sns", "Actions": [], "Resources": [
                {"Name": "topic", "ARNFormats": ["arn:${Partition}:sns:${Region}:${Account}:${TopicName}"]}
            ]
        })))
        .mount(&mock_server).await;

    let loader = ServiceReferenceLoader::empty_loader_for_tests()
        .unwrap()
        .with_mapping_url(url);
    (mock_server, loader)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("terraform_fixtures")
}

fn load_expected(fixture_name: &str) -> ExpectedFixture {
    let path = fixtures_dir().join(fixture_name).join("expected.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", path.display()))
}

fn fixture_dir(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

/// Build a resolver from a fixture, using mock loader and optional state file.
async fn build_resolver(
    fixture_name: &str,
    expected: &ExpectedFixture,
    server_and_loader: &(MockServer, ServiceReferenceLoader),
) -> TerraformResourceResolver {
    let dir = fixture_dir(fixture_name);
    let tfstate_path = dir.join("terraform.tfstate");
    let tfstate_ref = if expected.use_state_file && tfstate_path.exists() {
        Some(tfstate_path)
    } else {
        None
    };

    TerraformResourceResolver::from_directory(&dir, tfstate_ref.as_ref(), &server_and_loader.1)
        .await
        .unwrap_or_else(|e| panic!("[{fixture_name}] Failed to build resolver: {e}"))
}

/// Extract sorted binding names from the resolver for a given (service, type) key.
fn actual_binding_names(
    resolver: &TerraformResourceResolver,
    service: &str,
    resource_type: &str,
) -> Vec<String> {
    let key = (service.to_string(), resource_type.to_string());
    match resolver.resources().get(&key) {
        Some(resources) => {
            let mut names: Vec<String> = resources
                .iter()
                .map(|r| r.binding_name.clone().unwrap_or_else(|| "*".to_string()))
                .collect();
            names.sort();
            names
        }
        None => vec![],
    }
}

// ===================================================================
// Fixture-driven resolver + ARN substitution tests (unified)
// ===================================================================

/// Test resolver bindings AND ARN substitutions for every fixture.
#[rstest]
#[case("basic_multi_resource")]
#[case("variable_resolution")]
#[case("mixed_providers")]
#[case("expression_unresolvable")]
#[case("sub_resource_fallback")]
#[case("mixed_concrete_and_expression")]
#[case("multi_service_real_world")]
#[case("state_precedence")]
#[tokio::test]
async fn test_fixture(#[case] fixture_name: &str) {
    let expected = load_expected(fixture_name);
    let sl = mock_loader().await;
    let resolver = build_resolver(fixture_name, &expected, &sl).await;

    // --- 1. Verify resolver bindings ---
    let expected_resolver = &expected.expected_resolver;

    assert_eq!(
        resolver.len(),
        expected_resolver.resource_group_count,
        "[{fixture_name}] resource group count mismatch"
    );

    for binding in &expected_resolver.bindings {
        let actual = actual_binding_names(&resolver, &binding.service, &binding.resource_type);
        let mut expected_names = binding.binding_names.clone();
        expected_names.sort();

        assert_eq!(
            actual, expected_names,
            "[{fixture_name}] binding names mismatch for {}.{}",
            binding.service, binding.resource_type
        );
    }

    // --- 2. Completeness check: every binding has an ARN substitution entry ---
    let arn_sub_keys: std::collections::HashSet<(&str, &str)> = expected
        .expected_arn_substitutions
        .iter()
        .map(|s| (s.service.as_str(), s.resource_type.as_str()))
        .collect();

    for binding in &expected_resolver.bindings {
        assert!(
            arn_sub_keys.contains(&(binding.service.as_str(), binding.resource_type.as_str())),
            "[{fixture_name}] binding {}.{} has no corresponding entry in expected_arn_substitutions — \
             add one to ensure ARN substitution is tested",
            binding.service, binding.resource_type
        );
    }

    // --- 3. Verify ARN substitutions ---
    for sub in &expected.expected_arn_substitutions {
        let actual = resolver.substitute_arn_patterns_for_test(
            &sub.service,
            &sub.resource_type,
            &sub.input_patterns,
        );

        match (&actual, &sub.expected_output) {
            (None, None) => {
                // Both None — substitution returns None as expected
            }
            (Some(actual_arns), Some(expected_arns)) => {
                let actual_set: BTreeSet<&str> = actual_arns.iter().map(String::as_str).collect();
                let expected_set: BTreeSet<&str> =
                    expected_arns.iter().map(String::as_str).collect();
                assert_eq!(
                    actual_set, expected_set,
                    "[{fixture_name}] ARN substitution mismatch for {}.{}\n  input:    {:?}\n  actual:   {:?}\n  expected: {:?}",
                    sub.service, sub.resource_type,
                    sub.input_patterns, actual_arns, expected_arns
                );
            }
            (None, Some(expected_arns)) => {
                panic!(
                    "[{fixture_name}] ARN substitution for {}.{} returned None, expected {:?}",
                    sub.service, sub.resource_type, expected_arns
                );
            }
            (Some(actual_arns), None) => {
                panic!(
                    "[{fixture_name}] ARN substitution for {}.{} returned {:?}, expected None",
                    sub.service, sub.resource_type, actual_arns
                );
            }
        }
    }

    // --- 4. Verify binding explanations (if specified) ---
    if let Some(ref expl_expected) = expected.expected_binding_explanations {
        let explanations = resolver.build_binding_explanations();

        // Exact ARN set comparison
        let actual_arns: BTreeSet<&str> = explanations.iter().map(|e| e.arn.as_str()).collect();
        let expected_arns: BTreeSet<&str> = expl_expected
            .expected_arns
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            actual_arns, expected_arns,
            "[{fixture_name}] binding explanation ARN set mismatch"
        );

        // Always validate structural fields
        for expl in &explanations {
            assert!(
                !expl.arn.is_empty(),
                "[{fixture_name}] explanation ARN should not be empty"
            );
            assert!(
                expl.resource_type.starts_with(
                    iam_policy_autopilot_policy_generation::extraction::terraform::AWS_RESOURCE_PREFIX
                ),
                "[{fixture_name}] resource type should start with aws_: {}",
                expl.resource_type
            );
            assert!(
                expl.location.start_line() > 0,
                "[{fixture_name}] location should have a valid start line"
            );
        }

        // Validate exact location values when specified in the fixture
        if !expl_expected.expected_locations.is_empty() {
            let fixture_path = fixture_dir(fixture_name);
            for expl in &explanations {
                if let Some(expected_gnu) = expl_expected.expected_locations.get(&expl.arn) {
                    // expected_gnu is relative to fixture dir (e.g., "main.tf:5.1-5.39")
                    // actual location has an absolute path; compare using just the filename and positions
                    let actual_gnu = expl.location.to_gnu_format();
                    // Strip the fixture dir prefix from the actual path for comparison
                    let actual_relative = actual_gnu
                        .strip_prefix(&format!("{}/", fixture_path.display()))
                        .unwrap_or(&actual_gnu);
                    assert_eq!(
                        actual_relative, expected_gnu,
                        "[{fixture_name}] location mismatch for ARN {}",
                        expl.arn
                    );
                }
            }
        }
    }
}

// ===================================================================
// State-specific invariant: no ${Partition} in state-derived ARNs
// ===================================================================

#[tokio::test]
async fn test_state_arns_have_no_placeholders() {
    let fixture_name = "state_precedence";
    let expected = load_expected(fixture_name);
    let sl = mock_loader().await;
    let resolver = build_resolver(fixture_name, &expected, &sl).await;

    for sub in &expected.expected_arn_substitutions {
        if let Some(actual) = resolver.substitute_arn_patterns_for_test(
            &sub.service,
            &sub.resource_type,
            &sub.input_patterns,
        ) {
            for arn in &actual {
                assert!(
                    !arn.contains("${Partition}")
                        && !arn.contains("${Region}")
                        && !arn.contains("${Account}"),
                    "[{fixture_name}] state ARN should not contain placeholders: {arn}"
                );
            }
        }
    }
}

// ===================================================================
// State file parsing tests
// ===================================================================

#[test]
fn test_state_file_parsing() {
    use iam_policy_autopilot_policy_generation::extraction::terraform::state_parser::parse_terraform_state;

    let fixture_name = "state_precedence";
    let expected = load_expected(fixture_name);
    let state_path = fixture_dir(fixture_name).join("terraform.tfstate");
    let map = parse_terraform_state(&state_path).expect("should parse state file");

    let state_expected = expected.expected_state_parse.as_ref().unwrap_or_else(|| {
        panic!("[{fixture_name}] expected_state_parse is required for state fixtures")
    });

    assert_eq!(
        map.len(),
        state_expected.managed_resource_count,
        "[{fixture_name}] state managed resource count mismatch"
    );

    // Verify excluded keys (data sources)
    for excluded_key in &state_expected.excluded_keys {
        let parts: Vec<&str> = excluded_key.splitn(2, '.').collect();
        assert_eq!(
            parts.len(),
            2,
            "excluded_keys entry must be 'type.name': {excluded_key}"
        );
        assert!(
            !map.contains_key(&(parts[0].to_string(), parts[1].to_string())),
            "[{fixture_name}] {excluded_key} should have been excluded (data source)"
        );
    }

    // Verify specific ARNs
    for (key, expected_arn) in &state_expected.specific_arns {
        let parts: Vec<&str> = key.splitn(2, '.').collect();
        assert_eq!(
            parts.len(),
            2,
            "specific_arns key must be 'type.name': {key}"
        );
        let resources = map
            .get(&(parts[0].to_string(), parts[1].to_string()))
            .unwrap_or_else(|| panic!("[{fixture_name}] state missing resource {key}"));
        assert_eq!(
            resources[0].arn.as_deref(),
            Some(expected_arn.as_str()),
            "[{fixture_name}] state ARN mismatch for {key}"
        );
    }
}

// ===================================================================
// Empty directory test
// ===================================================================

#[tokio::test]
async fn test_empty_directory_resolver_is_empty() {
    let dir = fixture_dir("empty_directory");
    let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
    let resolver = TerraformResourceResolver::from_directory(&dir, None, &loader)
        .await
        .expect("resolve empty dir");

    assert!(
        resolver.is_empty(),
        "Empty fixture directory should produce empty resolver"
    );
    assert_eq!(resolver.len(), 0);
}

// ===================================================================
// Error handling tests
// ===================================================================

#[tokio::test]
async fn test_corrupt_state_file_produces_error() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(
        tmp.path().join("main.tf"),
        r#"resource "aws_s3_bucket" "b" { bucket = "test" }"#,
    )
    .expect("write");
    let bad_state = tmp.path().join("terraform.tfstate");
    std::fs::write(&bad_state, "this is not valid JSON").expect("write");

    let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
    let result =
        TerraformResourceResolver::from_directory(tmp.path(), Some(&bad_state), &loader).await;

    assert!(
        result.is_err(),
        "Corrupt state file should produce an error"
    );
}

#[tokio::test]
async fn test_missing_state_file_produces_error() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(
        tmp.path().join("main.tf"),
        r#"resource "aws_s3_bucket" "b" { bucket = "test" }"#,
    )
    .expect("write");
    let missing_state = tmp.path().join("nonexistent.tfstate");

    let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
    let result =
        TerraformResourceResolver::from_directory(tmp.path(), Some(&missing_state), &loader).await;

    assert!(
        result.is_err(),
        "Missing state file should produce an error"
    );
}

// ===================================================================
// JSON round-trip test
// ===================================================================

#[test]
fn test_parse_result_json_roundtrip() {
    use iam_policy_autopilot_policy_generation::extraction::terraform::hcl_parser::parse_terraform_directory;
    use iam_policy_autopilot_policy_generation::extraction::terraform::TerraformParseResult;

    let dir = fixture_dir("basic_multi_resource");
    let result = parse_terraform_directory(&dir).expect("parse");

    let json = serde_json::to_string_pretty(&result).expect("serialize");
    let deserialized: TerraformParseResult = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(result, deserialized);
}
