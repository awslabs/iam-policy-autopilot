//! Integration tests for the Terraform resource binding feature.
//!
//! These tests verify the end-to-end pipeline: parsing Terraform HCL,
//! resolving resources to IAM services, building resource bindings, and
//! substituting concrete resource ARNs into enriched SDK calls.

use std::path::{Path, PathBuf};

use iam_policy_autopilot_policy_generation::ServiceReferenceLoader;
use iam_policy_autopilot_policy_generation::extraction::terraform::hcl_parser::parse_terraform_directory;
use iam_policy_autopilot_policy_generation::enrichment::terraform::resource_binder::TerraformResourceResolver;
use iam_policy_autopilot_policy_generation::extraction::terraform::{
    AttributeValue, TerraformParseResult, TerraformResource,
};
use iam_policy_autopilot_policy_generation::extraction::terraform::state_parser::parse_terraform_state;

use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path};

/// Mock loader with ARN patterns for the services used in the sample fixtures.
///
/// Returns `(MockServer, ServiceReferenceLoader)`. The `MockServer` must be
/// held alive (via `_server` binding) for the duration of the test — dropping
/// it shuts down the HTTP endpoints the loader depends on.
async fn mock_loader() -> (MockServer, ServiceReferenceLoader) {
    let mock_server = MockServer::start().await;
    let url = mock_server.uri();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"service": "s3",       "url": format!("{url}/s3.json")},
            {"service": "dynamodb", "url": format!("{url}/dynamodb.json")},
            {"service": "sqs",      "url": format!("{url}/sqs.json")},
            {"service": "lambda",   "url": format!("{url}/lambda.json")},
            {"service": "iam",      "url": format!("{url}/iam.json")}
        ])))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET")).and(path("/s3.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Name": "s3", "Actions": [], "Resources": [
                {"Name": "bucket", "ARNFormats": ["arn:${Partition}:s3:::${BucketName}"]},
                {"Name": "object",  "ARNFormats": ["arn:${Partition}:s3:::${BucketName}/${ObjectName}"]}
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

    let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap().with_mapping_url(url);
    (mock_server, loader)
}

// ---------------------------------------------------------------------------
// Helper: path to the sample fixtures
// ---------------------------------------------------------------------------
fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("terraform_sample")
}

fn vars_sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("terraform_vars_sample")
}

// ===================================================================
// 1. Terraform Parsing Integration Tests
// ===================================================================

#[test]
fn test_parse_sample_terraform_directory() {
    let result = parse_terraform_directory(&sample_dir()).expect("should parse sample dir");

    assert!(
        result.resources.len() >= 5,
        "Expected at least 5 resources, got {}",
        result.resources.len()
    );

    let types: Vec<&str> = result.resources.values().map(|r| r.resource_type.as_str()).collect();
    assert!(types.contains(&"aws_s3_bucket"), "Missing aws_s3_bucket");
    assert!(types.contains(&"aws_dynamodb_table"), "Missing aws_dynamodb_table");
    assert!(types.contains(&"aws_sqs_queue"), "Missing aws_sqs_queue");
    assert!(types.contains(&"aws_lambda_function"), "Missing aws_lambda_function");
    assert!(types.contains(&"aws_iam_role"), "Missing aws_iam_role");

    assert!(
        result.warnings.is_empty(),
        "Unexpected warnings: {:?}",
        result.warnings
    );
}

#[test]
fn test_parse_extracts_correct_bucket_names() {
    let result = parse_terraform_directory(&sample_dir()).expect("parse");

    let buckets: Vec<&TerraformResource> = result
        .resources
        .values()
        .filter(|r| r.resource_type == "aws_s3_bucket")
        .collect();

    assert_eq!(buckets.len(), 2);

    let bucket_names: Vec<&str> = buckets
        .iter()
        .filter_map(|r| r.attributes.get("bucket"))
        .map(|v| v.as_str())
        .collect();

    assert!(bucket_names.contains(&"my-app-data-bucket"));
    assert!(bucket_names.contains(&"my-app-logs-bucket"));
}

#[test]
fn test_parse_extracts_dynamodb_table_name() {
    let result = parse_terraform_directory(&sample_dir()).expect("parse");

    let tables: Vec<&TerraformResource> = result
        .resources
        .values()
        .filter(|r| r.resource_type == "aws_dynamodb_table")
        .collect();

    assert_eq!(tables.len(), 1);
    assert_eq!(
        tables[0].attributes.get("name"),
        Some(&AttributeValue::Literal("users-table".to_string()))
    );
}

// ===================================================================
// 2. Resource Resolver Integration Tests
// ===================================================================

#[tokio::test]
async fn test_resource_resolver_from_sample_terraform() {
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("should resolve");

    assert!(!resolver.is_empty(), "Resolver should have resources");

    // Check S3 buckets
    let s3_buckets = &resolver.resources()[&("s3".to_string(), "bucket".to_string())];
    assert_eq!(s3_buckets.len(), 2);
    let bucket_names: Vec<&str> = s3_buckets
        .iter()
        .filter_map(|r| r.binding_name.as_deref())
        .collect();
    assert!(bucket_names.contains(&"my-app-data-bucket"));
    assert!(bucket_names.contains(&"my-app-logs-bucket"));

    // Check DynamoDB table
    let ddb_tables = &resolver.resources()[&("dynamodb".to_string(), "table".to_string())];
    assert_eq!(ddb_tables.len(), 1);
    assert_eq!(ddb_tables[0].binding_name.as_deref(), Some("users-table"));

    // Check SQS queue
    let sqs_queues = &resolver.resources()[&("sqs".to_string(), "queue".to_string())];
    assert_eq!(sqs_queues.len(), 1);
    assert_eq!(sqs_queues[0].binding_name.as_deref(), Some("task-processing-queue"));

    // Check Lambda function
    let lambda_fns = &resolver.resources()[&("lambda".to_string(), "function".to_string())];
    assert_eq!(lambda_fns.len(), 1);
    assert_eq!(lambda_fns[0].binding_name.as_deref(), Some("data-processor"));

    // Check IAM role
    let iam_roles = &resolver.resources()[&("iam".to_string(), "role".to_string())];
    assert_eq!(iam_roles.len(), 1);
    assert_eq!(iam_roles[0].binding_name.as_deref(), Some("data-processor-role"));
}

#[tokio::test]
async fn test_arn_substitution_s3_bucket() {
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
    let substituted = resolver
        .substitute_arn_patterns_for_test("s3", "bucket", &patterns)
        .expect("should substitute");

    assert_eq!(substituted.len(), 2);
    assert!(substituted.contains(&"arn:${Partition}:s3:::my-app-data-bucket".to_string()));
    assert!(substituted.contains(&"arn:${Partition}:s3:::my-app-logs-bucket".to_string()));
}

#[tokio::test]
async fn test_arn_substitution_dynamodb_table() {
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec![
        "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string(),
    ];
    let substituted = resolver
        .substitute_arn_patterns_for_test("dynamodb", "table", &patterns)
        .expect("should substitute");

    assert_eq!(substituted.len(), 1);
    assert_eq!(
        substituted[0],
        "arn:${Partition}:dynamodb:${Region}:${Account}:table/users-table"
    );
}

#[tokio::test]
async fn test_arn_substitution_sqs_queue() {
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec![
        "arn:${Partition}:sqs:${Region}:${Account}:${QueueName}".to_string(),
    ];
    let substituted = resolver
        .substitute_arn_patterns_for_test("sqs", "queue", &patterns)
        .expect("should substitute");

    assert_eq!(substituted.len(), 1);
    assert_eq!(
        substituted[0],
        "arn:${Partition}:sqs:${Region}:${Account}:task-processing-queue"
    );
}

// ===================================================================
// 3. Expression Handling Tests
// ===================================================================

#[tokio::test]
async fn test_expression_attributes_produce_wildcard_binding() {
    let hcl = r#"
resource "aws_s3_bucket" "dynamic" {
  bucket = var.bucket_name
}
"#;
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("main.tf"), hcl).expect("write");

    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        tmp.path(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
    let substituted = resolver.substitute_arn_patterns_for_test("s3", "bucket", &patterns);
    assert!(
        substituted.is_none(),
        "Single wildcard binding should return None (no improvement over default)"
    );
}

#[tokio::test]
async fn test_mixed_literal_and_expression_bindings() {
    let hcl = r#"
resource "aws_s3_bucket" "concrete" {
  bucket = "known-bucket"
}

resource "aws_s3_bucket" "dynamic" {
  bucket = var.bucket_name
}
"#;
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("main.tf"), hcl).expect("write");

    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        tmp.path(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
    let substituted = resolver
        .substitute_arn_patterns_for_test("s3", "bucket", &patterns)
        .expect("should substitute (has at least one concrete)");

    assert_eq!(substituted.len(), 2);
    assert!(substituted.contains(&"arn:${Partition}:s3:::known-bucket".to_string()));
    assert!(substituted.contains(&"arn:${Partition}:s3:::*".to_string()));
}

// ===================================================================
// 4. Serialization Round-Trip Test
// ===================================================================

#[test]
fn test_parse_result_json_roundtrip() {
    let result = parse_terraform_directory(&sample_dir()).expect("parse");

    let json = serde_json::to_string_pretty(&result).expect("serialize");
    let deserialized: TerraformParseResult =
        serde_json::from_str(&json).expect("deserialize");

    assert_eq!(result, deserialized);
}

// ===================================================================
// 5. Edge Cases
// ===================================================================

#[test]
fn test_empty_directory_produces_empty_result() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    let result = parse_terraform_directory(tmp.path()).expect("parse");

    assert!(result.resources.is_empty());
}

#[tokio::test]
async fn test_empty_directory_resolver_is_empty() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("empty.tf"), "").expect("write");

    let loader = ServiceReferenceLoader::empty_loader_for_tests().unwrap();
    let resolver = TerraformResourceResolver::from_directory(
        tmp.path(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    assert!(resolver.is_empty());
}

#[test]
fn test_directory_with_only_source_code_no_tf() {
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("app.py"), "import boto3").expect("write");

    let result = parse_terraform_directory(tmp.path()).expect("parse");
    assert!(result.resources.is_empty());
}

#[tokio::test]
async fn test_non_aws_resources_not_in_resolver() {
    let hcl = r#"
resource "google_storage_bucket" "gcs" {
  name = "my-gcs-bucket"
}

resource "aws_s3_bucket" "s3" {
  bucket = "my-s3-bucket"
}
"#;
    let tmp = tempfile::TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("main.tf"), hcl).expect("write");

    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        tmp.path(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    assert_eq!(resolver.len(), 1);
    let s3 = &resolver.resources()[&("s3".to_string(), "bucket".to_string())];
    assert_eq!(s3[0].binding_name.as_deref(), Some("my-s3-bucket"));
}

// ===================================================================
// 6. Terraform State File Integration Tests
// ===================================================================

#[test]
fn test_parse_sample_state_file() {
    let state_path = sample_dir().join("terraform.tfstate");
    let map = parse_terraform_state(&state_path).expect("should parse state file");

    assert!(
        map.len() >= 5,
        "Expected at least 5 state resource groups, got {}",
        map.len()
    );

    for resources in map.values() {
        for resource in resources {
            assert!(
                resource.arn.is_some(),
                "Resource {}.{} should have an ARN",
                resource.resource_type,
                resource.name
            );
        }
    }
}

#[test]
fn test_state_arns_are_exact() {
    let state_path = sample_dir().join("terraform.tfstate");
    let map = parse_terraform_state(&state_path).expect("parse");

    let s3_data = &map[&("aws_s3_bucket".into(), "data_bucket".into())][0];
    assert_eq!(
        s3_data.arn.as_deref(),
        Some("arn:aws:s3:::my-app-data-bucket")
    );

    let ddb = &map[&("aws_dynamodb_table".into(), "users_table".into())][0];
    assert_eq!(
        ddb.arn.as_deref(),
        Some("arn:aws:dynamodb:us-east-1:123456789012:table/users-table")
    );
}

#[test]
fn test_state_data_sources_skipped() {
    let state_path = sample_dir().join("terraform.tfstate");
    let map = parse_terraform_state(&state_path).expect("parse");

    assert!(
        !map.contains_key(&("aws_caller_identity".into(), "current".into())),
        "Data sources should be skipped in state parsing"
    );
}

#[tokio::test]
async fn test_state_bindings_take_precedence_over_hcl() {
    let state_path = sample_dir().join("terraform.tfstate");

    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        Some(&state_path),
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec!["arn:${Partition}:s3:::${BucketName}".to_string()];
    let substituted = resolver
        .substitute_arn_patterns_for_test("s3", "bucket", &patterns)
        .expect("should substitute");

    for arn in &substituted {
        assert!(
            !arn.contains("${Partition}"),
            "State-derived ARN should not contain placeholders: {arn}"
        );
        assert!(
            arn.starts_with("arn:aws:s3:::"),
            "State ARN should be a full S3 ARN: {arn}"
        );
    }

    assert_eq!(substituted.len(), 2);
    assert!(substituted.contains(&"arn:aws:s3:::my-app-data-bucket".to_string()));
    assert!(substituted.contains(&"arn:aws:s3:::my-app-logs-bucket".to_string()));
}

#[tokio::test]
async fn test_state_dynamodb_binding_is_full_arn() {
    let state_path = sample_dir().join("terraform.tfstate");

    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        Some(&state_path),
        &loader,
    )
    .await
    .expect("resolve");

    let patterns = vec![
        "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}".to_string(),
    ];
    let substituted = resolver
        .substitute_arn_patterns_for_test("dynamodb", "table", &patterns)
        .expect("should substitute");

    assert_eq!(substituted.len(), 1);
    assert_eq!(
        substituted[0],
        "arn:aws:dynamodb:us-east-1:123456789012:table/users-table"
    );
}

// ===================================================================
// 7. Variable Resolution Integration Tests
// ===================================================================

#[tokio::test]
async fn test_vars_sample_resolves_interpolations() {
    // terraform_vars_sample has:
    //   bucket = "${var.app_name}-${var.environment}-data"  (app_name=myapp default, environment overridden to "prod")
    //   name = var.table_name  (from tfvars: "users-prod")
    //   name = "${var.app_name}-tasks"  (app_name=myapp default)
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &vars_sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    // S3 bucket: "${var.app_name}-${var.environment}-data" → "myapp-prod-data"
    let s3_buckets = &resolver.resources()[&("s3".to_string(), "bucket".to_string())];
    assert_eq!(s3_buckets.len(), 1);
    assert_eq!(
        s3_buckets[0].binding_name.as_deref(),
        Some("myapp-prod-data"),
        "Interpolation should resolve with tfvars override"
    );

    // DynamoDB table: var.table_name → "users-prod" (from terraform.tfvars)
    let ddb_tables = &resolver.resources()[&("dynamodb".to_string(), "table".to_string())];
    assert_eq!(ddb_tables.len(), 1);
    assert_eq!(
        ddb_tables[0].binding_name.as_deref(),
        Some("users-prod"),
        "Bare var reference should resolve from tfvars"
    );

    // SQS queue: "${var.app_name}-tasks" → "myapp-tasks"
    let sqs_queues = &resolver.resources()[&("sqs".to_string(), "queue".to_string())];
    assert_eq!(sqs_queues.len(), 1);
    assert_eq!(
        sqs_queues[0].binding_name.as_deref(),
        Some("myapp-tasks"),
        "Interpolation should resolve with default value"
    );
}

// ===================================================================
// 8. Binding Explanations Integration Test
// ===================================================================

#[tokio::test]
async fn test_binding_explanations_from_hcl() {
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        None,
        &loader,
    )
    .await
    .expect("resolve");

    let explanations = resolver.build_binding_explanations();
    assert!(
        !explanations.is_empty(),
        "Should produce binding explanations for resolved resources"
    );

    // Each explanation should have a non-empty ARN and a valid terraform resource type
    for explanation in &explanations {
        assert!(!explanation.arn.is_empty(), "ARN should not be empty");
        assert!(
            explanation.terraform_resource_type.starts_with("aws_"),
            "Resource type should start with aws_: {}",
            explanation.terraform_resource_type
        );
        assert!(
            !explanation.location.is_empty(),
            "Location should not be empty"
        );
    }
}

#[tokio::test]
async fn test_binding_explanations_with_state_use_state_source() {
    let state_path = sample_dir().join("terraform.tfstate");
    let (_server, loader) = mock_loader().await;
    let resolver = TerraformResourceResolver::from_directory(
        &sample_dir(),
        Some(&state_path),
        &loader,
    )
    .await
    .expect("resolve");

    let explanations = resolver.build_binding_explanations();
    assert!(
        !explanations.is_empty(),
        "Should produce binding explanations"
    );

    // With state file, at least some explanations should have full AWS ARNs
    let state_explanations: Vec<_> = explanations
        .iter()
        .filter(|e| e.arn.starts_with("arn:aws:"))
        .collect();
    assert!(
        !state_explanations.is_empty(),
        "Should have state-derived ARN explanations"
    );
}

// ===================================================================
// 9. Error Handling Tests
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
    let result = TerraformResourceResolver::from_directory(
        tmp.path(),
        Some(&bad_state),
        &loader,
    )
    .await;

    assert!(result.is_err(), "Corrupt state file should produce an error");
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
    let result = TerraformResourceResolver::from_directory(
        tmp.path(),
        Some(&missing_state),
        &loader,
    )
    .await;

    assert!(result.is_err(), "Missing state file should produce an error");
}
