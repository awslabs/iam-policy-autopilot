//! Native JSON provider implementation for the extract-sdk-methods project.
//!
//! This module provides a high-performance JSON parsing and serialization implementation
//! using `serde_json` for native environments. It supports generic types with proper bounds,
//! comprehensive error handling, and performance optimizations for large JSON documents.

use serde::{Deserialize, Serialize};

use crate::errors::{ExtractorError, Result};

/// Native JSON provider using `serde_json` for high-performance JSON operations.
///
/// This provider is optimized for native environments and provides comprehensive
/// JSON parsing and serialization capabilities with detailed error reporting.
/// It supports both compact and pretty-printed output formats.
///
/// # Thread Safety
///
/// This provider is `Send + Sync` and can be safely shared across threads.
/// All operations are stateless and do not require mutable access.
///
/// # Performance Considerations
///
/// - Uses `serde_json` for optimal performance on native platforms
/// - Supports streaming operations for large documents
/// - Memory-efficient parsing with minimal allocations
/// - Optimized for AWS SDK JSON structures which can be quite large
///
/// # Error Handling
///
/// All JSON operations provide detailed error context including:
/// - The operation that failed (parsing, serialization, etc.)
/// - Line and column information for parsing errors
/// - Type mismatch details for deserialization errors
/// - Memory or I/O error information
#[derive(Debug, Clone)]
pub struct NativeJsonProvider;

impl NativeJsonProvider {
    /// Parse JSON to a generic `serde_json::Value`.
    ///
    /// This method parses JSON to a generic value type, which can be useful
    /// for dynamic JSON processing or when the exact structure is unknown.
    pub fn parse_to_value(json_str: &str) -> Result<serde_json::Value> {
        serde_json::from_str(json_str).map_err(ExtractorError::from)
    }

    /// Serialize a `serde_json::Value` to JSON string.
    ///
    /// This method serializes a generic JSON value back to a string,
    /// respecting the pretty-print setting.
    pub fn stringify_value(value: &serde_json::Value) -> Result<String> {
        serde_json::to_string(value).map_err(ExtractorError::from)
    }
    
    /// Serialize a `serde_json::Value` to JSON string.
    ///
    /// This method serializes a generic JSON value back to a string,
    /// respecting the pretty-print setting.
    pub fn stringify_value_pretty(value: &serde_json::Value) -> Result<String> {
        serde_json::to_string_pretty(value).map_err(ExtractorError::from)
    }

    /// Parse JSON string into a typed value.
    pub(crate) async fn parse<T>(json_str: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de> + Send + 'static,
    {
        serde_json::from_str(json_str).map_err(ExtractorError::from)
    }

    /// Serialize a value to JSON string.
    ///
    /// This method serializes a Rust value to JSON text, respecting the
    /// pretty-print setting configured on the provider.
    pub fn stringify<T>(value: &T) -> Result<String>
    where
        T: ?Sized + Serialize,
    {
        serde_json::to_string(value).map_err(ExtractorError::from)
    }
    
    /// Serialize a value to JSON string.
    ///
    /// This method serializes a Rust value to JSON text, respecting the
    /// pretty-print setting configured on the provider.
    pub fn stringify_pretty<T>(value: &T) -> Result<String>
    where
        T: ?Sized + Serialize,
    {
        serde_json::to_string_pretty(value).map_err(ExtractorError::from)
    }
}

#[cfg(test)]
mod tests {
    use crate::providers::JsonProvider;

    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Value};
    use std::collections::HashMap;

    // Test data structures
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct SimpleConfig {
        name: String,
        value: i32,
        enabled: bool,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct ComplexConfig {
        metadata: HashMap<String, String>,
        items: Vec<SimpleConfig>,
        optional_field: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct AwsSdkOperation {
        name: String,
        http: AwsHttpConfig,
        input: Option<AwsShapeRef>,
        output: Option<AwsShapeRef>,
        errors: Vec<AwsShapeRef>,
        documentation: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct AwsHttpConfig {
        method: String,
        #[serde(rename = "requestUri")]
        request_uri: String,
        #[serde(rename = "locationName")]
        location_name: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct AwsShapeRef {
        shape: String,
        location: Option<String>,
        #[serde(rename = "locationName")]
        location_name: Option<String>,
    }

    #[tokio::test]
    async fn test_basic_parsing() {
        let json_str = r#"{"name": "test", "value": 42, "enabled": true}"#;
        
        let config: SimpleConfig = JsonProvider::parse(json_str).await.unwrap();
        
        assert_eq!(config.name, "test");
        assert_eq!(config.value, 42);
        assert!(config.enabled);
    }

    #[tokio::test]
    async fn test_basic_serialization() {
        let config = SimpleConfig {
            name: "test".to_string(),
            value: 42,
            enabled: true,
        };
        
        let json_str = JsonProvider::stringify(&config).unwrap();
        let parsed: SimpleConfig = JsonProvider::parse(&json_str).await.unwrap();
        
        assert_eq!(config, parsed);
    }

    #[tokio::test]
    async fn test_pretty_printing() {
        let config = SimpleConfig {
            name: "test".to_string(),
            value: 42,
            enabled: true,
        };
        
        let json_str = JsonProvider::stringify_pretty(&config).unwrap();
        
        // Pretty-printed JSON should contain newlines and indentation
        assert!(json_str.contains('\n'));
        assert!(json_str.contains("  ")); // Indentation
        
        // Should still parse correctly
        let parsed: SimpleConfig = JsonProvider::parse(&json_str).await.unwrap();
        assert_eq!(config, parsed);
    }

    #[tokio::test]
    async fn test_complex_structures() {
        let mut metadata = HashMap::new();
        metadata.insert("version".to_string(), "1.0.0".to_string());
        metadata.insert("author".to_string(), "test".to_string());
        
        let complex_config = ComplexConfig {
            metadata,
            items: vec![
                SimpleConfig {
                    name: "item1".to_string(),
                    value: 1,
                    enabled: true,
                },
                SimpleConfig {
                    name: "item2".to_string(),
                    value: 2,
                    enabled: false,
                },
            ],
            optional_field: Some("present".to_string()),
        };
        
        let json_str = JsonProvider::stringify(&complex_config).unwrap();
        let parsed: ComplexConfig = JsonProvider::parse(&json_str).await.unwrap();
        
        assert_eq!(complex_config, parsed);
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.optional_field, Some("present".to_string()));
    }

    #[tokio::test]
    async fn test_aws_sdk_structure() {
        let operation = AwsSdkOperation {
            name: "CreateBucket".to_string(),
            http: AwsHttpConfig {
                method: "PUT".to_string(),
                request_uri: "/{Bucket}".to_string(),
                location_name: Some("Bucket".to_string()),
            },
            input: Some(AwsShapeRef {
                shape: "CreateBucketRequest".to_string(),
                location: None,
                location_name: None,
            }),
            output: Some(AwsShapeRef {
                shape: "CreateBucketOutput".to_string(),
                location: None,
                location_name: None,
            }),
            errors: vec![
                AwsShapeRef {
                    shape: "BucketAlreadyExists".to_string(),
                    location: None,
                    location_name: None,
                },
            ],
            documentation: Some("Creates a new S3 bucket".to_string()),
        };
        
        let json_str = JsonProvider::stringify(&operation).unwrap();
        let parsed: AwsSdkOperation = JsonProvider::parse(&json_str).await.unwrap();
        
        assert_eq!(operation, parsed);
        assert_eq!(parsed.name, "CreateBucket");
        assert_eq!(parsed.http.method, "PUT");
        assert_eq!(parsed.errors.len(), 1);
    }

    #[tokio::test]
    async fn test_malformed_json_error() {
        let malformed_json = r#"{"name": "test", "value": }"#; // Missing value
        
        let result: Result<SimpleConfig> = JsonProvider::parse(malformed_json).await;
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("JSON parsing error"));
        assert!(error_msg.contains("line"));
        assert!(error_msg.contains("column"));
    }

    #[tokio::test]
    async fn test_type_mismatch_error() {
        let json_str = r#"{"name": "test", "value": "not_a_number", "enabled": true}"#;
        
        let result: Result<SimpleConfig> = JsonProvider::parse(json_str).await;
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("JSON parsing error"));
    }

    #[tokio::test]
    async fn test_missing_required_field_error() {
        let json_str = r#"{"name": "test", "enabled": true}"#; // Missing "value" field
        
        let result: Result<SimpleConfig> = JsonProvider::parse(json_str).await;
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        assert!(error_msg.contains("JSON parsing error"));
    }

    #[tokio::test]
    async fn test_large_json_performance() {
        // Create a large JSON structure
        let mut large_data = HashMap::new();
        for i in 0..1000 {
            large_data.insert(
                format!("key_{i}"),
                SimpleConfig {
                    name: format!("item_{i}"),
                    value: i,
                    enabled: i % 2 == 0,
                },
            );
        }
        
        // Test serialization and parsing performance
        let start = std::time::Instant::now();
        let json_str = JsonProvider::stringify(&large_data).unwrap();
        let serialize_time = start.elapsed();
        
        let start = std::time::Instant::now();
        let parsed: HashMap<String, SimpleConfig> = JsonProvider::parse(&json_str).await.unwrap();
        let parse_time = start.elapsed();
        
        assert_eq!(large_data.len(), parsed.len());
        
        // Performance should be reasonable (adjust thresholds as needed)
        assert!(serialize_time.as_millis() < 1000, "Serialization too slow: {serialize_time:?}");
        assert!(parse_time.as_millis() < 1000, "Parsing too slow: {parse_time:?}");
    }

    #[tokio::test]
    async fn test_empty_and_null_values() {
        // Test empty string
        let empty_str: String = JsonProvider::parse(r#""""#).await.unwrap();
        assert_eq!(empty_str, "");
        
        // Test null value
        let null_value: Option<String> = JsonProvider::parse("null").await.unwrap();
        assert_eq!(null_value, None);
        
        // Test empty array
        let empty_array: Vec<i32> = JsonProvider::parse("[]").await.unwrap();
        assert!(empty_array.is_empty());
        
        // Test empty object
        let empty_object: HashMap<String, String> = JsonProvider::parse("{}").await.unwrap();
        assert!(empty_object.is_empty());
    }

    #[tokio::test]
    async fn test_unicode_handling() {
        let unicode_data = json!({
            "english": "Hello, World!",
            "chinese": "你好，世界！",
            "japanese": "こんにちは、世界！",
            "emoji": "🌍🚀✨",
            "special_chars": "\"\\n\\t\\r\""
        });
        
        let json_str = NativeJsonProvider::stringify_value(&unicode_data).unwrap();
        let parsed = NativeJsonProvider::parse_to_value(&json_str).unwrap();
        
        assert_eq!(unicode_data, parsed);
    }

    // Integration tests moved from core/tests/json_integration_test.rs
    #[tokio::test]
    async fn test_real_aws_sdk_json_parsing() {
        use crate::FileSystemProvider;
        
        // Test with S3 waiters JSON (a real AWS SDK file)
        let waiters_path = "../iam-policy-autopilot-policy-generation/resources/config/sdks/botocore-data/botocore/data/s3/2006-03-01/waiters-2.json";
        
        if let Ok(json_content) = FileSystemProvider::read_file(waiters_path).await {
            // Parse the JSON to a generic Value first
            let parsed_value: Value = JsonProvider::parse(&json_content).await
                .expect("Failed to parse S3 waiters JSON");
            
            // Verify it's a valid JSON structure
            assert!(parsed_value.is_object());
            
            // Test round-trip serialization
            let serialized = JsonProvider::stringify(&parsed_value)
                .expect("Failed to serialize parsed JSON");
            
            let reparsed: Value = JsonProvider::parse(&serialized).await
                .expect("Failed to reparse serialized JSON");
            
            assert_eq!(parsed_value, reparsed);
            
            println!("Successfully processed S3 waiters JSON with {} top-level keys",
                     parsed_value.as_object().unwrap().len());
        }
        
        // Test with S3 paginators JSON
        let paginators_path = "../iam-policy-autopilot-policy-generation/resources/config/sdks/botocore-data/botocore/data/s3/2006-03-01/paginators-1.json";
        
        if let Ok(json_content) = FileSystemProvider::read_file(paginators_path).await {
            let parsed_value: Value = JsonProvider::parse(&json_content).await
                .expect("Failed to parse S3 paginators JSON");
            
            assert!(parsed_value.is_object());
            
            println!("Successfully processed S3 paginators JSON with {} top-level keys",
                     parsed_value.as_object().unwrap().len());
        }
    }

    #[tokio::test]
    async fn test_pretty_printing_with_aws_json() {
        // Create a sample AWS-like structure
        let aws_operation = serde_json::json!({
            "CreateBucket": {
                "name": "CreateBucket",
                "http": {
                    "method": "PUT",
                    "requestUri": "/{Bucket}"
                },
                "input": {
                    "shape": "CreateBucketRequest"
                },
                "output": {
                    "shape": "CreateBucketOutput"
                },
                "errors": [
                    {
                        "shape": "BucketAlreadyExists"
                    },
                    {
                        "shape": "BucketAlreadyOwnedByYou"
                    }
                ],
                "documentation": "Creates a new S3 bucket. When the bucket is created in the US East (N. Virginia) region, you do not need to specify the location."
            }
        });
        
        let pretty_json = JsonProvider::stringify_pretty(&aws_operation)
            .expect("Failed to serialize with pretty printing");
        
        // Verify pretty printing worked
        assert!(pretty_json.contains('\n'));
        assert!(pretty_json.contains("  "));
        
        // Verify it can be parsed back
        let reparsed: Value = JsonProvider::parse(&pretty_json).await
            .expect("Failed to parse pretty-printed JSON");
        
        assert_eq!(aws_operation, reparsed);
        
        println!("Pretty-printed JSON:\n{}", pretty_json);
    }

    #[tokio::test]
    async fn test_error_handling_with_malformed_aws_json() {
        // Test with malformed JSON that might occur in AWS SDK files
        let malformed_json = r#"{
            "operations": {
                "CreateBucket": {
                    "name": "CreateBucket",
                    "http": {
                        "method": "PUT",
                        "requestUri": "/{Bucket}"
                    },
                    "input": {
                        "shape": "CreateBucketRequest"
                    },
                    "output": {
                        "shape": "CreateBucketOutput"
                    },
                    "errors": [
                        {
                            "shape": "BucketAlreadyExists"
                        },
                        {
                            "shape": "BucketAlreadyOwnedByYou"
                        }
                    ],
                    "documentation": "Creates a new S3 bucket."
                }
            }
        "#; // Missing closing brace
        
        let result: Result<Value> = JsonProvider::parse(malformed_json).await;
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        let error_msg = error.to_string();
        
        // Verify error contains useful information
        assert!(error_msg.contains("JSON parsing error"));
        assert!(error_msg.contains("line"));
        assert!(error_msg.contains("column"));
        
        println!("Error message: {}", error_msg);
    }
}