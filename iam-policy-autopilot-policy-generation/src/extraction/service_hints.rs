//! Service hints processing and validation
//!
//! This module provides functionality for validating and expanding service hints
//! to handle service name mappings from both SmithyBotocoreServiceNameMapping
//! and RenameServicesServiceReference configurations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use log::trace;

use crate::api::model::ServiceHints;
use crate::errors::ExtractorError;
use crate::extraction::sdk_model::ServiceModelIndex;
use crate::service_configuration::ServiceConfiguration;
use crate::ExtractedMethods;

use anyhow::Result;

/// Service hints processor that handles validation and filtering with service name mappings
pub(crate) struct ServiceHintsProcessor {
    /// Original service hints provided by the user
    hints: ServiceHints,
    /// Map from service name to all its synonyms (including the service name itself)
    /// Example: "logs" -> ["logs", "cloudwatch-logs", "cloudwatchlogs"]
    service_synonyms: HashMap<String, HashSet<String>>,
}

impl ServiceHintsProcessor {
    /// Create a new ServiceHintsProcessor
    ///
    /// This will compute the expanded hints set that includes all service name variants
    /// based on the mappings in the service configuration.
    pub(crate) fn new(
        hints: ServiceHints,
        service_config: Arc<ServiceConfiguration>,
        service_index: Arc<ServiceModelIndex>,
    ) -> Self {
        let service_synonyms = Self::compute_service_synonyms(&service_index, &service_config);

        Self {
            hints,
            service_synonyms,
        }
    }

    /// Compute synonyms for each service in the service index
    ///
    /// For each service, creates a set of all names that should match it, including:
    /// - The service name itself
    /// - SmithyBotocoreServiceNameMapping (bidirectional)
    /// - RenameServicesServiceReference (bidirectional)
    /// - Dash-less variants of all names
    ///
    /// Returns a map: service_name -> set of synonyms
    fn compute_service_synonyms(
        service_index: &ServiceModelIndex,
        service_config: &ServiceConfiguration,
    ) -> HashMap<String, HashSet<String>> {
        // Pre-compute reverse lookup maps for efficient processing
        let mut botocore_to_smithy: HashMap<String, Vec<String>> = HashMap::new();
        for (smithy_name, botocore_name) in &service_config.smithy_botocore_service_name_mapping {
            botocore_to_smithy
                .entry(botocore_name.clone())
                .or_default()
                .push(smithy_name.clone());
        }

        let mut target_to_sources: HashMap<String, Vec<String>> = HashMap::new();
        for (source_service, target_service) in &service_config.rename_services_service_reference {
            target_to_sources
                .entry(target_service.clone())
                .or_default()
                .push(source_service.clone());
        }

        let mut synonyms_map = HashMap::new();

        // For each service in the index, compute all its synonyms
        for service_name in service_index.services.keys() {
            let mut synonyms = HashSet::new();

            // Add the service name itself
            synonyms.insert(service_name.clone());

            // Add dash-less variant
            if service_name.contains('-') {
                synonyms.insert(service_name.replace('-', ""));
            }

            // SmithyBotocoreServiceNameMapping: if this is a botocore name, add smithy names
            if let Some(smithy_names) = botocore_to_smithy.get(service_name) {
                for smithy_name in smithy_names {
                    synonyms.insert(smithy_name.clone());
                    // Add dash-less variant of smithy name
                    if smithy_name.contains('-') {
                        synonyms.insert(smithy_name.replace('-', ""));
                    }
                }
            }

            // SmithyBotocoreServiceNameMapping: if this is a smithy name, add botocore name
            if let Some(botocore_name) = service_config
                .smithy_botocore_service_name_mapping
                .get(service_name)
            {
                synonyms.insert(botocore_name.clone());
                // Add dash-less variant of botocore name
                if botocore_name.contains('-') {
                    synonyms.insert(botocore_name.replace('-', ""));
                }
            }

            // RenameServicesServiceReference: if this service maps to a target, add the target
            if let Some(target_service) = service_config
                .rename_services_service_reference
                .get(service_name)
            {
                synonyms.insert(target_service.clone());
                // Add dash-less variant of target
                if target_service.contains('-') {
                    synonyms.insert(target_service.replace('-', ""));
                }
            }

            // RenameServicesServiceReference: if other services map to this as target, add them
            if let Some(source_services) = target_to_sources.get(service_name) {
                for source_service in source_services {
                    synonyms.insert(source_service.clone());
                    // Add dash-less variant of source
                    if source_service.contains('-') {
                        synonyms.insert(source_service.replace('-', ""));
                    }
                }
            }

            trace!("Service '{}' synonyms: {:?}", service_name, synonyms);
            synonyms_map.insert(service_name.clone(), synonyms);
        }

        synonyms_map
    }

    /// Validate that all service hints are valid
    ///
    /// Checks if each hint (or its expanded variants) corresponds to a valid service
    /// in the service index. Returns an error with suggestions if any hints are invalid.
    pub(crate) fn validate(&self) -> Result<()> {
        let mut invalid_services = Vec::new();

        for hint in &self.hints.service_names {
            // Check if this hint matches any service's synonyms
            let is_valid = self
                .service_synonyms
                .values()
                .any(|synonyms| synonyms.contains(hint));

            if !is_valid {
                // Find suggestions using Levenshtein distance
                let suggestions = self.find_similar_services(hint);
                invalid_services.push((hint.clone(), suggestions));
            }
        }

        if !invalid_services.is_empty() {
            let mut error_msg = String::new();
            for (invalid_service, suggestions) in invalid_services {
                if suggestions.is_empty() {
                    error_msg.push_str(&format!("  - {}\n", invalid_service));
                } else if suggestions.len() == 1 {
                    error_msg.push_str(&format!(
                        "  - {} (did you mean {}?)\n",
                        invalid_service, suggestions[0]
                    ));
                } else {
                    let suggestion_list = suggestions.join(", ");
                    error_msg.push_str(&format!(
                        "  - {} (did you mean one of: {}?)\n",
                        invalid_service, suggestion_list
                    ));
                }
            }
            return Err(ExtractorError::invalid_service_hints(error_msg).into());
        }

        Ok(())
    }

    /// Find similar service names using Levenshtein distance
    ///
    /// Searches through service names and their synonyms to find close matches
    fn find_similar_services(&self, target: &str) -> Vec<String> {
        let mut matches = Vec::new();

        for synonyms in self.service_synonyms.values() {
            for synonym in synonyms {
                let distance = strsim::levenshtein(target, synonym);
                if distance <= 2 {
                    matches.push(synonym.clone());
                }
            }
        }

        matches.sort();
        matches.dedup();
        matches
    }

    /// Filter extracted methods to only include those matching the service hints
    pub(crate) fn filter(&self, results: &mut ExtractedMethods) {
        results.methods.retain_mut(|method_call| {
            // Keep services where any of the user hints matches the service's synonyms
            method_call.possible_services.retain(|service| {
                if let Some(synonyms) = self.service_synonyms.get(service) {
                    // Check if any user hint matches any synonym of this service
                    self.hints
                        .service_names
                        .iter()
                        .any(|hint| synonyms.contains(hint))
                } else {
                    false
                }
            });

            // Keep the method call only if it still has at least one possible service
            !method_call.possible_services.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::sdk_model::ServiceDiscovery;
    use crate::extraction::{ExtractionMetadata, SdkMethodCall};
    use crate::service_configuration::load_service_configuration;
    use crate::Language;

    #[tokio::test]
    async fn test_service_hints_processor_validation() {
        let service_config = load_service_configuration().expect("Failed to load config");
        let service_index = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index");

        // Test valid hints - "chime" and "bedrock" should expand to valid services
        let hints = ServiceHints {
            service_names: vec!["chime".to_string(), "bedrock".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        assert!(
            processor.validate().is_ok(),
            "chime and bedrock should be valid hints that expand to real services"
        );

        // Test "elasticloadbalancing" - should be valid because it expands to elb and elbv2
        let hints = ServiceHints {
            service_names: vec!["elasticloadbalancing".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        assert!(
            processor.validate().is_ok(),
            "elasticloadbalancing should be valid because it's a synonym of elb and elbv2"
        );
        // Verify elb and elbv2 have elasticloadbalancing as a synonym
        assert!(
            processor
                .service_synonyms
                .get("elb")
                .unwrap()
                .contains("elasticloadbalancing"),
            "elb should have elasticloadbalancing as synonym"
        );
        assert!(
            processor
                .service_synonyms
                .get("elbv2")
                .unwrap()
                .contains("elasticloadbalancing"),
            "elbv2 should have elasticloadbalancing as synonym"
        );

        // Test invalid hint
        let hints = ServiceHints {
            service_names: vec!["totally-invalid-xyz".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));
        assert!(
            processor.validate().is_err(),
            "totally-invalid-xyz should be invalid"
        );
    }

    #[test]
    fn test_service_hints_processor_filtering() {
        use crate::extraction::sdk_model::{SdkServiceDefinition, ServiceMetadata};

        let service_config = load_service_configuration().expect("Failed to load config");

        // Helper to create a minimal service definition
        let create_service_def = || SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2020-01-01".to_string(),
                service_id: "Test".to_string(),
            },
            operations: HashMap::new(),
            shapes: HashMap::new(),
        };

        // Create a minimal service index for testing
        let service_index = Arc::new(ServiceModelIndex {
            services: [
                ("logs".to_string(), create_service_def()),
                ("bedrock-agent".to_string(), create_service_def()),
                ("bedrock-runtime".to_string(), create_service_def()),
                ("chime-sdk-messaging".to_string(), create_service_def()),
            ]
            .iter()
            .cloned()
            .collect(),
            method_lookup: HashMap::new(),
            waiter_lookup: HashMap::new(),
        });

        let hints = ServiceHints {
            service_names: vec!["bedrock".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));

        let mut results = ExtractedMethods {
            methods: vec![
                SdkMethodCall {
                    name: "invoke_model".to_string(),
                    possible_services: vec!["bedrock-runtime".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "create_agent".to_string(),
                    possible_services: vec!["bedrock-agent".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "put_log_events".to_string(),
                    possible_services: vec!["logs".to_string()],
                    metadata: None,
                },
            ],
            metadata: ExtractionMetadata::new(vec![], vec![]),
        };

        processor.filter(&mut results);

        // Should keep only bedrock-related methods
        assert_eq!(results.methods.len(), 2);
        assert!(results
            .methods
            .iter()
            .all(|m| m.possible_services.iter().any(|s| s.starts_with("bedrock"))));
    }

    #[test]
    fn test_service_synonyms_computation() {
        use crate::extraction::sdk_model::{SdkServiceDefinition, ServiceMetadata};

        let service_config = load_service_configuration().expect("Failed to load config");

        // Helper to create a minimal service definition
        let create_service_def = || SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2020-01-01".to_string(),
                service_id: "Test".to_string(),
            },
            operations: HashMap::new(),
            shapes: HashMap::new(),
        };

        // Create a service index with logs service
        let service_index = Arc::new(ServiceModelIndex {
            services: [
                ("logs".to_string(), create_service_def()),
                ("elb".to_string(), create_service_def()),
                ("elbv2".to_string(), create_service_def()),
            ]
            .iter()
            .cloned()
            .collect(),
            method_lookup: HashMap::new(),
            waiter_lookup: HashMap::new(),
        });

        let hints = ServiceHints {
            service_names: vec!["cloudwatch-logs".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));

        // The "logs" service should have these synonyms
        let logs_synonyms = processor
            .service_synonyms
            .get("logs")
            .expect("logs service should exist");
        assert!(
            logs_synonyms.contains("logs"),
            "logs should be its own synonym"
        );
        assert!(
            logs_synonyms.contains("cloudwatch-logs"),
            "logs should have cloudwatch-logs as synonym"
        );
        assert!(
            logs_synonyms.contains("cloudwatchlogs"),
            "logs should have cloudwatchlogs as synonym"
        );

        // The "elb" and "elbv2" services should both have "elasticloadbalancing" as a synonym
        let elb_synonyms = processor
            .service_synonyms
            .get("elb")
            .expect("elb service should exist");
        assert!(
            elb_synonyms.contains("elb"),
            "elb should be its own synonym"
        );
        assert!(
            elb_synonyms.contains("elasticloadbalancing"),
            "elb should have elasticloadbalancing as synonym"
        );

        let elbv2_synonyms = processor
            .service_synonyms
            .get("elbv2")
            .expect("elbv2 service should exist");
        assert!(
            elbv2_synonyms.contains("elbv2"),
            "elbv2 should be its own synonym"
        );
        assert!(
            elbv2_synonyms.contains("elasticloadbalancing"),
            "elbv2 should have elasticloadbalancing as synonym"
        );
    }

    #[test]
    fn test_service_hints_with_multiple_matching_services() {
        use crate::extraction::sdk_model::{SdkServiceDefinition, ServiceMetadata};

        let service_config = load_service_configuration().expect("Failed to load config");

        // Helper to create a minimal service definition
        let create_service_def = || SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2020-01-01".to_string(),
                service_id: "Test".to_string(),
            },
            operations: HashMap::new(),
            shapes: HashMap::new(),
        };

        // Create a service index with services A and B
        let service_index = Arc::new(ServiceModelIndex {
            services: [
                ("A".to_string(), create_service_def()),
                ("B".to_string(), create_service_def()),
                ("C".to_string(), create_service_def()),
            ]
            .iter()
            .cloned()
            .collect(),
            method_lookup: HashMap::new(),
            waiter_lookup: HashMap::new(),
        });

        // Service hints with both A and B
        let hints = ServiceHints {
            service_names: vec!["A".to_string(), "B".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));

        let mut results = ExtractedMethods {
            methods: vec![
                SdkMethodCall {
                    name: "method_ab".to_string(),
                    possible_services: vec!["A".to_string(), "B".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "method_a".to_string(),
                    possible_services: vec!["A".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "method_b".to_string(),
                    possible_services: vec!["B".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "method_c".to_string(),
                    possible_services: vec!["C".to_string()],
                    metadata: None,
                },
            ],
            metadata: ExtractionMetadata::new(vec![], vec![]),
        };

        processor.filter(&mut results);

        // Should keep all methods that have A or B as possible services
        assert_eq!(
            results.methods.len(),
            3,
            "Should keep 3 methods (method_ab, method_a, method_b)"
        );

        // Verify method_ab still has both A and B
        let method_ab = results
            .methods
            .iter()
            .find(|m| m.name == "method_ab")
            .expect("method_ab should be present");
        assert_eq!(
            method_ab.possible_services.len(),
            2,
            "method_ab should have 2 possible services"
        );
        assert!(
            method_ab.possible_services.contains(&"A".to_string()),
            "method_ab should contain service A"
        );
        assert!(
            method_ab.possible_services.contains(&"B".to_string()),
            "method_ab should contain service B"
        );

        // Verify method_a has only A
        let method_a = results
            .methods
            .iter()
            .find(|m| m.name == "method_a")
            .expect("method_a should be present");
        assert_eq!(
            method_a.possible_services,
            vec!["A".to_string()],
            "method_a should only have service A"
        );

        // Verify method_b has only B
        let method_b = results
            .methods
            .iter()
            .find(|m| m.name == "method_b")
            .expect("method_b should be present");
        assert_eq!(
            method_b.possible_services,
            vec!["B".to_string()],
            "method_b should only have service B"
        );

        // Verify method_c was filtered out
        assert!(
            results
                .methods
                .iter()
                .find(|m| m.name == "method_c")
                .is_none(),
            "method_c should be filtered out"
        );
    }
}
