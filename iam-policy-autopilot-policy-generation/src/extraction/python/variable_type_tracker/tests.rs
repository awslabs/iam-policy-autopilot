use super::types::*;
use crate::extraction::AstWithSourceFile;
use crate::SourceFile;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Python;
use rstest::rstest;

fn create_ast(source_code: &str) -> AstWithSourceFile<Python> {
    let source_file = SourceFile::with_language(
        std::path::PathBuf::new(),
        source_code.to_string(),
        crate::Language::Python,
    );
    let ast_grep = Python.ast_grep(&source_file.content);
    AstWithSourceFile::new(ast_grep, source_file)
}

// Test helper: convenience method that calls get_service_for_variable_in_context with no function context
impl VariableTypeTracker {
    fn get_service_for_variable(&self, var_name: &str) -> Option<&String> {
        self.get_service_for_variable_in_context(var_name, None)
    }
}

// ========== Direct Assignment Tracking Tests (parameterized) ==========

#[rstest]
#[case("boto3.client('s3')", "s3_client", "s3", SdkObjectKind::Client)]
#[case("boto3.client(\"ec2\")", "ec2_client", "ec2", SdkObjectKind::Client)]
#[case(
    "boto3.resource('dynamodb')",
    "dynamodb",
    "dynamodb",
    SdkObjectKind::Resource
)]
#[case("boto3.resource('s3')", "s3_res", "s3", SdkObjectKind::Resource)]
fn test_direct_assignment_tracking(
    #[case] rhs: &str,
    #[case] var_name: &str,
    #[case] expected_service: &str,
    #[case] expected_kind: SdkObjectKind,
) {
    let source_code = format!("import boto3\n{var_name} = {rhs}\n");
    let ast = create_ast(&source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let info = tracker
        .get_type_info_for_variable_in_context(var_name, None)
        .unwrap();
    assert_eq!(info.service_name, expected_service);
    assert_eq!(info.kind, Some(expected_kind));
}

#[test]
fn test_reassignment_in_same_scope() {
    let source_code = r#"
import boto3
s3 = boto3.client('s3')
s3 = boto3.client('ec2')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // Second assignment should win (last-write-wins, matching Python semantics)
    assert_eq!(
        tracker.get_service_for_variable("s3"),
        Some(&"ec2".to_string())
    );
}

#[test]
fn test_async_def_function() {
    let source_code = r#"
import boto3

async def upload(client):
    await client.put_object(Bucket='b', Key='k', Body=b'data')

s3 = boto3.client('s3')
upload(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let services = tracker.get_services_for_parameter("upload", "client");
    assert!(services.is_some());
    assert!(services.unwrap().contains("s3"));
}

#[test]
fn test_decorated_function() {
    let source_code = r#"
import boto3

@retry(max_attempts=3)
def upload(client):
    client.put_object(Bucket='b', Key='k', Body=b'data')

s3 = boto3.client('s3')
upload(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let services = tracker.get_services_for_parameter("upload", "client");
    assert!(services.is_some());
    assert!(services.unwrap().contains("s3"));
}

#[test]
fn test_paginator_waiter_not_tracked() {
    let source_code = r#"
import boto3
s3 = boto3.client('s3')
paginator = s3.get_paginator('list_objects_v2')
waiter = s3.get_waiter('object_exists')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // s3 should be tracked
    assert_eq!(
        tracker.get_service_for_variable("s3"),
        Some(&"s3".to_string())
    );

    // Paginator and waiter should NOT be tracked (handled by separate extractors)
    assert!(tracker
        .get_type_info_for_variable_in_context("paginator", None)
        .is_none());
    assert!(tracker
        .get_type_info_for_variable_in_context("waiter", None)
        .is_none());
}

#[test]
fn test_nested_function_assignment_does_not_leak_to_outer() {
    let source_code = r#"
import boto3

def outer():
    def inner():
        client = boto3.client('s3')
    client.something()
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // 'client' assigned inside inner() should NOT appear in outer()'s scope
    assert!(tracker
        .get_service_for_variable_in_context("client", Some("outer"))
        .is_none());

    // It should appear in inner()'s scope
    assert_eq!(
        tracker.get_service_for_variable_in_context("client", Some("inner")),
        Some(&"s3".to_string())
    );
}

#[test]
fn test_module_alias_available_in_function() {
    let source_code = r#"
import boto3
s3 = boto3.client('s3')
my_s3 = s3

def func(x):
    local = my_s3
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // Module-level alias should be resolved
    assert_eq!(
        tracker.get_service_for_variable("my_s3"),
        Some(&"s3".to_string())
    );

    // Function-level alias referencing module alias should also resolve
    assert_eq!(
        tracker.get_service_for_variable_in_context("local", Some("func")),
        Some(&"s3".to_string())
    );
}

#[test]
fn test_track_multiple_assignments() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
ec2_client = boto3.client('ec2')
dynamodb = boto3.resource('dynamodb')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("ec2_client"),
        Some(&"ec2".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("dynamodb"),
        Some(&"dynamodb".to_string())
    );
}

#[test]
fn test_unknown_variable() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(tracker.get_service_for_variable("unknown_var"), None);
}

#[test]
fn test_multi_arg_boto3_client_call() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3', region_name='us-east-1')
ec2_client = boto3.client('ec2', endpoint_url='http://localhost:4566')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("ec2_client"),
        Some(&"ec2".to_string())
    );
}

#[test]
fn test_nested_function_does_not_pollute_outer_context() {
    let source_code = r#"
import boto3

def helper(client):
    client.list_buckets()

def outer():
    client = boto3.client('s3')

    def inner(client):
        helper(client)

    ec2 = boto3.client('ec2')
    inner(ec2)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // helper's 'client' parameter should only have ec2 (from inner's call),
    // not s3 (from outer's scope being wrongly attributed)
    let services = tracker.get_services_for_parameter("helper", "client");
    assert!(services.is_some());
    let services = services.unwrap();
    assert!(services.contains("ec2"));
    assert!(!services.contains("s3"));
}

#[test]
fn test_variadic_params_excluded_from_positional_matching() {
    let source_code = r#"
import boto3

def func(client, *args, **kwargs):
    pass

s3 = boto3.client('s3')
ec2 = boto3.client('ec2')
func(s3, ec2)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // s3 should map to 'client' (position 0)
    let client_services = tracker.get_services_for_parameter("func", "client");
    assert!(client_services.is_some());
    assert!(client_services.unwrap().contains("s3"));

    // ec2 should NOT map to 'args' — variadic params are excluded
    assert!(tracker.get_services_for_parameter("func", "args").is_none());
    assert!(tracker
        .get_services_for_parameter("func", "kwargs")
        .is_none());
}

#[test]
fn test_real_world_scenario() {
    let source_code = r#"
import boto3

s3_direct = boto3.client('s3')
s3_direct.put_object(Bucket='bucket1', Key='key1', Body=b'data1')

def upload_data(client, bucket, key):
    client.put_object(Bucket=bucket, Key=key, Body=b'data2')

s3_client = boto3.client('s3')
upload_data(s3_client, 'bucket2', 'key2')

dynamodb_direct = boto3.resource('dynamodb')
table_direct = dynamodb_direct.Table('users')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_direct"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("dynamodb_direct"),
        Some(&"dynamodb".to_string())
    );
}

// ========== Alias Tracking Tests ==========

#[test]
fn test_simple_alias() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
my_client = s3_client
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("my_client"),
        Some(&"s3".to_string())
    );
}

#[test]
fn test_chained_aliases() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
client_a = s3_client
client_b = client_a
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("client_a"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("client_b"),
        Some(&"s3".to_string())
    );
}

// ========== Function Parameter Inference Tests ==========

#[test]
fn test_function_parameter_inference() {
    let source_code = r#"
import boto3

def upload_file(client):
    pass

s3_client = boto3.client('s3')
upload_file(s3_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let services = tracker.get_services_for_parameter("upload_file", "client");
    assert!(services.is_some());
    assert!(services.unwrap().contains("s3"));
}

#[test]
fn test_function_local_variable_passed_as_argument() {
    let source_code = r#"
import boto3

def helper(client):
    client.list_buckets()

def caller():
    local_client = boto3.client('s3')
    helper(local_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let services = tracker.get_services_for_parameter("helper", "client");
    assert!(
        services.is_some(),
        "should resolve function-local variable at call site"
    );
    assert!(services.unwrap().contains("s3"));
}

#[test]
fn test_function_parameter_multiple_types() {
    let source_code = r#"
import boto3

def process_data(client):
    pass

s3_client = boto3.client('s3')
ec2_client = boto3.client('ec2')

process_data(s3_client)
process_data(ec2_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let services = tracker.get_services_for_parameter("process_data", "client");
    assert!(services.is_some());
    let services = services.unwrap();
    assert_eq!(services.len(), 2);
    assert!(services.contains("s3"));
    assert!(services.contains("ec2"));
}

#[test]
fn test_ambiguous_parameter_returns_none() {
    let source_code = r#"
import boto3

def process_data(client):
    pass

s3_client = boto3.client('s3')
ec2_client = boto3.client('ec2')

process_data(s3_client)
process_data(ec2_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // Single-type API should return None for ambiguous parameters
    assert_eq!(
        tracker.get_service_for_variable_in_context("client", Some("process_data")),
        None
    );

    // Full type info API should also return None
    assert!(tracker
        .get_type_info_for_variable_in_context("client", Some("process_data"))
        .is_none());

    // But get_services_for_parameter still returns all possibilities
    let services = tracker.get_services_for_parameter("process_data", "client");
    assert!(services.is_some());
    let services = services.unwrap();
    assert_eq!(services.len(), 2);
    assert!(services.contains("s3"));
    assert!(services.contains("ec2"));
}

#[test]
fn test_multiple_function_calls() {
    let source_code = r#"
import boto3

def process_s3(client):
    pass

def process_dynamodb(table):
    pass

s3 = boto3.client('s3')
dynamodb = boto3.resource('dynamodb')

process_s3(s3)
process_dynamodb(dynamodb)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_services = tracker.get_services_for_parameter("process_s3", "client");
    assert!(s3_services.is_some());
    assert!(s3_services.unwrap().contains("s3"));

    let dynamodb_services = tracker.get_services_for_parameter("process_dynamodb", "table");
    assert!(dynamodb_services.is_some());
    assert!(dynamodb_services.unwrap().contains("dynamodb"));
}

#[test]
fn test_multiple_parameters() {
    let source_code = r#"
import boto3

def sync_data(s3_client, dynamodb_client):
    s3_client.get_object(Bucket='bucket', Key='key')
    dynamodb_client.put_item(TableName='table', Item={})

s3 = boto3.client('s3')
dynamodb = boto3.client('dynamodb')
sync_data(s3, dynamodb)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_services = tracker.get_services_for_parameter("sync_data", "s3_client");
    assert!(
        s3_services.is_some(),
        "s3_client parameter should be tracked"
    );
    assert!(s3_services.unwrap().contains("s3"));

    let dynamodb_services = tracker.get_services_for_parameter("sync_data", "dynamodb_client");
    assert!(
        dynamodb_services.is_some(),
        "dynamodb_client parameter should be tracked"
    );
    assert!(dynamodb_services.unwrap().contains("dynamodb"));
}

#[test]
fn test_three_parameters() {
    let source_code = r#"
import boto3

def process(s3, ec2, lambda_client):
    s3.list_buckets()
    ec2.describe_instances()
    lambda_client.list_functions()

s3 = boto3.client('s3')
ec2 = boto3.client('ec2')
lambda_client = boto3.client('lambda')
process(s3, ec2, lambda_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_services = tracker.get_services_for_parameter("process", "s3");
    assert!(s3_services.is_some());
    assert!(s3_services.unwrap().contains("s3"));

    let ec2_services = tracker.get_services_for_parameter("process", "ec2");
    assert!(ec2_services.is_some());
    assert!(ec2_services.unwrap().contains("ec2"));

    let lambda_services = tracker.get_services_for_parameter("process", "lambda_client");
    assert!(lambda_services.is_some());
    assert!(lambda_services.unwrap().contains("lambda"));
}

#[test]
fn test_mixed_parameters() {
    let source_code = r#"
import boto3

def upload(client, bucket_name, key):
    client.put_object(Bucket=bucket_name, Key=key, Body=b'data')

s3 = boto3.client('s3')
upload(s3, 'my-bucket', 'my-key')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let client_services = tracker.get_services_for_parameter("upload", "client");
    assert!(client_services.is_some());
    assert!(client_services.unwrap().contains("s3"));

    assert!(tracker
        .get_services_for_parameter("upload", "bucket_name")
        .is_none());
    assert!(tracker
        .get_services_for_parameter("upload", "key")
        .is_none());
}

// ========== Keyword Argument Tests ==========

#[test]
fn test_keyword_arguments_matched_by_name() {
    let source_code = r#"
import boto3

def sync_data(s3_client, dynamodb_client):
    s3_client.get_object(Bucket='bucket', Key='key')
    dynamodb_client.put_item(TableName='table', Item={})

s3 = boto3.client('s3')
ddb = boto3.client('dynamodb')
sync_data(dynamodb_client=ddb, s3_client=s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_services = tracker.get_services_for_parameter("sync_data", "s3_client");
    assert!(s3_services.is_some());
    assert!(s3_services.unwrap().contains("s3"));

    let dynamodb_services = tracker.get_services_for_parameter("sync_data", "dynamodb_client");
    assert!(dynamodb_services.is_some());
    assert!(dynamodb_services.unwrap().contains("dynamodb"));
}

#[test]
fn test_mixed_positional_and_keyword_arguments() {
    let source_code = r#"
import boto3

def process(client, table_name, region):
    pass

s3 = boto3.client('s3')
process(s3, region='us-east-1', table_name='my-table')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let client_services = tracker.get_services_for_parameter("process", "client");
    assert!(client_services.is_some());
    assert!(client_services.unwrap().contains("s3"));

    assert!(tracker
        .get_services_for_parameter("process", "table_name")
        .is_none());
    assert!(tracker
        .get_services_for_parameter("process", "region")
        .is_none());
}

// ========== self/cls Filtering Tests ==========

#[test]
fn test_self_filtered_from_parameter_mapping() {
    let source_code = r#"
import boto3

class Uploader:
    def upload(self, client):
        client.put_object(Bucket='b', Key='k', Body=b'data')

s3 = boto3.client('s3')
upload(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert!(tracker
        .get_services_for_parameter("upload", "self")
        .is_none());
    let client_services = tracker.get_services_for_parameter("upload", "client");
    assert!(client_services.is_some());
    assert!(client_services.unwrap().contains("s3"));
}

#[test]
fn test_cls_filtered_from_parameter_mapping() {
    let source_code = r#"
import boto3

class Factory:
    @classmethod
    def create(cls, client):
        pass

s3 = boto3.client('s3')
create(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert!(tracker
        .get_services_for_parameter("create", "cls")
        .is_none());
    let client_services = tracker.get_services_for_parameter("create", "client");
    assert!(client_services.is_some());
    assert!(client_services.unwrap().contains("s3"));
}

// ========== Helper Function Tests (parameterized) ==========

#[rstest]
#[case("client", &["client"])]
#[case("client, bucket, key", &["client", "bucket", "key"])]
#[case("self, client, table", &["self", "client", "table"])]
#[case("", &[])]
#[case("client=None, bucket='default'", &["client", "bucket"])]
#[case("client: str, count: int", &["client", "count"])]
#[case(" client , bucket ", &["client", "bucket"])]
#[case("client, bucket='default', key: str", &["client", "bucket", "key"])]
#[case("*args, **kwargs", &["args", "kwargs"])]
#[case("client, *args, **kwargs", &["client", "args", "kwargs"])]
#[case("*, client", &["client"])]
#[case("client, *, region", &["client", "region"])]
fn test_extract_all_params(#[case] input: &str, #[case] expected: &[&str]) {
    let result = VariableTypeTracker::extract_all_params(input);
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(result, expected);
}

// ========== SDK Object Kind Inference Tests (parameterized) ==========

#[rstest]
#[case("boto3.client('s3')", "s3_client", "s3", SdkObjectKind::Client)]
#[case("boto3.resource('s3')", "s3_resource", "s3", SdkObjectKind::Resource)]
#[case(
    "boto3.resource('dynamodb')",
    "dynamodb",
    "dynamodb",
    SdkObjectKind::Resource
)]
fn test_kind_inference(
    #[case] rhs: &str,
    #[case] var_name: &str,
    #[case] expected_service: &str,
    #[case] expected_kind: SdkObjectKind,
) {
    let source_code = format!("import boto3\n{var_name} = {rhs}\n");
    let ast = create_ast(&source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let info = tracker
        .get_type_info_for_variable_in_context(var_name, None)
        .unwrap();
    assert_eq!(info.service_name, expected_service);
    assert_eq!(info.kind, Some(expected_kind));
    assert_eq!(info.qualified_type, None);
}

#[test]
fn test_resource_collection_kind_inference() {
    let source_code = r#"
import boto3
dynamodb = boto3.resource('dynamodb')
table = dynamodb.Table('users')
s3 = boto3.resource('s3')
bucket = s3.Bucket('my-bucket')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let dynamodb_info = tracker.get_type_info_for_variable_in_context("dynamodb", None);
    assert!(dynamodb_info.is_some());
    assert_eq!(dynamodb_info.unwrap().kind, Some(SdkObjectKind::Resource));

    let table_info = tracker.get_type_info_for_variable_in_context("table", None);
    assert!(table_info.is_some());
    let table_info = table_info.unwrap();
    assert_eq!(table_info.service_name, "dynamodb");
    assert_eq!(table_info.kind, Some(SdkObjectKind::ResourceCollection));

    let s3_info = tracker.get_type_info_for_variable_in_context("s3", None);
    assert!(s3_info.is_some());
    assert_eq!(s3_info.unwrap().kind, Some(SdkObjectKind::Resource));

    let bucket_info = tracker.get_type_info_for_variable_in_context("bucket", None);
    assert!(bucket_info.is_some());
    let bucket_info = bucket_info.unwrap();
    assert_eq!(bucket_info.service_name, "s3");
    assert_eq!(bucket_info.kind, Some(SdkObjectKind::ResourceCollection));
}

#[test]
fn test_kind_preserved_through_aliases() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
my_client = s3_client
another_client = my_client
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_info = tracker.get_type_info_for_variable_in_context("s3_client", None);
    assert!(s3_info.is_some());
    assert_eq!(s3_info.unwrap().kind, Some(SdkObjectKind::Client));

    let my_info = tracker.get_type_info_for_variable_in_context("my_client", None);
    assert!(my_info.is_some());
    assert_eq!(my_info.unwrap().kind, Some(SdkObjectKind::Client));

    let another_info = tracker.get_type_info_for_variable_in_context("another_client", None);
    assert!(another_info.is_some());
    assert_eq!(another_info.unwrap().kind, Some(SdkObjectKind::Client));
}

#[test]
fn test_client_vs_resource_distinction() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
s3_resource = boto3.resource('s3')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let client_info = tracker
        .get_type_info_for_variable_in_context("s3_client", None)
        .unwrap();
    let resource_info = tracker
        .get_type_info_for_variable_in_context("s3_resource", None)
        .unwrap();

    assert_eq!(client_info.service_name, "s3");
    assert_eq!(resource_info.service_name, "s3");

    assert_eq!(client_info.kind, Some(SdkObjectKind::Client));
    assert_eq!(resource_info.kind, Some(SdkObjectKind::Resource));
    assert_ne!(client_info.kind, resource_info.kind);
}

// ========== Conflicted Function Name Tests ==========

#[test]
fn test_duplicate_top_level_functions_return_none() {
    // Two top-level functions with the same name — tracker should return None
    let source_code = r#"
import boto3

def process(client):
    client.list_buckets()

def process(client):
    client.describe_instances()

s3 = boto3.client('s3')
process(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // 'process' is conflicted — lookups should return None
    assert_eq!(
        tracker.get_service_for_variable_in_context("client", Some("process")),
        None
    );
    assert!(tracker
        .get_type_info_for_variable_in_context("client", Some("process"))
        .is_none());
    assert!(tracker
        .get_services_for_parameter("process", "client")
        .is_none());
}

#[test]
fn test_same_named_methods_in_different_classes_not_conflicted() {
    // Methods with same name in different classes should NOT be conflicted —
    // class methods don't poison the tracker for top-level functions.
    let source_code = r#"
import boto3

class S3Handler:
    def process(self, client):
        client.list_buckets()

class EC2Handler:
    def process(self, client):
        client.describe_instances()

def upload(client):
    client.put_object(Bucket='b', Key='k', Body=b'data')

s3 = boto3.client('s3')
upload(s3)
process(s3)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    // 'process' is NOT conflicted (they're methods, not top-level functions)
    assert!(!tracker.conflicted_functions.contains("process"));

    // 'upload' is unique and should resolve
    let services = tracker.get_services_for_parameter("upload", "client");
    assert!(services.is_some());
    assert!(services.unwrap().contains("s3"));

    // Calling process(s3) still maps correctly via parameter_types
    // (self is filtered, so 'client' is position 0)
    let process_services = tracker.get_services_for_parameter("process", "client");
    assert!(process_services.is_some());
    assert!(process_services.unwrap().contains("s3"));
}

// ========== Python Scoping (LEGB) Tests ==========

#[test]
fn test_parameter_shadows_module_variable() {
    let source_code = r#"
import boto3

s3_client = boto3.client('s3')

def upload_file(s3_client):
    pass

dynamodb_client = boto3.client('dynamodb')
upload_file(dynamodb_client)
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable_in_context("s3_client", None),
        Some(&"s3".to_string())
    );

    let param_service =
        tracker.get_service_for_variable_in_context("s3_client", Some("upload_file"));
    assert_eq!(param_service, Some(&"dynamodb".to_string()));

    let module_service = tracker.get_service_for_variable_in_context("s3_client", None);
    assert_ne!(param_service, module_service);
}

#[test]
fn test_function_variable_shadows_module_variable() {
    let source_code = r#"
import boto3

client = boto3.client('s3')

def process_data():
    client = boto3.client('dynamodb')
    client.put_item(TableName='table', Item={})
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable_in_context("client", None),
        Some(&"s3".to_string())
    );

    assert_eq!(
        tracker.get_service_for_variable_in_context("client", Some("process_data")),
        Some(&"dynamodb".to_string())
    );

    let module_service = tracker.get_service_for_variable_in_context("client", None);
    let function_service =
        tracker.get_service_for_variable_in_context("client", Some("process_data"));
    assert_ne!(module_service, function_service);
}
