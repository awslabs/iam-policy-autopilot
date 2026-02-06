//! Service Configuration loader with caching capabilities.
//!
//! This module provides functionality to load service configuration files
//! from embedded data with caching for performance optimization.

use crate::errors::Result;
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, OnceLock},
};

/// Operation rename configuration
#[derive(Clone, Debug, Deserialize)]
// TODO: remove
#[allow(dead_code)]
pub(crate) struct OperationRename {
    /// Target service name
    pub(crate) service: String,
    /// Target operation name
    pub(crate) operation: String,
}

/// Service configuration
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ServiceConfiguration {
    /// Service renames
    pub(crate) rename_services_operation_action_map: HashMap<String, String>,
    /// Service renames
    pub(crate) rename_services_service_reference: HashMap<String, String>,
    /// Smithy to Botocore model: service renames
    pub(crate) smithy_botocore_service_name_mapping: HashMap<String, String>,
    /// Resource overrides
    pub(crate) resource_overrides: HashMap<String, HashMap<String, String>>,
}

impl ServiceConfiguration {
    pub(crate) fn rename_service_operation_action_map<'a>(
        &self,
        original: &'a str,
    ) -> Cow<'a, str> {
        match self.rename_services_operation_action_map.get(original) {
            Some(renamed) => Cow::Owned(renamed.clone()),
            None => Cow::Borrowed(original),
        }
    }

    pub(crate) fn rename_service_service_reference<'a>(&self, original: &'a str) -> Cow<'a, str> {
        match self.rename_services_service_reference.get(original) {
            Some(renamed) => Cow::Owned(renamed.clone()),
            None => Cow::Borrowed(original),
        }
    }
}

/// Embedded service configuration data
#[derive(RustEmbed)]
#[folder = "resources/config"]
#[include = "service-configuration.json"]
struct EmbeddedServiceConfig;

/// Static cache for the service configuration
static SERVICE_CONFIG_CACHE: OnceLock<Arc<ServiceConfiguration>> = OnceLock::new();

/// Load and cache the embedded service configuration
///
/// This function loads the service configuration from embedded data and caches it
/// for subsequent calls, similar to how botocore data is handled.
///
/// # Returns
/// An Arc to the cached service configuration, or an error if loading/parsing fails
///
/// # Errors
/// Returns `ExtractorError` if:
/// - The embedded service configuration file is not found
/// - The file contains invalid JSON
/// - The JSON structure doesn't match ServiceConfiguration
pub(crate) fn load_service_configuration() -> Result<Arc<ServiceConfiguration>> {
    let config = SERVICE_CONFIG_CACHE.get_or_init(|| {
        let embedded_file = EmbeddedServiceConfig::get("service-configuration.json")
            .expect("Embedded service configuration file not found");

        let json_str = std::str::from_utf8(&embedded_file.data)
            .expect("Invalid UTF-8 in embedded service configuration");

        let service_config: ServiceConfiguration = serde_json::from_str(json_str)
            .expect("Failed to parse embedded service configuration JSON");

        Arc::new(service_config)
    });

    Ok(config.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_service_configuration_embedded() {
        // Test loading the embedded service configuration
        let config = load_service_configuration().unwrap();

        // Verify the configuration has expected structure
        assert!(!config.rename_services_operation_action_map.is_empty());

        // Test that subsequent calls return the same cached data
        let config2 = load_service_configuration().unwrap();

        // Since we're returning clones of the same cached data, they should be equal
        assert_eq!(
            config.rename_services_operation_action_map,
            config2.rename_services_operation_action_map
        );
    }

    #[test]
    fn test_service_configuration_rename_methods() {
        let config = ServiceConfiguration {
            rename_services_operation_action_map: [(
                "old-service".to_string(),
                "new-service".to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
            rename_services_service_reference: HashMap::new(),
            smithy_botocore_service_name_mapping: HashMap::new(),
            resource_overrides: HashMap::new(),
        };

        // Test service renaming
        assert_eq!(
            config.rename_service_operation_action_map("old-service"),
            "new-service"
        );
        assert_eq!(
            config.rename_service_operation_action_map("unchanged-service"),
            "unchanged-service"
        );
    }

    #[test]
    fn test_embedded_service_configuration_content() {
        // Load the actual embedded configuration and verify it has expected content
        let config = load_service_configuration().unwrap();

        // Test some known renames
        assert_eq!(
            config
                .rename_services_operation_action_map
                .get("accessanalyzer"),
            Some(&"access-analyzer".to_string())
        );
        assert_eq!(
            config
                .rename_services_operation_action_map
                .get("stepfunctions"),
            Some(&"states".to_string())
        );
    }
}

#[cfg(test)]
mod negative_tests {
    use rust_embed::RustEmbed;

    use super::ServiceConfiguration;

    /// Embedded invalid test configuration files for negative testing
    /// This RustEmbed points to test resources with intentionally malformed configs
    #[derive(RustEmbed)]
    #[folder = "tests/resources/invalid_configs"]
    #[include = "*.json"]
    struct InvalidTestConfigs;

    #[test]
    fn test_invalid_service_configuration() {
        let file_paths = [
            "invalid_service_config1.json",
            "invalid_service_config2.json",
        ];
        for file_path in file_paths {
            // Test that malformed JSON (missing closing brace) is rejected
            let file = InvalidTestConfigs::get(file_path).expect("Test file should exist");

            let json_str =
                std::str::from_utf8(&file.data).expect("Test file should be valid UTF-8");

            let result: Result<ServiceConfiguration, _> = serde_json::from_str(json_str);

            assert!(
                result.is_err(),
                "{}: Parsing should fail for malformed JSON",
                file_path
            );
        }
    }

    #[test]
    fn test_invalid_configs_directory_exists() {
        // Verify that the test resources directory is properly set up
        let file_count = InvalidTestConfigs::iter().count();

        assert!(
            file_count > 0,
            "Should have at least one invalid test configuration file"
        );

        println!("✓ Found {} invalid test configuration files", file_count);
    }
}
