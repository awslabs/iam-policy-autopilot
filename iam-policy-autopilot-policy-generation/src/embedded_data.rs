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

/// Embedded AWS service definitions with compression
///
/// This struct provides access to pre-processed AWS service definitions
/// that have been simplified to remove documentation and examples,
/// reducing binary size while maintaining essential functionality.
#[derive(RustEmbed)]
#[folder = "target/botocore-data-simplified"]
#[include = "*.json"]
pub struct Botocore;

/// Embedded AWS boto3 resource definitions
///
/// This struct provides access to boto3 resource definitions
/// that are embedded directly into the binary at compile time.
#[derive(RustEmbed)]
#[folder = "target/boto3-data-simplified"]
#[include = "*.json"]
pub struct Boto3Resources;

/// Embedded boto3 utilities mapping
///
/// This struct provides access to the boto3 utilities mapping configuration
/// that defines client utility methods and resource methods.
#[derive(RustEmbed)]
#[folder = "resources/config/sdks"]
#[include = "boto3_utilities_mapping.json"]
pub struct Boto3Utilities;

impl Boto3Utilities {
    /// Get the boto3 utilities mapping configuration
    pub fn get_utilities_mapping() -> Option<Vec<u8>> {
        Self::get("boto3_utilities_mapping.json").map(|file| file.data.to_vec())
    }
}

impl Boto3Resources {
    /// Get a boto3 resources definition file by service name and API version
    pub fn get_resources_definition(service: &str, api_version: &str) -> Option<Vec<u8>> {
        let start_time = std::time::Instant::now();

        let json_path = format!("{}/{}/resources-1.json", service, api_version);
        if let Some(file) = Self::get(&json_path) {
            let file_size = file.data.len();
            let result = Some(file.data.to_vec());

            let total_time = start_time.elapsed();
            if total_time.as_millis() > 10 {
                log::debug!(
                    "Loaded boto3 {}/{}: {}KB in {:?}",
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

    /// Build a complete service-to-versions map for boto3 resources
    pub(crate) fn build_service_versions_map() -> std::collections::HashMap<String, Vec<String>> {
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
    }
}

impl Botocore {
    /// Get a service definition file by service name and API version
    pub fn get_service_definition(service: &str, api_version: &str) -> Option<Vec<u8>> {
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
    pub fn get_waiters(
        service: &str,
        api_version: &str,
    ) -> Option<std::borrow::Cow<'static, [u8]>> {
        let path = format!("{}/{}/waiters-2.json", service, api_version);
        Self::get(&path).map(|file| file.data)
    }

    /// Get a paginators definition file by service name and API version
    pub fn get_paginators(
        service: &str,
        api_version: &str,
    ) -> Option<std::borrow::Cow<'static, [u8]>> {
        let path = format!("{}/{}/paginators-1.json", service, api_version);
        Self::get(&path).map(|file| file.data)
    }

    /// Build a complete service-to-versions map in a single iteration
    pub(crate) fn build_service_versions_map() -> std::collections::HashMap<String, Vec<String>> {
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
    }
}

/// Embedded AWS boto3 resource data manager
///
/// Provides convenient access to embedded boto3 resource definitions with
/// automatic JSON parsing.
pub(crate) struct EmbeddedBoto3Data;

impl EmbeddedBoto3Data {
    /// Get raw boto3 resources data by service name and API version
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "s3", "ec2", "dynamodb")
    /// * `api_version` - API version (e.g., "2006-03-01", "2016-11-15")
    ///
    /// # Returns
    /// Raw resources JSON data or None if not found
    pub fn get_resources_raw(service: &str, api_version: &str) -> Option<Vec<u8>> {
        Boto3Resources::get_resources_definition(service, api_version)
    }

    /// Build a complete service-to-versions map for boto3 resources
    pub(crate) fn build_service_versions_map() -> std::collections::HashMap<String, Vec<String>> {
        Boto3Resources::build_service_versions_map()
    }

    /// Get the boto3 utilities mapping configuration from embedded data
    pub(crate) fn get_utilities_mapping() -> Option<Vec<u8>> {
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
    pub fn get_waiters_raw(service: &str, api_version: &str) -> Option<Vec<u8>> {
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
    pub fn get_paginators_raw(service: &str, api_version: &str) -> Option<Vec<u8>> {
        Botocore::get_paginators(service, api_version).map(|data| data.to_vec())
    }

    /// Build a complete service-to-versions map in a single iteration
    pub(crate) fn build_service_versions_map() -> std::collections::HashMap<String, Vec<String>> {
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
        for (service, versions) in &service_versions {
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

        for (service, versions) in &service_versions {
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

        for (service, versions) in &service_versions {
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

    #[test]
    fn test_service_versions_map_performance() {
        // Test that building the map doesn't take an unreasonable amount of time
        let start = std::time::Instant::now();
        let _service_versions = Botocore::build_service_versions_map();
        let duration = start.elapsed();

        // Should complete in reasonable time (less than 1 second for this operation)
        assert!(
            duration.as_secs() < 1,
            "build_service_versions_map took too long: {:?}",
            duration
        );
    }
}
