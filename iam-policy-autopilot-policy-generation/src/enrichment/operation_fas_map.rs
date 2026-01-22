//! Operation Action Map data models and embedded loader implementation
//!
//! This module contains the data structures used to represent operation
//! action maps that are loaded from embedded JSON files and used for IAM policy enrichment.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use rust_embed::RustEmbed;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

use crate::enrichment::Context;

type ServiceName = String;
type OperationName = String;

pub(crate) type OperationFasMaps = HashMap<ServiceName, Arc<OperationFasMap>>;

/// Root structure for operation FAS map JSON files
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct OperationFasMapRoot {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Operations")]
    operations: Vec<OperationWithFas>,
}

/// Individual operation with its FAS operations
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct OperationWithFas {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "FasOperations", default)]
    fas_operations: Vec<FasOperation>,
}

/// Complete operation dependency map for a service
///
/// Represents the complete mapping of operations to actions for a specific AWS service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationFasMap {
    /// Map of operation to the associated actions with resources
    pub(crate) fas_operations: HashMap<OperationName, Vec<FasOperation>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
pub struct FasContext {
    pub(crate) key: String,
    pub(crate) values: Vec<String>,
}

impl FasContext {
    pub(crate) fn new(key: String, values: Vec<String>) -> Self {
        Self { key, values }
    }
}

impl Context for FasContext {
    fn key(&self) -> &str {
        &self.key
    }

    fn values(&self) -> &[String] {
        &self.values
    }
}

// Custom deserializer for Context that handles HashMap-like JSON objects
// Supports both single string values and arrays of strings
fn deserialize_context_map<'de, D>(deserializer: D) -> Result<Vec<FasContext>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{MapAccess, Visitor};
    use serde_json::Value;
    use std::fmt;

    struct ContextMapVisitor;

    impl<'de> Visitor<'de> for ContextMapVisitor {
        type Value = Vec<FasContext>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of key-value pairs where values can be strings or arrays")
        }

        fn visit_map<V>(self, mut map: V) -> Result<Vec<FasContext>, V::Error>
        where
            V: MapAccess<'de>,
        {
            let mut contexts = Vec::new();

            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                let values = match value {
                    Value::String(s) => vec![s],
                    Value::Array(arr) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => continue, // Skip non-string/non-array values
                };

                if !values.is_empty() {
                    contexts.push(FasContext::new(key, values));
                }
            }

            Ok(contexts)
        }
    }

    deserializer.deserialize_map(ContextMapVisitor)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct FasOperation {
    #[serde(rename = "Operation")]
    pub(crate) operation: String,
    #[serde(rename = "Service")]
    pub(crate) service: String,
    #[serde(rename = "Context", deserialize_with = "deserialize_context_map")]
    pub(crate) context: Vec<FasContext>,
}

#[cfg(test)]
impl FasOperation {
    pub(crate) fn new(operation: String, service: String, context: Vec<FasContext>) -> Self {
        FasOperation {
            operation,
            service,
            context,
        }
    }
}

impl<'de> Deserialize<'de> for OperationFasMap {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let root = OperationFasMapRoot::deserialize(deserializer)?;

        let mut fas_operations = HashMap::new();
        for op in root.operations {
            // Create the key as "service:operation" format
            let key = format!("{}:{}", root.name, op.name);
            fas_operations.insert(key, op.fas_operations);
        }

        Ok(OperationFasMap { fas_operations })
    }
}

/// Embedded operation FAS maps data
#[derive(RustEmbed)]
#[folder = "resources/config/operation-fas-maps"]
#[include = "*.json"]
struct EmbeddedOperationFasMaps;

/// Cache for parsed operation FAS maps (per service)
static OPERATION_FAS_MAPS_CACHE: OnceLock<RwLock<HashMap<String, Option<Arc<OperationFasMap>>>>> =
    OnceLock::new();

/// Load operation FAS map for a specific service from embedded data with caching
///
/// This function loads a single operation FAS map from embedded data and caches the parsed result
/// to avoid re-parsing JSON on subsequent calls for the same service.
///
/// # Arguments
/// * `service_name` - The name of the service to load the FAS map for
///
/// # Returns
/// An Option containing the OperationFasMap if found, None otherwise
///
/// # Errors
/// Returns `ExtractorError` if:
/// - The embedded operation FAS map file contains invalid JSON
/// - The JSON structure doesn't match OperationFasMap
pub(crate) fn load_operation_fas_map(service_name: &str) -> Option<Arc<OperationFasMap>> {
    let cache = OPERATION_FAS_MAPS_CACHE.get_or_init(|| RwLock::new(HashMap::new()));

    // Check cache first
    {
        let cache_guard = cache
            .read()
            .expect("Failed to acquire read lock on operation FAS maps cache");
        if let Some(cached_result) = cache_guard.get(service_name) {
            return cached_result.clone();
        }
    }

    // Load and parse from embedded data
    let file_name = format!("{}.json", service_name);
    let result = match EmbeddedOperationFasMaps::get(&file_name) {
        Some(embedded_file) => {
            let json_str = std::str::from_utf8(&embedded_file.data)
                .expect("Invalid UTF-8 in embedded operation FAS map");

            let operation_fas_map: OperationFasMap =
                serde_json::from_str(json_str).unwrap_or_else(|_| {
                    panic!(
                        "Failed to parse embedded operation FAS map JSON for service: {}",
                        service_name
                    )
                });

            Some(Arc::new(operation_fas_map))
        }
        None => None,
    };

    // Cache the result
    {
        let mut cache_guard = cache
            .write()
            .expect("Failed to acquire write lock on operation FAS maps cache");
        cache_guard.insert(service_name.to_string(), result.clone());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_operation_fas_map_existing_service() {
        // Test loading a FAS map for an existing service
        let ssm_map = load_operation_fas_map("ssm");

        // SSM should exist in our embedded data
        assert!(ssm_map.is_some());

        let map = ssm_map.unwrap();
        assert!(!map.fas_operations.is_empty());

        // Test that subsequent calls return cached result (should be identical)
        let ssm_map2 = load_operation_fas_map("ssm");
        assert!(ssm_map2.is_some());

        let map2 = ssm_map2.unwrap();
        assert_eq!(map.fas_operations.len(), map2.fas_operations.len());
    }

    #[test]
    fn test_load_operation_fas_map_nonexistent_service() {
        // Test loading a FAS map for a non-existent service
        let result = load_operation_fas_map("nonexistent-service");

        // Should return None for non-existent service
        assert!(result.is_none());

        // Test that subsequent calls return cached None result
        let result2 = load_operation_fas_map("nonexistent-service");
        assert!(result2.is_none());
    }

    #[test]
    fn test_embedded_operation_fas_maps_content() {
        // Test that we have some known services
        let ssm_map = load_operation_fas_map("ssm");
        assert!(ssm_map.is_some());

        let s3_map = load_operation_fas_map("s3");
        assert!(s3_map.is_some());

        let dynamodb_map = load_operation_fas_map("dynamodb");
        assert!(dynamodb_map.is_some());

        // Test SSM map content
        if let Some(ssm_map) = ssm_map {
            // SSM should have operations that require KMS decrypt/encrypt
            assert!(!ssm_map.fas_operations.is_empty());

            // Check for a known operation
            let get_param_key = "secretsmanager:GetParameter";
            if let Some(fas_ops) = ssm_map.fas_operations.get(get_param_key) {
                assert!(!fas_ops.is_empty());
                let first_op = &fas_ops[0];
                assert_eq!(first_op.service, "kms");
                assert_eq!(first_op.operation, "Decrypt");
            }
        }
    }

    #[test]
    fn test_operation_fas_map_deserialization() {
        let json_content = r#"{
            "Name": "test-service",
            "Operations": [
                {
                    "Name": "TestOperation",
                    "FasOperations": [
                        {
                            "Operation": "Decrypt",
                            "Service": "kms",
                            "Context": {
                                "kms:ViaService": "test-service.${region}.amazonaws.com"
                            }
                        }
                    ]
                }
            ]
        }"#;

        let operation_fas_map: OperationFasMap = serde_json::from_str(json_content).unwrap();

        // Verify the deserialized structure
        assert_eq!(operation_fas_map.fas_operations.len(), 1);

        let key = "test-service:TestOperation";
        assert!(operation_fas_map.fas_operations.contains_key(key));

        let fas_ops = &operation_fas_map.fas_operations[key];
        assert_eq!(fas_ops.len(), 1);

        let fas_op = &fas_ops[0];
        assert_eq!(fas_op.operation, "Decrypt");
        assert_eq!(fas_op.service, "kms");

        // Check that context contains the expected key-value pair
        assert_eq!(fas_op.context.len(), 1);
        let context_item = &fas_op.context[0];
        assert_eq!(context_item.key, "kms:ViaService");
        assert_eq!(
            context_item.values,
            vec!["test-service.${region}.amazonaws.com"]
        );
    }

    #[test]
    fn test_operation_fas_map_deserialization_multiple_context_entries() {
        let json_content = r#"{
            "Name": "test-service",
            "Operations": [
                {
                    "Name": "TestOperation",
                    "FasOperations": [
                        {
                            "Operation": "Decrypt",
                            "Service": "kms",
                            "Context": {
                                "kms:ViaService": "test-service.${region}.amazonaws.com",
                                "kms:EncryptionContext:SecretARN": "${aws:RequestedRegion}",
                                "aws:RequestedRegion": "us-east-1"
                            }
                        }
                    ]
                }
            ]
        }"#;

        let operation_fas_map: OperationFasMap = serde_json::from_str(json_content).unwrap();

        // Verify the deserialized structure
        assert_eq!(operation_fas_map.fas_operations.len(), 1);

        let key = "test-service:TestOperation";
        assert!(operation_fas_map.fas_operations.contains_key(key));

        let fas_ops = &operation_fas_map.fas_operations[key];
        assert_eq!(fas_ops.len(), 1);

        let fas_op = &fas_ops[0];
        assert_eq!(fas_op.operation, "Decrypt");
        assert_eq!(fas_op.service, "kms");

        // Check that context contains all expected key-value pairs
        assert_eq!(fas_op.context.len(), 3);

        // Verify all context keys are present
        let keys: std::collections::HashSet<_> =
            fas_op.context.iter().map(|ctx| &ctx.key).collect();
        assert!(keys.contains(&"kms:ViaService".to_string()));
        assert!(keys.contains(&"kms:EncryptionContext:SecretARN".to_string()));
        assert!(keys.contains(&"aws:RequestedRegion".to_string()));
    }

    #[test]
    fn test_caching_behavior() {
        // Test that caching works correctly for both existing and non-existing services

        // Load a service that exists
        let ssm_map1 = load_operation_fas_map("ssm");
        let ssm_map2 = load_operation_fas_map("ssm");

        // Both should be Some and have the same content
        assert!(ssm_map1.is_some());
        assert!(ssm_map2.is_some());
        assert_eq!(
            ssm_map1.as_ref().unwrap().fas_operations.len(),
            ssm_map2.as_ref().unwrap().fas_operations.len()
        );

        // Load a service that doesn't exist
        let nonexistent1 = load_operation_fas_map("cache-test-nonexistent");
        let nonexistent2 = load_operation_fas_map("cache-test-nonexistent");

        // Both should be None
        assert!(nonexistent1.is_none());
        assert!(nonexistent2.is_none());
    }

    #[test]
    fn test_multiple_service_loads() {
        // Test loading multiple different services to ensure cache isolation
        let services = ["ssm", "s3", "dynamodb", "nonexistent"];

        for service in &services {
            let result = load_operation_fas_map(service);

            if *service == "nonexistent" {
                assert!(result.is_none());
            } else {
                assert!(result.is_some());
            }
        }

        // Load them again to test cache hits
        for service in &services {
            let result = load_operation_fas_map(service);

            if *service == "nonexistent" {
                assert!(result.is_none());
            } else {
                assert!(result.is_some());
            }
        }
    }

    #[test]
    fn test_load_all_embedded_fas_maps() {
        // Test that all embedded FAS map files can be loaded and parsed successfully
        let mut loaded_services = Vec::new();
        let mut failed_services = Vec::new();

        // Iterate through all embedded files
        for file_path in EmbeddedOperationFasMaps::iter() {
            let file_name = file_path.as_ref();

            // Extract service name from filename (remove .json extension)
            if let Some(service_name) = file_name.strip_suffix(".json") {
                println!("Testing FAS map for service: {}", service_name);

                match load_operation_fas_map(service_name) {
                    Some(fas_map) => {
                        // Verify the map has some content
                        assert!(
                            !fas_map.fas_operations.is_empty(),
                            "FAS map for service '{}' should not be empty",
                            service_name
                        );

                        // Verify all FAS operations have required fields
                        for (operation_key, fas_operations) in &fas_map.fas_operations {
                            assert!(
                                !operation_key.is_empty(),
                                "Operation key should not be empty for service '{}'",
                                service_name
                            );

                            for fas_op in fas_operations {
                                assert!(!fas_op.operation.is_empty(),
                                       "FAS operation should have non-empty operation field for service '{}'", service_name);
                                assert!(!fas_op.service.is_empty(),
                                       "FAS operation should have non-empty service field for service '{}'", service_name);
                                // Context can be empty, so we don't assert on it
                            }
                        }

                        loaded_services.push(service_name.to_string());
                    }
                    None => {
                        // This shouldn't happen since we're iterating over existing files
                        failed_services.push(format!("{}: returned None", service_name));
                    }
                }
            }
        }

        // Print summary
        println!(
            "Successfully loaded FAS maps for {} services: {:?}",
            loaded_services.len(),
            loaded_services
        );

        if !failed_services.is_empty() {
            println!(
                "Failed to load FAS maps for {} services: {:?}",
                failed_services.len(),
                failed_services
            );
        }

        // Assert that we loaded at least some services and had no failures
        assert!(
            !loaded_services.is_empty(),
            "Should have loaded at least some FAS maps"
        );
        assert!(
            failed_services.is_empty(),
            "All embedded FAS maps should load successfully: {:?}",
            failed_services
        );

        // Verify we have the expected services (based on the files we know exist)
        let expected_services = [
            "dynamodb",
            "logs",
            "s3",
            "secretsmanager",
            "sns",
            "sqs",
            "ssm",
        ];
        for expected in &expected_services {
            assert!(
                loaded_services.contains(&expected.to_string()),
                "Expected service '{}' should be in loaded services: {:?}",
                expected,
                loaded_services
            );
        }

        println!(
            "✅ All {} embedded FAS maps loaded and validated successfully",
            loaded_services.len()
        );
    }
}
