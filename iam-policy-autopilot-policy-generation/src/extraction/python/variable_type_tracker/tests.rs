use super::types::*;
use crate::extraction::AstWithSourceFile;
use crate::SourceFile;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Python;

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

// ========== Basic Assignment Tracking Tests ==========

#[test]
fn test_track_simple_client_assignment() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
}

#[test]
fn test_track_simple_resource_assignment() {
    let source_code = r#"
import boto3
dynamodb = boto3.resource('dynamodb')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("dynamodb"),
        Some(&"dynamodb".to_string())
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
fn test_double_quotes() {
    let source_code = r#"
import boto3
s3_client = boto3.client("s3")
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
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

// ========== Helper Function Tests ==========

#[test]
fn test_extract_all_params() {
    assert_eq!(
        VariableTypeTracker::extract_all_params("client"),
        vec!["client"]
    );
    assert_eq!(
        VariableTypeTracker::extract_all_params("client, bucket, key"),
        vec!["client", "bucket", "key"]
    );
    assert_eq!(
        VariableTypeTracker::extract_all_params("self, client, table"),
        vec!["self", "client", "table"]
    );
    assert_eq!(
        VariableTypeTracker::extract_all_params(""),
        Vec::<String>::new()
    );

    // Test with default values
    assert_eq!(
        VariableTypeTracker::extract_all_params("client=None, bucket='default'"),
        vec!["client", "bucket"]
    );

    // Test with type annotations
    assert_eq!(
        VariableTypeTracker::extract_all_params("client: str, count: int"),
        vec!["client", "count"]
    );

    // Test with whitespace
    assert_eq!(
        VariableTypeTracker::extract_all_params(" client , bucket "),
        vec!["client", "bucket"]
    );

    // Test mixed
    assert_eq!(
        VariableTypeTracker::extract_all_params("client, bucket='default', key: str"),
        vec!["client", "bucket", "key"]
    );
}

// ========== SDK Object Kind Inference Tests ==========

#[test]
fn test_client_kind_inference() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let info = tracker.get_type_info_for_variable_in_context("s3_client", None);
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.service_name, "s3");
    assert_eq!(info.kind, Some(SdkObjectKind::Client));
    assert_eq!(info.qualified_type, None);
}

#[test]
fn test_resource_kind_inference() {
    let source_code = r#"
import boto3
s3 = boto3.resource('s3')
dynamodb = boto3.resource('dynamodb')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    let s3_info = tracker.get_type_info_for_variable_in_context("s3", None);
    assert!(s3_info.is_some());
    let s3_info = s3_info.unwrap();
    assert_eq!(s3_info.service_name, "s3");
    assert_eq!(s3_info.kind, Some(SdkObjectKind::Resource));

    let dynamodb_info = tracker.get_type_info_for_variable_in_context("dynamodb", None);
    assert!(dynamodb_info.is_some());
    let dynamodb_info = dynamodb_info.unwrap();
    assert_eq!(dynamodb_info.service_name, "dynamodb");
    assert_eq!(dynamodb_info.kind, Some(SdkObjectKind::Resource));
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
fn test_service_name_and_kind_apis() {
    let source_code = r#"
import boto3
s3_client = boto3.client('s3')
s3_resource = boto3.resource('s3')
"#;
    let ast = create_ast(source_code);
    let mut tracker = VariableTypeTracker::new();
    tracker.track_boto3_assignments(&ast);

    assert_eq!(
        tracker.get_service_for_variable("s3_client"),
        Some(&"s3".to_string())
    );
    assert_eq!(
        tracker.get_service_for_variable("s3_resource"),
        Some(&"s3".to_string())
    );

    let client_info = tracker.get_type_info_for_variable_in_context("s3_client", None);
    assert!(client_info.is_some());
    assert_eq!(client_info.unwrap().kind, Some(SdkObjectKind::Client));

    let resource_info = tracker.get_type_info_for_variable_in_context("s3_resource", None);
    assert!(resource_info.is_some());
    assert_eq!(resource_info.unwrap().kind, Some(SdkObjectKind::Resource));
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
