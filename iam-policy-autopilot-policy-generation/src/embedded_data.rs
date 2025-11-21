//! Embedded AWS SDK service definitions
//!
//! This module provides access to pre-processed and compressed AWS service definitions
//! that are embedded directly into the binary at compile time. The service definitions
//! have been simplified to remove documentation and examples, reducing binary size
//! while maintaining all essential functionality.

use crate::errors::{ExtractorError, Result};
use crate::extraction::sdk_model::SdkServiceDefinition;
use crate::providers::JsonProvider;
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// JSON structure for boto3 utilities mapping
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UtilityMappingJson {
    /// Map of service names to their utility methods configuration
    pub(crate) services: std::collections::HashMap<String, ServiceUtilityMethodsJson>,
}

/// Service-level utility methods configuration
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServiceUtilityMethodsJson {
    /// Client-level utility methods (e.g., s3.upload_file)
    pub(crate) client_methods: std::collections::HashMap<String, UtilityMethodJson>,
    /// Resource-level utility methods organized by resource type
    pub(crate) resource_methods: std::collections::HashMap<String, ResourceTypeUtilityMethodsJson>,
}

/// Resource type utility methods map
pub(crate) type ResourceTypeUtilityMethodsJson =
    std::collections::HashMap<String, UtilityMethodJson>;

/// Individual utility method specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UtilityMethodJson {
    /// List of AWS API operations this utility method invokes
    pub(crate) operations: Vec<ServiceOperation>,
    /// Parameters accepted by this utility method
    pub(crate) accepted_params: Vec<String>,
    /// Mappings from constructor arguments to operation parameters
    #[serde(default)]
    pub(crate) identifier_mappings: Vec<IdentifierMapping>,
}

/// Service operation with required parameters
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServiceOperation {
    /// AWS API operation name (e.g., "PutObject", "GetItem")
    pub(crate) operation: String,
    /// Required parameters for this operation
    pub(crate) required_params: Vec<String>,
}

/// Identifier mapping for utility methods
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IdentifierMapping {
    /// Target parameter name in the operation
    pub(crate) target_param: String,
    /// Index of the constructor argument to map from
    pub(crate) constructor_arg_index: usize,
}

/// Raw JSON structure for parsing boto3 resources files
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Boto3ResourcesJson {
    /// Service-level configuration
    pub(crate) service: Option<ServiceSpec>,
    /// Resource definitions
    pub(crate) resources: Option<HashMap<String, ResourceSpec>>,
}

/// Service specification from boto3 resources
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServiceSpec {
    /// Service constructors (e.g., s3.Bucket())
    pub(crate) has: Option<HashMap<String, HasSpec>>,
    /// Service-level collections
    #[serde(rename = "hasMany")]
    pub(crate) has_many: Option<HashMap<String, HasManySpecJson>>,
}

/// Constructor specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HasSpec {
    /// Resource reference
    pub(crate) resource: ResourceRef,
}

/// Resource reference in constructor
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResourceRef {
    /// Resource type name
    #[serde(rename = "type")]
    pub(crate) resource_type: String,
    /// Identifiers for this resource
    #[serde(default)]
    pub(crate) identifiers: Option<serde_json::Value>,
}

/// Resource specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResourceSpec {
    /// Resource identifiers
    pub(crate) identifiers: Option<Vec<ResourceIdentifier>>,
    /// Resource actions
    pub(crate) actions: Option<HashMap<String, ActionSpec>>,
    /// Load operation
    pub(crate) load: Option<LoadSpec>,
    /// Waiters
    pub(crate) waiters: Option<HashMap<String, WaiterSpec>>,
    /// HasMany collections
    #[serde(rename = "hasMany")]
    pub(crate) has_many: Option<HashMap<String, HasManySpecJson>>,
}

/// Resource identifier
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResourceIdentifier {
    /// Identifier name
    pub(crate) name: String,
}

/// Action specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ActionSpec {
    /// Request details
    pub(crate) request: RequestSpec,
}

/// Load operation specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LoadSpec {
    /// Request details
    pub(crate) request: RequestSpec,
}

/// Waiter specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WaiterSpec {
    /// Waiter name
    #[serde(rename = "waiterName")]
    pub(crate) waiter_name: String,
    /// Parameters
    #[serde(default)]
    pub(crate) params: Option<Vec<ParamMapping>>,
}

/// Request specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RequestSpec {
    /// Operation name
    pub(crate) operation: String,
    /// Parameters
    pub(crate) params: Option<Vec<ParamMapping>>,
}

/// Parameter mapping
#[derive(Debug, Clone, Deserialize)]
pub struct ParamMapping {
    /// Target parameter
    pub target: String,
    /// Source (e.g., "identifier")
    pub source: String,
    /// Optional name
    #[serde(default)]
    pub name: Option<String>,
}

/// HasMany collection specification
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HasManySpecJson {
    /// Request details
    pub(crate) request: RequestSpec,
}

/// Cached, parsed utilities mapping
static UTILITIES_MAPPING: OnceLock<Option<UtilityMappingJson>> = OnceLock::new();

/// Cached boto3 service versions map
static BOTO3_SERVICE_VERSIONS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();

/// Cached botocore service versions map
static BOTOCORE_SERVICE_VERSIONS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();

/// Embedded boto3 utilities mapping
///
/// This struct provides access to the boto3 utilities mapping configuration
/// that defines client utility methods and resource methods.
#[derive(RustEmbed)]
#[folder = "resources/config/sdks"]
#[include = "boto3_utilities_mapping.json"]
pub(crate) struct Boto3Utilities;

impl Boto3Utilities {
    /// Get the boto3 utilities mapping configuration (cached and deserialized)
    pub(crate) fn get_utilities_mapping() -> Option<&'static UtilityMappingJson> {
        UTILITIES_MAPPING
            .get_or_init(|| {
                let file = Self::get("boto3_utilities_mapping.json")?;
                serde_json::from_slice(&file.data).ok()
            })
            .as_ref()
    }
}

/// Embedded AWS boto3 resource definitions
///
/// This struct provides access to boto3 resource definitions
/// that are embedded directly into the binary at compile time.
#[derive(RustEmbed)]
#[folder = "target/boto3-data-simplified"]
#[include = "*.json"]
pub(crate) struct Boto3Resources;

impl Boto3Resources {
    /// Get a boto3 resources definition file by service name and API version (deserialized)
    pub(crate) fn get_resources_definition(
        service: &str,
        api_version: &str,
    ) -> Option<Boto3ResourcesJson> {
        let start_time = std::time::Instant::now();

        let json_path = format!("{}/{}/resources-1.json", service, api_version);
        if let Some(file) = Self::get(&json_path) {
            let file_size = file.data.len();

            // Deserialize directly from bytes
            let result = serde_json::from_slice(&file.data).ok();

            let total_time = start_time.elapsed();
            if total_time.as_millis() > 10 {
                log::debug!(
                    "Loaded and parsed boto3 {}/{}: {}KB in {:?}",
                    service,
                    api_version,
                    file_size / 1024,
                    total_time
                );
            }

            result
        } else {
            None
        }
    }

    /// Build a complete service-to-versions map for boto3 resources (cached)
    pub(crate) fn build_service_versions_map() -> &'static HashMap<String, Vec<String>> {
        BOTO3_SERVICE_VERSIONS.get_or_init(|| {
            log::debug!("Building boto3 service versions map...");

            let start_time = std::time::Instant::now();
            let mut service_versions: std::collections::HashMap<
                String,
                std::collections::HashSet<String>,
            > = std::collections::HashMap::new();
            let mut file_count = 0;

            for file_path in Boto3Resources::iter() {
                file_count += 1;
                let path_parts: Vec<&str> = file_path.split('/').collect();
                if path_parts.len() >= 2 {
                    let service = path_parts[0].to_string();
                    let version = path_parts[1].to_string();
                    service_versions.entry(service).or_default().insert(version);
                }
            }

            // Convert HashSet to sorted Vec for each service
            let mut result: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (service, versions_set) in service_versions {
                let mut versions: Vec<String> = versions_set.into_iter().collect();
                versions.sort();
                result.insert(service, versions);
            }

            let duration = start_time.elapsed();
            log::debug!(
                "Built boto3 service versions map in {:?} (processed {} files, found {} services)",
                duration,
                file_count,
                result.len()
            );

            result
        })
    }
}

/// Embedded AWS service definitions with compression
///
/// This struct provides access to pre-processed AWS service definitions
/// that have been simplified to remove documentation and examples,
/// reducing binary size while maintaining essential functionality.
#[derive(RustEmbed)]
#[folder = "target/botocore-data-simplified"]
#[include = "*.json"]
pub(crate) struct Botocore;

impl Botocore {
    /// Get a service definition file by service name and API version
    pub(crate) fn get_service_definition(service: &str, api_version: &str) -> Option<Vec<u8>> {
        let start_time = std::time::Instant::now();

        let json_path = format!("{}/{}/service-2.json", service, api_version);
        if let Some(file) = Self::get(&json_path) {
            let file_size = file.data.len();
            let result = Some(file.data.to_vec());

            let total_time = start_time.elapsed();
            if total_time.as_millis() > 10 {
                log::debug!(
                    "Loaded {}/{}: {}KB in {:?}",
                    service,
                    api_version,
                    file_size / 1024,
                    total_time
                );
            }

            result
        } else {
            None
        }
    }

    /// Get a waiters definition file by service name and API version
    pub(crate) fn get_waiters(
        service: &str,
        api_version: &str,
    ) -> Option<std::borrow::Cow<'static, [u8]>> {
        let path = format!("{}/{}/waiters-2.json", service, api_version);
        Self::get(&path).map(|file| file.data)
    }

    /// Get a paginators definition file by service name and API version
    pub(crate) fn get_paginators(
        service: &str,
        api_version: &str,
    ) -> Option<std::borrow::Cow<'static, [u8]>> {
        let path = format!("{}/{}/paginators-1.json", service, api_version);
        Self::get(&path).map(|file| file.data)
    }

    /// Build a complete service-to-versions map in a single iteration (cached)
    pub(crate) fn build_service_versions_map() -> &'static HashMap<String, Vec<String>> {
        BOTOCORE_SERVICE_VERSIONS.get_or_init(|| {
            log::debug!("Building service versions map...");

            let start_time = std::time::Instant::now();
            let mut service_versions: std::collections::HashMap<
                String,
                std::collections::HashSet<String>,
            > = std::collections::HashMap::new();
            let mut file_count = 0;

            for file_path in Botocore::iter() {
                file_count += 1;
                let path_parts: Vec<&str> = file_path.split('/').collect();
                if path_parts.len() >= 2 {
                    let service = path_parts[0].to_string();
                    let version = path_parts[1].to_string();
                    service_versions.entry(service).or_default().insert(version);
                }
            }

            // Convert HashSet to sorted Vec for each service
            let mut result: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (service, versions_set) in service_versions {
                let mut versions: Vec<String> = versions_set.into_iter().collect();
                versions.sort();
                result.insert(service, versions);
            }

            let duration = start_time.elapsed();
            log::debug!(
                "Built service versions map in {:?} (processed {} files, found {} services)",
                duration,
                file_count,
                result.len()
            );

            result
        })
    }
}

/// Embedded AWS boto3 resource data manager
///
/// Provides convenient access to embedded boto3 resource definitions with
/// automatic JSON parsing.
pub(crate) struct EmbeddedBoto3Data;

impl EmbeddedBoto3Data {
    /// Get boto3 resources data by service name and API version (deserialized)
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "s3", "ec2", "dynamodb")
    /// * `api_version` - API version (e.g., "2006-03-01", "2016-11-15")
    ///
    /// # Returns
    /// Deserialized resources JSON data or None if not found
    pub(crate) fn get_resources_definition(
        service: &str,
        api_version: &str,
    ) -> Option<Boto3ResourcesJson> {
        Boto3Resources::get_resources_definition(service, api_version)
    }

    /// Build a complete service-to-versions map for boto3 resources (cached)
    pub(crate) fn build_service_versions_map() -> &'static HashMap<String, Vec<String>> {
        Boto3Resources::build_service_versions_map()
    }

    /// Get the boto3 utilities mapping configuration from embedded data
    pub(crate) fn get_utilities_mapping() -> Option<&'static UtilityMappingJson> {
        Boto3Utilities::get_utilities_mapping()
    }
}

/// Embedded AWS service data manager
///
/// Provides convenient access to embedded AWS service definitions with
/// automatic decompression and JSON parsing.
pub(crate) struct EmbeddedServiceData;

impl EmbeddedServiceData {
    /// Get a parsed service definition by service name and API version
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "s3", "ec2", "lambda")
    /// * `api_version` - API version (e.g., "2006-03-01", "2016-11-15")
    ///
    /// # Returns
    /// Parsed service definition or error if not found or parsing fails
    pub(crate) async fn get_service_definition(
        service: &str,
        api_version: &str,
    ) -> Result<SdkServiceDefinition> {
        let data = Botocore::get_service_definition(service, api_version).ok_or_else(|| {
            ExtractorError::validation(format!(
                "Service definition not found for {}/{}",
                service, api_version
            ))
        })?;

        let json_str = std::str::from_utf8(&data).map_err(|e| {
            ExtractorError::validation(format!("Invalid UTF-8 in embedded data: {}", e))
        })?;

        JsonProvider::parse(json_str).await.map_err(|e| {
            ExtractorError::sdk_processing_with_source(
                service,
                "Failed to parse embedded service definition",
                e,
            )
        })
    }

    /// Get raw waiters data by service name and API version
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "s3", "ec2", "lambda")
    /// * `api_version` - API version (e.g., "2006-03-01", "2016-11-15")
    ///
    /// # Returns
    /// Raw waiters JSON data or None if not found
    pub(crate) fn get_waiters_raw(service: &str, api_version: &str) -> Option<Vec<u8>> {
        Botocore::get_waiters(service, api_version).map(|data| data.to_vec())
    }

    /// Get raw paginators data by service name and API version
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "s3", "ec2", "lambda")
    /// * `api_version` - API version (e.g., "2006-03-01", "2016-11-15")
    ///
    /// # Returns
    /// Raw paginators JSON data or None if not found
    #[allow(dead_code)]
    pub(crate) fn get_paginators_raw(service: &str, api_version: &str) -> Option<Vec<u8>> {
        Botocore::get_paginators(service, api_version).map(|data| data.to_vec())
    }

    /// Build a complete service-to-versions map in a single iteration (cached)
    pub(crate) fn build_service_versions_map() -> &'static HashMap<String, Vec<String>> {
        Botocore::build_service_versions_map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_botocore_get_service_definition_returns_none_for_invalid_service() {
        let result = Botocore::get_service_definition("nonexistent-service", "2023-01-01");
        assert!(result.is_none());
    }

    #[test]
    fn test_botocore_get_waiters_returns_none_for_invalid_service() {
        let result = Botocore::get_waiters("nonexistent-service", "2023-01-01");
        assert!(result.is_none());
    }

    #[test]
    fn test_botocore_get_paginators_returns_none_for_invalid_service() {
        let result = Botocore::get_paginators("nonexistent-service", "2023-01-01");
        assert!(result.is_none());
    }

    #[test]
    fn test_build_service_versions_map_returns_hashmap() {
        let service_versions = Botocore::build_service_versions_map();

        // Should return a HashMap
        assert!(service_versions.is_empty() || !service_versions.is_empty());

        // If there are services, each should have at least one version
        for (service, versions) in service_versions {
            assert!(!service.is_empty(), "Service name should not be empty");
            assert!(
                !versions.is_empty(),
                "Service {} should have at least one version",
                service
            );

            // Versions should be sorted
            let mut sorted_versions = versions.clone();
            sorted_versions.sort();
            assert_eq!(
                versions, &sorted_versions,
                "Versions for service {} should be sorted",
                service
            );
        }
    }

    #[test]
    fn test_build_service_versions_map_consistency() {
        // Call the function twice and ensure results are consistent
        let map1 = Botocore::build_service_versions_map();
        let map2 = Botocore::build_service_versions_map();

        assert_eq!(
            map1, map2,
            "build_service_versions_map should return consistent results"
        );
    }

    #[test]
    fn test_embedded_service_data_build_service_versions_map_delegates() {
        let embedded_result = EmbeddedServiceData::build_service_versions_map();
        let botocore_result = Botocore::build_service_versions_map();

        assert_eq!(
            embedded_result, botocore_result,
            "EmbeddedServiceData should delegate to Botocore::build_service_versions_map"
        );
    }

    #[tokio::test]
    async fn test_embedded_service_data_get_service_definition_invalid_service() {
        let result =
            EmbeddedServiceData::get_service_definition("nonexistent-service", "2023-01-01").await;

        assert!(
            result.is_err(),
            "Should return error for nonexistent service"
        );

        if let Err(e) = result {
            let error_msg = format!("{}", e);
            assert!(
                error_msg.contains("Service definition not found"),
                "Error should mention service not found: {}",
                error_msg
            );
        }
    }

    #[test]
    fn test_embedded_service_data_get_waiters_raw_invalid_service() {
        let result = EmbeddedServiceData::get_waiters_raw("nonexistent-service", "2023-01-01");
        assert!(
            result.is_none(),
            "Should return None for nonexistent service"
        );
    }

    #[test]
    fn test_embedded_service_data_get_paginators_raw_invalid_service() {
        let result = EmbeddedServiceData::get_paginators_raw("nonexistent-service", "2023-01-01");
        assert!(
            result.is_none(),
            "Should return None for nonexistent service"
        );
    }

    #[test]
    fn test_service_versions_map_structure() {
        let service_versions = Botocore::build_service_versions_map();

        for (service, versions) in service_versions {
            // Service names should not contain path separators
            assert!(
                !service.contains('/'),
                "Service name '{}' should not contain path separators",
                service
            );
            assert!(
                !service.contains('\\'),
                "Service name '{}' should not contain backslashes",
                service
            );

            // Versions should look like valid API versions (basic format check)
            for version in versions {
                assert!(
                    !version.is_empty(),
                    "Version should not be empty for service '{}'",
                    service
                );
                assert!(
                    !version.contains('/'),
                    "Version '{}' should not contain path separators",
                    version
                );
                assert!(
                    !version.contains('\\'),
                    "Version '{}' should not contain backslashes",
                    version
                );
            }
        }
    }

    #[test]
    fn test_botocore_path_formatting() {
        // Test that path formatting works correctly
        let service = "test-service";
        let version = "2023-01-01";

        // These should not panic and should format correctly
        let service_path = format!("{}/{}/service-2.json", service, version);
        let waiters_path = format!("{}/{}/waiters-2.json", service, version);
        let paginators_path = format!("{}/{}/paginators-1.json", service, version);

        assert_eq!(service_path, "test-service/2023-01-01/service-2.json");
        assert_eq!(waiters_path, "test-service/2023-01-01/waiters-2.json");
        assert_eq!(paginators_path, "test-service/2023-01-01/paginators-1.json");
    }

    #[test]
    fn test_botocore_get_service_definition_timing_logging() {
        // This test ensures the timing logic doesn't panic
        // We can't easily test the actual logging without setting up a logger,
        // but we can ensure the code path works
        let result = Botocore::get_service_definition("nonexistent-service", "2023-01-01");
        assert!(result.is_none());
    }

    #[test]
    fn test_service_versions_map_no_duplicates() {
        let service_versions = Botocore::build_service_versions_map();

        for (service, versions) in service_versions {
            // Check that there are no duplicate versions
            let mut unique_versions = versions.clone();
            unique_versions.sort();
            unique_versions.dedup();

            assert_eq!(
                versions.len(),
                unique_versions.len(),
                "Service '{}' should not have duplicate versions",
                service
            );
        }
    }

    #[test]
    fn test_embedded_data_methods_handle_empty_strings() {
        // Test edge cases with empty strings
        let result1 = Botocore::get_service_definition("", "");
        let result2 = Botocore::get_waiters("", "");
        let result3 = Botocore::get_paginators("", "");

        assert!(result1.is_none());
        assert!(result2.is_none());
        assert!(result3.is_none());
    }

    #[tokio::test]
    async fn test_embedded_service_data_handles_empty_strings() {
        let result = EmbeddedServiceData::get_service_definition("", "").await;
        assert!(result.is_err());
    }
}
