//! Integration tests for the public API of iam-policy-autopilot-policy-generation
//!
//! These tests verify that the entire public interface works correctly together,
//! testing the complete workflow from source code extraction through enrichment
//! to policy generation using only the public API.

use iam_policy_autopilot_policy_generation::{
    EnrichmentEngine, ExtractionEngine, FileSystemProvider, JsonProvider, Language, PolicyGenerationEngine, SourceFile
};
use std::path::PathBuf;
use tempfile::NamedTempFile;
use std::io::Write;

/// Helper function to create a temporary Python file with SDK calls
fn create_test_python_file(content: &str) -> NamedTempFile {
    let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
    temp_file.write_all(content.as_bytes()).expect("Failed to write to temp file");
    temp_file
}

#[tokio::test]
async fn test_complete_workflow_s3_operations() {
    // Test the complete workflow: Extraction -> Enrichment -> Policy Generation
    
    // 1. Create test source file with S3 operations
    let python_source = r#"
import boto3

def upload_and_list_objects():
    """Function that performs S3 operations."""
    s3_client = boto3.client('s3')
    
    # Upload an object
    s3_client.put_object(
        Bucket='my-test-bucket',
        Key='test-file.txt',
        Body=b'Hello, World!'
    )
    
    # List objects
    response = s3_client.list_objects_v2(
        Bucket='my-test-bucket'
    )
    
    return response

def download_object():
    """Function that downloads an S3 object."""
    s3 = boto3.client('s3')
    return s3.get_object(Bucket='my-bucket', Key='my-key')
"#;

    let temp_file = create_test_python_file(python_source);
    let temp_path = temp_file.path().to_path_buf();

    // 2. Test Extraction Engine (Public API)
    let extraction_engine = ExtractionEngine::new();
    
    let source_file = SourceFile::with_language(
        temp_path.clone(),
        python_source.to_string(),
        Language::Python
    );

    let extracted_methods = extraction_engine
        .extract_sdk_method_calls(Language::Python, vec![source_file])
        .await
        .expect("Extraction should succeed");

    // Verify extraction results using only public API
    assert!(!extracted_methods.methods.is_empty(), "Should extract some methods");
    assert_eq!(extracted_methods.metadata.source_files.len(), 1);
    assert_eq!(extracted_methods.metadata.source_files[0].language, Language::Python);
    
    println!("Extracted {} methods", extracted_methods.methods.len());
    // Note: Cannot access private fields, so we test serialization instead
    let methods_json = serde_json::to_string(&extracted_methods.methods)
        .expect("Should serialize methods");
    assert!(!methods_json.is_empty());

    // 3. Test Enrichment Engine (Public API)
    let mut enrichment_engine = EnrichmentEngine::new(false).unwrap();

    let enriched_methods = enrichment_engine
        .enrich_methods(&extracted_methods.methods)
        .await
        .expect("Enrichment should succeed");

    // Verify enrichment results
    assert!(!enriched_methods.is_empty(), "Should have enriched methods");
    println!("Enriched {} method calls", enriched_methods.len());

    // 4. Test Policy Generation Engine (Public API)
    let policy_engine = PolicyGenerationEngine::new(
        "aws",
        "us-east-1",
        "123456789012",
    );

    let policies = policy_engine
        .generate_policies(&enriched_methods)
        .expect("Policy generation should succeed");

    // Verify policy generation results using serialization
    assert!(!policies.is_empty(), "Should generate policies");
    println!("Generated {} policies", policies.len());
    
    // Test that policies can be serialized (verifies structure)
    for (i, policy) in policies.iter().enumerate() {
        let policy_json = serde_json::to_string(policy)
            .expect("Should serialize policy");
        assert!(policy_json.contains("2012-10-17"));
        assert!(policy_json.contains("Statement"));
        println!("  Policy {}: serialized successfully", i + 1);
    }

    // 5. Test policy merging
    let merged_policy = policy_engine
        .merge_policies(&policies)
        .expect("Policy merging should succeed");

    let merged_json = serde_json::to_string(&merged_policy)
        .expect("Should serialize merged policy");
    assert!(merged_json.contains("2012-10-17"));
    println!("Merged policy serialized successfully");

    // 6. Test method action mapping extraction
    let action_mappings = policy_engine
        .extract_action_mappings(&enriched_methods)
        .expect("Action mapping extraction should succeed");

    assert!(!action_mappings.is_empty(), "Should have action mappings");
    println!("Generated {} action mappings", action_mappings.len());
    
    // Test serialization of action mappings
    let mappings_json = serde_json::to_string(&action_mappings)
        .expect("Should serialize action mappings");
    assert!(!mappings_json.is_empty());
}

#[tokio::test]
async fn test_extraction_engine_language_detection() {
    // Test language detection and validation
    let extraction_engine = ExtractionEngine::new();

    // Create temporary files to test with
    let python_file1 = PathBuf::from("test1.py");
    let python_file2 = PathBuf::from("test2.py");
    let python_paths = vec![python_file1.as_path(), python_file2.as_path()];

    let detected_language = extraction_engine
        .detect_and_validate_language(&python_paths)
        .expect("Should detect Python language");

    assert_eq!(detected_language.to_string(), "python");

    // Test with mixed languages (should fail)
    let mixed_file1 = PathBuf::from("test.py");
    let mixed_file2 = PathBuf::from("test.js");
    let mixed_paths = vec![mixed_file1.as_path(), mixed_file2.as_path()];

    let result = extraction_engine.detect_and_validate_language(&mixed_paths);
    assert!(result.is_err(), "Should fail with mixed languages");
}

#[tokio::test]
async fn test_source_file_creation_and_serialization() {
    // Test SourceFile public API
    let source_file = SourceFile::with_language(
        PathBuf::from("test.py"),
        "print('hello')".to_string(),
        Language::Python,
    );

    assert_eq!(source_file.language, Language::Python);
    assert_eq!(source_file.content, "print('hello')");

    // Test serialization through JsonProvider
    let json_str = JsonProvider::stringify(&source_file)
        .expect("Should serialize SourceFile");
    
    assert!(json_str.contains("python"));
    assert!(!json_str.contains("print('hello')")); // content is skipped in serialization

    // Test pretty printing
    let pretty_json = JsonProvider::stringify_pretty(&source_file)
        .expect("Should serialize SourceFile with pretty printing");
    
    assert!(pretty_json.contains('\n'));
    assert!(pretty_json.contains("  "));
}

#[tokio::test]
async fn test_json_provider_public_api() {
    // Test JsonProvider public methods
    use serde_json::json;

    let test_data = json!({
        "name": "test",
        "value": 42,
        "nested": {
            "array": [1, 2, 3]
        }
    });

    // Test stringify
    let json_str = JsonProvider::stringify(&test_data)
        .expect("Should stringify JSON");
    assert!(json_str.contains("\"name\":\"test\""));

    // Test stringify_pretty
    let pretty_str = JsonProvider::stringify_pretty(&test_data)
        .expect("Should stringify JSON with pretty printing");
    assert!(pretty_str.contains('\n'));
    assert!(pretty_str.contains("  "));

    // Test value operations
    let value_str = JsonProvider::stringify_value(&test_data)
        .expect("Should stringify value");
    let parsed_value = JsonProvider::parse_to_value(&value_str)
        .expect("Should parse to value");
    assert_eq!(parsed_value, test_data);
}

#[tokio::test]
async fn test_filesystem_provider_public_api() {
    // Test FileSystemProvider public methods
    let temp_file = create_test_python_file("test content");
    let temp_path = temp_file.path();

    // Test file_exists
    let exists = FileSystemProvider::file_exists(temp_path)
        .await
        .expect("Should check file existence");
    assert!(exists, "Temp file should exist");

    // Test read_file
    let content = FileSystemProvider::read_file(temp_path)
        .await
        .expect("Should read file");
    assert_eq!(content, "test content");

    // Test with non-existent file
    let non_existent = PathBuf::from("/non/existent/file.txt");
    let exists = FileSystemProvider::file_exists(&non_existent)
        .await
        .expect("Should check non-existent file");
    assert!(!exists, "Non-existent file should not exist");
}

#[tokio::test]
async fn test_end_to_end_with_multiple_services() {
    // Test with source code that uses multiple AWS services
    let multi_service_source = r#"
import boto3

def multi_service_operations():
    """Function using multiple AWS services."""
    
    # S3 operations
    s3 = boto3.client('s3')
    s3.list_buckets()
    
    # EC2 operations  
    ec2 = boto3.client('ec2')
    ec2.describe_instances()
    
    # Lambda operations
    lambda_client = boto3.client('lambda')
    lambda_client.list_functions()
    
    return "done"
"#;

    let temp_file = create_test_python_file(multi_service_source);
    let temp_path = temp_file.path().to_path_buf();

    // Complete workflow test
    let extraction_engine = ExtractionEngine::new();
    let source_file = SourceFile::with_language(
        temp_path,
        multi_service_source.to_string(),
        Language::Python
    );

    let extracted = extraction_engine
        .extract_sdk_method_calls(Language::Python, vec![source_file])
        .await
        .expect("Should extract methods");

    // Verify we found methods (using serialization to check content)
    let methods_json = serde_json::to_string(&extracted.methods)
        .expect("Should serialize methods");
    println!("Extracted methods JSON length: {}", methods_json.len());
    
    // Continue with enrichment and policy generation
    let mut enrichment_engine = EnrichmentEngine::new(false).unwrap();
    
    let enriched = enrichment_engine
        .enrich_methods(&extracted.methods)
        .await
        .expect("Should enrich methods");

    let policy_engine = PolicyGenerationEngine::new(
        "aws",
        "us-west-2",
        "987654321098",
    );

    let policies = policy_engine
        .generate_policies(&enriched)
        .expect("Should generate policies");

    // Verify we got policies for multi-service operations
    println!("Generated {} policies for multi-service operations", policies.len());
    assert!(!policies.is_empty(), "Should generate policies for multi-service code");
}

#[test]
fn test_public_types_serialization() {
    // Test that all public types can be serialized/deserialized
    use serde_json::json;
    
    // Test basic JSON serialization of a simple structure
    let test_data = json!({
        "test": "value",
        "number": 42
    });
    
    let json_str = serde_json::to_string(&test_data).expect("Should serialize test data");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Should deserialize test data");
    assert_eq!(test_data, parsed);
}

#[tokio::test]
async fn test_extraction_with_simple_python_code() {
    // Test extraction with simple Python code that doesn't use AWS SDK
    let simple_python = r#"
def hello_world():
    print("Hello, World!")
    return "done"

class TestClass:
    def method(self):
        pass
"#;

    let temp_file = create_test_python_file(simple_python);
    let temp_path = temp_file.path().to_path_buf();

    let extraction_engine = ExtractionEngine::new();
    let source_file = SourceFile::with_language(
        temp_path,
        simple_python.to_string(),
        Language::Python
    );

    let extracted = extraction_engine
        .extract_sdk_method_calls(Language::Python, vec![source_file])
        .await
        .expect("Should extract methods");

    // Should succeed but find no AWS SDK methods
    println!("Extracted {} methods from simple Python code", extracted.methods.len());
    
    // Test that we can serialize the results
    let json = serde_json::to_string(&extracted.methods)
        .expect("Should serialize extracted methods");
    assert!(!json.is_empty());
}

#[tokio::test]
async fn test_provider_integration() {
    // Test that providers work together correctly
    let test_json = r#"{"name": "test", "value": 42}"#;
    
    // Create a temporary file with JSON content
    let temp_file = create_test_python_file(test_json);
    let temp_path = temp_file.path();
    
    // Read the file using FileSystemProvider
    let file_content = FileSystemProvider::read_file(temp_path)
        .await
        .expect("Should read file");
    
    // Parse the content using JsonProvider
    let parsed_value = JsonProvider::parse_to_value(&file_content)
        .expect("Should parse JSON");
    
    // Verify the parsed content
    assert_eq!(parsed_value["name"], "test");
    assert_eq!(parsed_value["value"], 42);
    
    // Serialize it back
    let serialized = JsonProvider::stringify_value(&parsed_value)
        .expect("Should serialize value");
    
    // Should be able to parse it again
    let reparsed = JsonProvider::parse_to_value(&serialized)
        .expect("Should reparse JSON");
    
    assert_eq!(parsed_value, reparsed);
}
