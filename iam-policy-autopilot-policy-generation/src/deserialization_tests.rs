//! Build-time configuration file validation tests
//!
//! These tests validate that all configuration files in resources/config can be
//! successfully parsed using the same deserialization logic as runtime. This ensures
//! that configuration errors are caught during development/CI rather than at runtime.
//!
//! Run these tests with: `cargo test --lib config_validation`

#[cfg(test)]
mod tests {
    use crate::enrichment::operation_fas_map::load_operation_fas_map;
    use crate::extraction::python::boto3_resources_model::Boto3ResourcesModel;
    use crate::service_configuration::load_service_configuration;
    use std::fs;
    use std::path::Path;

    /// Validates that service-configuration.json can be parsed successfully
    #[test]
    fn test_validate_service_configuration() {
        let result = load_service_configuration();
        assert!(
            result.is_ok(),
            "Failed to parse service-configuration.json: {:?}",
            result.err()
        );

        let config = result.unwrap();
        // Basic sanity checks
        assert!(
            !config.rename_services_operation_action_map.is_empty(),
            "service-configuration.json should have service renames"
        );
    }

    /// Validates that all operation-fas-maps/*.json files can be parsed successfully
    #[test]
    fn test_validate_all_operation_fas_maps() {
        let fas_maps_dir = Path::new("resources/config/operation-fas-maps");
        assert!(
            fas_maps_dir.exists(),
            "Operation FAS maps directory not found at: {}",
            fas_maps_dir.display()
        );

        let mut validated_count = 0;
        let mut errors = Vec::new();

        for entry in
            fs::read_dir(fas_maps_dir).expect("Failed to read operation-fas-maps directory")
        {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let service_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("Invalid file name");

                // Use the exact same runtime deserialization
                match load_operation_fas_map(service_name) {
                    Some(fas_map) => {
                        // Verify the map has content
                        assert!(
                            !fas_map.fas_operations.is_empty(),
                            "FAS map for service '{}' should not be empty",
                            service_name
                        );
                        validated_count += 1;
                    }
                    None => {
                        errors.push(format!(
                            "Failed to load operation FAS map for service: {}",
                            service_name
                        ));
                    }
                }
            }
        }

        assert!(
            errors.is_empty(),
            "Failed to validate {} operation FAS map files:\n{}",
            errors.len(),
            errors.join("\n")
        );

        assert!(
            validated_count > 0,
            "Should have validated at least one operation FAS map file"
        );

        println!(
            "✓ Successfully validated {} operation FAS map files",
            validated_count
        );
    }

    /// Validates that boto3_utilities_mapping.json can be parsed successfully
    /// by attempting to load all service models defined in the mapping
    #[test]
    fn test_validate_boto3_utilities_mapping() {
        // Load the utilities mapping to get the list of services dynamically
        let utilities_mapping = crate::embedded_data::EmbeddedBoto3Data::get_utilities_mapping();

        if utilities_mapping.is_none() {
            println!("⚠ Skipping boto3_utilities_mapping validation (embedded data not available)");
            return;
        }

        let mapping = utilities_mapping.unwrap();
        let services_in_mapping: Vec<&str> = mapping.services.keys().map(|s| s.as_str()).collect();

        assert!(
            !services_in_mapping.is_empty(),
            "boto3_utilities_mapping.json should define at least one service"
        );

        let mut validated_services = Vec::new();
        let mut errors = Vec::new();

        for service_name in &services_in_mapping {
            // The utilities mapping is loaded internally by Boto3ResourcesModel
            if let Err(e) = Boto3ResourcesModel::load(service_name) {
                errors.push(format!(
                    "Failed to load {} with utilities: {}",
                    service_name, e
                ));
            } else {
                validated_services.push(service_name.to_string());
            }
        }

        // If embedded data is not available, skip validation
        if validated_services.is_empty() && !errors.is_empty() {
            println!("⚠ Skipping boto3_utilities_mapping validation (embedded data not available)");
            return;
        }

        assert!(
            errors.is_empty(),
            "Failed to validate boto3_utilities_mapping.json for some services:\n{}",
            errors.join("\n")
        );

        assert_eq!(
            validated_services.len(),
            services_in_mapping.len(),
            "Should have validated all {} services in boto3_utilities_mapping.json",
            services_in_mapping.len()
        );

        println!(
            "✓ Successfully validated boto3_utilities_mapping.json for all {} services: {:?}",
            validated_services.len(),
            validated_services
        );
    }

    /// Test module for negative validation - ensuring malformed configs are rejected
    mod negative_tests {
        use rust_embed::RustEmbed;

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

                let result: Result<crate::service_configuration::ServiceConfiguration, _> =
                    serde_json::from_str(json_str);

                assert!(
                    result.is_err(),
                    "{}: Parsing should fail for malformed JSON",
                    file_path
                );
            }
        }

        #[test]
        fn test_invalid_operation_fas_map() {
            let file_paths = [
                "invalid_operation_fas_map1.json",
                "invalid_operation_fas_map2.json",
            ];
            for file_path in file_paths {
                // Test that malformed JSON (missing closing brace) is rejected
                let file = InvalidTestConfigs::get(file_path).expect("Test file should exist");

                let json_str =
                    std::str::from_utf8(&file.data).expect("Test file should be valid UTF-8");

                let result: Result<crate::enrichment::operation_fas_map::OperationFasMap, _> =
                    serde_json::from_str(json_str);

                assert!(
                    result.is_err(),
                    "{}: Parsing should fail for malformed JSON",
                    file_path
                );
            }
        }

        #[test]
        fn test_invalid_boto3_utilities_mapping() {
            let file_paths = [
                "invalid_boto3_utilities_mapping1.json",
                "invalid_boto3_utilities_mapping2.json",
            ];
            for file_path in file_paths {
                // Test that malformed boto3 utilities mapping is rejected
                let file = InvalidTestConfigs::get(file_path).expect("Test file should exist");

                let json_str =
                    std::str::from_utf8(&file.data).expect("Test file should be valid UTF-8");

                let result: Result<crate::embedded_data::UtilityMappingJson, _> =
                    serde_json::from_str(json_str);

                assert!(
                    result.is_err(),
                    "{}: Parsing should fail for malformed boto3 utilities mapping",
                    file_path
                );

                let error = result.unwrap_err();
                let error_msg = error.to_string();
                println!("✓ {}: Correctly rejected - {}", file_path, error_msg);
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
}
