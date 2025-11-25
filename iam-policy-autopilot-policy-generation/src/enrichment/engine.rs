//! Core Enrichment Engine implementation
//!
//! This module provides the [`MethodEnrichmentEngine`] that orchestrates the complete
//! 3-stage enrichment pipeline: input validation, OperationAction maps enrichment, and Service Reference enrichment
//! with resource matching.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::EnrichedSdkMethodCall;
use crate::enrichment::operation_fas_map::OperationFasMaps;
use crate::enrichment::{load_operation_fas_map, ResourceMatcher, ServiceReferenceLoader};
use crate::errors::{ExtractorError, Result};
use crate::service_configuration::{self, ServiceConfiguration};
use crate::SdkMethodCall;

/// Core enrichment engine that orchestrates the 3-stage enrichment pipeline
///
/// The MethodEnrichmentEngine coordinates all enrichment components to transform
/// ExtractedMethods into FullyEnrichedMethods through a systematic 3-stage process:
#[derive(Debug)]
#[non_exhaustive]
pub struct Engine {
    /// Service Reference loader
    service_reference_loader: ServiceReferenceLoader,
}

impl Engine {
    /// Create a new MethodEnrichmentEngine with custom base paths
    ///
    /// Allows customization of the base paths for OperationAction maps and Service Reference files,
    /// useful for testing or alternative configurations.
    pub fn new(disable_file_system_cache: bool) -> Result<Self> {
        Ok(Self {
            service_reference_loader: ServiceReferenceLoader::new(disable_file_system_cache)?,
        })
    }

    /// This is the main entry point for the enrichment process.
    /// 1. Maps operations to authorized actions
    /// 2. Expands actions using the FAS (Forward-Access Sessions) model
    pub async fn enrich_methods<'a>(
        &mut self,
        extracted_methods: &'a [SdkMethodCall],
    ) -> Result<Vec<EnrichedSdkMethodCall<'a>>> {
        let unique_services = self.get_unique_services(extracted_methods);

        let service_cfg = service_configuration::load_service_configuration()?;

        let fas_maps = self
            .load_fas_maps_for_services(&unique_services, &service_cfg)
            .await?;

        let resource_matcher = ResourceMatcher::new(service_cfg, fas_maps);
        let enriched_calls = self
            .enrich_all_methods(extracted_methods, &resource_matcher)
            .await?;

        Ok(enriched_calls)
    }

    /// Extract unique service names from ExtractedMethods
    pub(crate) fn get_unique_services(&self, extracted_methods: &[SdkMethodCall]) -> Vec<String> {
        let mut services = HashSet::new();

        for method in extracted_methods {
            for service in &method.possible_services {
                services.insert(service.clone());
            }
        }

        let mut result: Vec<String> = services.into_iter().collect();
        result.sort();
        result
    }

    /// Load FAS maps for all specified services
    async fn load_fas_maps_for_services(
        &self,
        services: &[String],
        service_cfg: &Arc<ServiceConfiguration>,
    ) -> Result<OperationFasMaps> {
        let mut fas = HashMap::new();
        let mut loaded_fas_maps = Vec::new();

        for service in services {
            log::debug!("FAS: processing service: {}", service);

            let renamed_service = service_cfg.rename_service_operation_action_map(service);
            if renamed_service.as_ref() != service {
                log::debug!(
                    "Service '{}' renamed to '{}' for operation action map lookup",
                    service,
                    renamed_service
                );
            }

            match load_operation_fas_map(renamed_service.as_ref()) {
                None => {
                    log::debug!(
                        "No operation FAS map found for service '{}' (expected)",
                        renamed_service
                    );
                    // We don't expect that a service also has a FAS map
                    continue;
                }
                Some(operation_fas_map) => {
                    log::debug!(
                        "Successfully loaded operation FAS map for service '{}' ({} operations)",
                        renamed_service,
                        operation_fas_map.fas_operations.len()
                    );
                    loaded_fas_maps.push(renamed_service.clone());
                    fas.insert(renamed_service.to_string(), operation_fas_map);
                }
            }
        }

        Ok(fas)
    }

    /// Enrich all method calls using loaded OperationAction maps, service
    /// references and the FAS model.
    async fn enrich_all_methods<'a>(
        &mut self,
        methods: &'a [SdkMethodCall],
        resource_matcher: &ResourceMatcher,
    ) -> Result<Vec<EnrichedSdkMethodCall<'a>>> {
        let mut enriched_calls = Vec::new();

        for method in methods {
            match resource_matcher
                .enrich_method_call(method, &self.service_reference_loader)
                .await
            {
                Ok(mut method_calls) => {
                    enriched_calls.append(&mut method_calls);
                }
                Err(e) => {
                    return Err(ExtractorError::enrichment_error(
                        &method.name,
                        format!("Failed to enrich method call: {}", e),
                    ));
                }
            }
        }

        Ok(enriched_calls)
    }
}

// Implement Clone for MethodEnrichmentEngine when providers support it

#[cfg(test)]
mod tests {
    use crate::{extraction::sdk_model::ServiceDiscovery, Language};

    use super::*;

    fn create_test_extracted_methods() -> Vec<SdkMethodCall> {
        vec![SdkMethodCall {
            name: "GetObject".to_string(),
            possible_services: vec!["s3".to_string(), "s3".to_string()],
            metadata: None,
        }]
    }

    #[test]
    fn test_get_unique_services() {
        let engine = Engine::new(false).unwrap();

        let extracted_methods = create_test_extracted_methods();
        let services = engine.get_unique_services(&extracted_methods);

        assert_eq!(services.len(), 1);
        assert!(services.contains(&"s3".to_string()));
    }

    #[test_log::test(tokio::test)]
    async fn test_enrichment_engine_comprehensive() {
        use std::time::Instant;

        let start_time = Instant::now();

        let service_index = match ServiceDiscovery::load_service_index(Language::Python).await {
            Ok(result) => result,
            Err(e) => {
                panic!("Failed to discover services: {}", e);
            }
        };

        println!("\nCreating SdkMethodCall objects from operations...");
        let sdk_method_calls = create_sdk_method_calls_from_service_index(&service_index);

        println!("Created {} SdkMethodCall objects", sdk_method_calls.len());

        if sdk_method_calls.is_empty() {
            panic!("No operations found to test - this may indicate missing botocore data");
        }

        println!("\nSetting up enrichment engine...");
        let mut enrichment_engine = Engine::new(true).unwrap();
        println!("Enrichment engine initialized");

        println!("\nRunning enrichment on all operations...");
        let enrichment_start = Instant::now();

        match enrichment_engine.enrich_methods(&sdk_method_calls).await {
            Ok(enriched_calls) => {
                let enrichment_duration = enrichment_start.elapsed();

                println!("Enrichment Results:");
                println!("   • Input operations: {}", sdk_method_calls.len());
                println!("   • Enriched calls: {}", enriched_calls.len());
                println!("   • Processing time: {:?}", enrichment_duration);

                println!("\nAnalyzing enrichment results...");
                analyze_enrichment_results(&enriched_calls);

                // Assertions
                assert!(
                    !enriched_calls.is_empty(),
                    "Should have at least some enriched calls"
                );

                // Verify structure of enriched calls
                for enriched_call in enriched_calls {
                    assert!(
                        !enriched_call.method_name.is_empty(),
                        "Method name should not be empty"
                    );
                    assert!(
                        !enriched_call.service.is_empty(),
                        "Service should not be empty"
                    );
                }
            }
            Err(e) => {
                panic!("Enrichment failed: {}", e);
            }
        }

        let total_duration = start_time.elapsed();
        println!("\nTest completed in {:?}", total_duration);
    }

    /// Create SdkMethodCall objects from the service index
    fn create_sdk_method_calls_from_service_index(
        service_index: &crate::extraction::sdk_model::ServiceModelIndex,
    ) -> Vec<SdkMethodCall> {
        use crate::extraction::sdk_model::ServiceDiscovery;

        let mut sdk_method_calls = Vec::new();

        // Iterate through all services and their operations
        for (service_name, service_definition) in &service_index.services {
            println!(
                "Processing service: {} ({} operations)",
                service_name,
                service_definition.operations.len()
            );

            for operation_name in service_definition.operations.keys() {
                // Convert operation name to snake_case using the existing function
                let method_name =
                    ServiceDiscovery::operation_to_method_name(operation_name, Language::Python);

                // Create SdkMethodCall with:
                // - Operation name in snake_case
                // - Single service name
                // - No metadata
                let sdk_method_call = SdkMethodCall {
                    name: method_name,
                    possible_services: vec![service_name.clone()],
                    metadata: None,
                };

                sdk_method_calls.push(sdk_method_call);
            }
        }

        // Sort for consistent ordering
        sdk_method_calls.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.possible_services.cmp(&b.possible_services))
        });

        sdk_method_calls
    }

    /// Analyze and report on enrichment results
    fn analyze_enrichment_results(enriched_calls: &[crate::enrichment::EnrichedSdkMethodCall]) {
        use std::collections::HashMap;

        let mut service_counts = HashMap::new();
        let mut total_actions = 0;
        let mut calls_with_actions = 0;

        for enriched_call in enriched_calls {
            // Count by service
            *service_counts
                .entry(enriched_call.service.clone())
                .or_insert(0) += 1;

            // Count actions
            total_actions += enriched_call.actions.len();
            if !enriched_call.actions.is_empty() {
                calls_with_actions += 1;
            }
        }

        println!("Enrichment Analysis:");
        println!("   • Total enriched calls: {}", enriched_calls.len());
        println!("   • Calls with actions: {}", calls_with_actions);
        println!("   • Total actions found: {}", total_actions);
        println!(
            "   • Average actions per call: {:.2}",
            if enriched_calls.is_empty() {
                0.0
            } else {
                total_actions as f64 / enriched_calls.len() as f64
            }
        );
    }
}
