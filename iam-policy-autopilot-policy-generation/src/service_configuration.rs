//! Service Configuration loader with caching capabilities.
//!
//! This module provides functionality to load service configuration files
//! from embedded data with caching for performance optimization.

use crate::embedded_data::BotocoreData;
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

    /// Build a lookup map for normalising Java SDK import segments to Botocore service names.
    ///
    /// The Java SDK derives its package segment from the Smithy service name by **removing
    /// all dashes** (e.g. Smithy `"cloudwatch-logs"` → Java import segment `"cloudwatchlogs"`).
    /// This method iterates over [`SmithyBotocoreServiceNameMapping`], strips dashes from each
    /// Smithy key, and maps the result to the corresponding Botocore name.
    ///
    /// In addition, for every botocore service whose canonical name contains a dash (e.g.
    /// `"bedrock-runtime"`, `"s3-outposts"`) and that is **not** already covered by the
    /// Smithy mapping above, a self-mapping `dashfreevariant → dashed-name` is added so
    /// that Java import segments like `bedrockruntime` are correctly resolved to the
    /// botocore service name `bedrock-runtime`.
    ///
    /// The returned map is keyed by the dash-free Java import segment so that
    /// `extract_service_from_import` can do a single O(1) lookup.
    ///
    /// # Collisions
    ///
    /// Explicit `SmithyBotocoreServiceNameMapping` entries take priority over the
    /// auto-generated botocore self-mappings.  If two botocore names produce the same
    /// dash-free key the last entry wins (non-deterministic, but no such collisions exist
    /// in practice).
    pub(crate) fn build_java_import_service_map(&self) -> HashMap<String, String> {
        // Step 1: explicit Smithy → Botocore mappings (highest priority).
        let mut map: HashMap<String, String> = self
            .smithy_botocore_service_name_mapping
            .iter()
            .map(|(smithy_name, botocore_name)| {
                let java_segment = smithy_name.replace('-', "");
                (java_segment, botocore_name.clone())
            })
            .collect();

        // Step 2: for every botocore service name that contains a dash and is not already
        // covered by step 1, add a self-mapping dashfree → dashed so that Java import
        // segments (which always have dashes stripped) resolve to the correct service name.
        for botocore_name in BotocoreData::build_service_versions_map().into_keys() {
            if botocore_name.contains('-') {
                let dash_free = botocore_name.replace('-', "");
                map.entry(dash_free).or_insert(botocore_name);
            }
        }

        map
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
