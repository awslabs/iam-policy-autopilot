//! SDK Model - Consolidated SDK service discovery, parsing, and type definitions.
//!
//! This module provides comprehensive functionality for AWS SDK services including:
//! - Service discovery and management
//! - SDK service definitions and type structures
//! - Method name conversion between languages
//!
//! All SDK-related functionality is consolidated here for better cohesion and maintainability.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinSet;

use convert_case::{Case, Casing};
use serde::{Deserialize, Serialize};

use crate::embedded_data::BotocoreData;
use crate::errors::{ExtractorError, Result};
use crate::Language;

// ================================================================================================
// SDK MODEL TYPES (from services/mod.rs)
// ================================================================================================

/// Information about an available AWS service
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SdkModel {
    /// Service name (e.g., "s3", "ec2", "lambda")
    pub(crate) name: String,
    /// API version (e.g., "2006-03-01", "2016-11-15")
    pub(crate) api_version: String,
}

impl SdkModel {
    /// Create a new `SdkModel` instance
    #[must_use]
    pub(crate) const fn new(name: String, api_version: String) -> Self {
        Self { name, api_version }
    }
}

// ================================================================================================
// SDK SERVICE DEFINITIONS (from models/mod.rs sdk module)
// ================================================================================================

/// Complete SDK service definition
///
/// Represents a complete AWS service definition including all operations,
/// data shapes, waiters, and metadata. This is typically loaded from AWS SDK
/// service definition files.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct SdkServiceDefinition {
    /// Service definition version (e.g., "2.0") - optional for compatibility
    pub(crate) version: Option<String>,
    /// Service metadata containing API version, service ID, etc.
    pub(crate) metadata: ServiceMetadata,
    /// Map of operation name to operation definition
    pub(crate) operations: HashMap<String, Operation>,
    /// Map of shape name to shape definition
    pub(crate) shapes: HashMap<String, Shape>,
}

/// Service metadata from AWS service definitions
///
/// Contains metadata about the AWS service including API version,
/// service identifier, protocol information, and other service-level details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ServiceMetadata {
    /// API version string (e.g., "2006-03-01")
    #[serde(rename = "apiVersion")]
    pub(crate) api_version: String,
    /// Service identifier (e.g., "S3", "EC2", "Lambda")
    #[serde(rename = "serviceId")]
    pub(crate) service_id: String,
}

/// SDK operation definition
///
/// Represents a single API operation with its HTTP configuration,
/// input/output shapes, error conditions, and documentation.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Operation {
    /// Operation name (e.g., "`CreateBucket`", "`ListObjects`")
    pub(crate) name: String,
    /// Input shape reference if the operation accepts input
    pub(crate) input: Option<ShapeReference>,
}

/// Data shape definition
///
/// Represents a data structure used in SDK operations including
/// its type, members, and documentation. Shapes define the structure
/// of request/response payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Shape {
    /// Shape type (structure, string, integer, list, map, etc.)
    #[serde(rename = "type")]
    pub(crate) type_name: String,
    /// Map of member name to shape reference for structure types
    /// TODO: Canonicalize member keys to lowercase during deserialization to handle
    /// inconsistent casing in AWS models. See: https://github.com/awslabs/iam-policy-autopilot/issues/57
    #[serde(default)]
    pub(crate) members: HashMap<String, ShapeReference>,
    /// Required parameters
    pub(crate) required: Option<Vec<String>>,
}

/// Reference to a shape with location information
///
/// Used to reference shapes in operation inputs/outputs and shape members.
/// Includes location metadata for protocol binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ShapeReference {
    /// Name of the referenced shape
    pub(crate) shape: String,
}

/// Service model index for comprehensive service lookup
///
/// Contains all loaded service models for a specific language and provides
/// efficient lookup capabilities for method validation.
#[derive(Debug, Clone)]
pub(crate) struct ServiceModelIndex {
    /// Map of service name to service definition
    pub(crate) services: HashMap<String, SdkServiceDefinition>,
    /// Map of method name to possible service method references
    pub(crate) method_lookup: HashMap<String, Vec<ServiceMethodRef>>,
    /// Reverse index: waiter name (PascalCase) to list of services that provide it
    /// Example: "InstanceTerminated" -> ["ec2"], "BucketExists" -> ["s3"]
    pub(crate) waiter_lookup: HashMap<String, Vec<ServiceMethodRef>>,
}

/// Reference to a service method for lookup purposes
///
/// Links a language-specific method name to its corresponding AWS service
/// operation, enabling validation of extracted method calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ServiceMethodRef {
    /// AWS service name (e.g., "s3", "ec2", "lambda")
    pub(crate) service_name: String,
    /// AWS operation name (e.g., "`GetObject`", "`RunInstances`")
    pub(crate) operation_name: String,
}

// ================================================================================================
// SERVICE DISCOVERY (from services/discovery.rs)
// ================================================================================================

/// Service discovery engine for AWS SDK services
///
/// Provides functionality to discover available services, load service definitions,
/// and build comprehensive service indexes for method validation.
pub(crate) struct ServiceDiscovery;

/// Global cache for service model indexes by language
/// Uses OnceLock for thread-safe lazy initialization
static SERVICE_INDEX_CACHE: OnceLock<RwLock<HashMap<String, Arc<ServiceModelIndex>>>> =
    OnceLock::new();

impl ServiceDiscovery {
    /// Discover all available services from embedded data
    ///
    /// Gets all available AWS services from the embedded service definitions.
    /// Each service includes its name and API version. Since build.rs only processes
    /// the latest version for each service, there's exactly one version per service.
    fn discover_services() -> Result<Vec<SdkModel>> {
        let start_time = std::time::Instant::now();
        log::debug!("Starting optimized service discovery...");

        // Use the optimized single-iteration approach
        let service_versions_map = BotocoreData::build_service_versions_map();

        let mut services = Vec::new();
        for (service_name, api_versions) in service_versions_map {
            // Since build.rs only processes the latest version, there should be exactly one version
            if let Some(api_version) = api_versions.first() {
                services.push(SdkModel::new(service_name, api_version.clone()));
            }
        }

        // Sort services by name for consistent ordering
        services.sort_by(|a, b| a.name.cmp(&b.name).then(a.api_version.cmp(&b.api_version)));

        let total_duration = start_time.elapsed();
        log::debug!(
            "Optimized service discovery completed in {:?} - found {} services",
            total_duration,
            services.len()
        );
        Ok(services)
    }

    /// Load all services into a comprehensive service model index
    ///
    /// Discovers all available services and builds a comprehensive index that includes
    /// method lookup tables for efficient validation of extracted method calls.
    ///
    /// This function uses a cache to avoid reloading the same service index multiple times
    /// for the same language. The cache is thread-safe and shared across all calls.
    pub(crate) async fn load_service_index(language: Language) -> Result<Arc<ServiceModelIndex>> {
        let language_key = language.to_string();

        // Initialize cache if needed
        let cache = SERVICE_INDEX_CACHE.get_or_init(|| RwLock::new(HashMap::new()));

        // Check if we already have this index cached
        {
            let read_guard = cache.read().await;
            if let Some(cached_index) = read_guard.get(&language_key) {
                log::debug!("Using cached service index for language '{}'", language);
                return Ok(Arc::clone(cached_index));
            }
        }

        // Not in cache, need to load it
        log::debug!(
            "Loading service index for language '{}' (not cached)",
            language
        );

        // Discover all available services
        let services = Self::discover_services()?;
        let mut service_models = HashMap::new();
        let mut method_lookup = HashMap::new();
        let mut waiter_lookup = HashMap::new();
        let mut load_errors = Vec::new();

        log::debug!(
            "Attempting to load {} services in parallel for language '{}'",
            services.len(),
            language
        );
        Self::load_services(
            services,
            &mut service_models,
            &mut method_lookup,
            &mut waiter_lookup,
            &mut load_errors,
            language,
        )
        .await?;

        log::debug!("Successfully loaded {} services", service_models.len());
        log::debug!("Failed to load {} services", load_errors.len());

        // If we couldn't load any services, return an error
        if service_models.is_empty() && !load_errors.is_empty() {
            return Err(ExtractorError::validation(format!(
                "Failed to load any services. Errors: {}",
                load_errors.join("; ")
            )));
        }

        // Return an error if we have failed services to make the issue visible
        if !load_errors.is_empty() {
            return Err(ExtractorError::validation(format!(
                "Failed to load {} services. First error: {}",
                load_errors.len(),
                load_errors[0]
            )));
        }

        let index = Arc::new(ServiceModelIndex {
            services: service_models,
            method_lookup,
            waiter_lookup,
        });

        // Cache the index for future use
        {
            let mut write_guard = cache.write().await;
            write_guard.insert(language_key, Arc::clone(&index));
        }

        Ok(index)
    }

    /// Load services in parallel using embedded data
    async fn load_services(
        services: Vec<SdkModel>,
        service_models: &mut HashMap<String, SdkServiceDefinition>,
        method_lookup: &mut HashMap<String, Vec<ServiceMethodRef>>,
        waiter_lookup: &mut HashMap<String, Vec<ServiceMethodRef>>,
        load_errors: &mut Vec<String>,
        language: Language,
    ) -> Result<()> {
        #[allow(clippy::type_complexity)]
        let mut join_set: JoinSet<
            Result<(
                SdkModel,
                SdkServiceDefinition,
                Option<HashMap<String, WaiterEntry>>,
            )>,
        > = JoinSet::new();

        // Import waiter types for inline loading
        use crate::extraction::waiter_model::WaiterEntry;
        use std::sync::Arc;

        // Create semaphore to limit concurrent operations (max 50)
        let semaphore = Arc::new(Semaphore::new(50));

        // Spawn parallel tasks for loading service models from embedded data
        let start_time = std::time::Instant::now();
        for service_info in services {
            let semaphore = semaphore.clone();
            join_set.spawn(async move {
                let service_start = std::time::Instant::now();

                // Acquire permit for concurrent operations
                let _permit = semaphore.acquire_owned().await.map_err(|e| {
                    ExtractorError::validation(format!("Failed to acquire semaphore permit: {}", e))
                })?;

                // Load service definition from embedded data
                let service_definition = BotocoreData::get_service_definition(
                    &service_info.name,
                    &service_info.api_version,
                )?;

                // Load waiters from embedded data
                let waiters =
                    BotocoreData::get_waiters(&service_info.name, &service_info.api_version);

                let service_time = service_start.elapsed();
                if service_time.as_millis() > 100 {
                    log::debug!(
                        "Service {}/{} took {:?} to load",
                        service_info.name,
                        service_info.api_version,
                        service_time
                    );
                }

                Ok((service_info, service_definition, waiters))
            });
        }

        log::debug!(
            "Spawned all {} service loading tasks in {:?}",
            join_set.len(),
            start_time.elapsed()
        );

        // Collect results from parallel tasks
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok((service_info, service_model, waiters))) => {
                    let service_name = service_info.name;
                    // Build method lookup index for this service
                    for operation_name in service_model.operations.keys() {
                        let method_name = Self::operation_to_method_name(operation_name, language);

                        method_lookup.entry(method_name.clone()).or_default().push(
                            ServiceMethodRef {
                                service_name: service_name.clone(),
                                operation_name: operation_name.clone(),
                            },
                        );
                    }

                    if let Some(waiters) = waiters {
                        for (waiter_name, waiter_entry) in waiters {
                            let method_name =
                                Self::operation_to_method_name(&waiter_name, language);

                            waiter_lookup.entry(method_name.clone()).or_default().push(
                                ServiceMethodRef {
                                    service_name: service_name.clone(),
                                    operation_name: waiter_entry.operation.clone(),
                                },
                            );
                        }
                    }

                    service_models.insert(service_name.clone(), service_model);
                }
                Ok(Err(e)) => {
                    // Collect errors but continue loading other services
                    let error_msg = format!("Failed to load service: {e}");
                    log::debug!("{error_msg}");
                    load_errors.push(error_msg);
                }
                Err(e) => {
                    // Task join error - this is more serious
                    let error_msg = format!("Task execution failed: {e}");
                    log::debug!("{error_msg}");
                    load_errors.push(error_msg);
                }
            }
        }

        Ok(())
    }

    /// Convert AWS operation name to language-specific method name
    ///
    /// Maps AWS API operation names (e.g., "`GetObject`") to the corresponding
    /// method names used in specific programming language SDKs.
    ///
    /// # Arguments
    ///
    /// * `operation_name` - AWS operation name (e.g., "`GetObject`", "`ListBuckets`")
    /// * `language` - Programming language identifier
    ///
    /// # Returns
    ///
    /// Language-specific method name (e.g., "`get_object`", "listBuckets")
    ///
    /// # Language Mappings
    ///
    /// - **Python (boto3)**: `PascalCase` → `snake_case` (`GetObject` → `get_object`)
    /// - **TypeScript/JavaScript**: `PascalCase` → camelCase (`GetObject` → getObject)
    /// - **Go**: `PascalCase` unchanged (`GetObject` → `GetObject`)
    #[must_use]
    pub(crate) fn operation_to_method_name(operation_name: &str, language: Language) -> String {
        #[allow(unreachable_patterns)]
        match language {
            Language::Python => {
                // Convert PascalCase to snake_case for Python (boto3)
                // Handle special AWS version suffixes like V2, V3, etc.
                Self::aws_python_case_conversion(operation_name)
            }
            Language::JavaScript | Language::TypeScript => {
                // Keep PascalCase for TypeScript/JavaScript to match operation-action maps
                operation_name.to_string()
            }
            Language::Go => {
                // Go uses PascalCase unchanged (GetObject -> GetObject)
                operation_name.to_string()
            }
            _ => {
                // Default: use operation name as-is
                operation_name.to_string()
            }
        }
    }

    /// Convert AWS operation names to Python method names with special handling for version suffixes
    ///
    /// This function uses convert_case for the base conversion but fixes AWS-specific patterns
    /// like "V2", "V3" suffixes that should not have underscores inserted.
    ///
    /// Examples:
    /// - "ListObjectsV2" → "list_objects_v2" (not "list_objects_v_2")
    /// - "GetObjectV1" → "get_object_v1" (not "get_object_v_1")
    /// - "CreateBucket" → "create_bucket" (normal cases unchanged)
    fn aws_python_case_conversion(operation_name: &str) -> String {
        // First, apply normal snake_case conversion
        let snake_case = operation_name.to_case(Case::Snake);

        // Fix AWS version suffixes at the end: "_v_N" → "_vN" where N is digits
        // Only replace if "_v_" is followed by digits and is at the end of string
        if snake_case.len() >= 4 && snake_case.ends_with(|c: char| c.is_ascii_digit()) {
            if let Some(v_pos) = snake_case.rfind("_v_") {
                let after_v = &snake_case[v_pos + 3..];
                // Check if everything after "_v_" is digits (ensuring it's a version suffix)
                if after_v.chars().all(|c| c.is_ascii_digit()) {
                    let prefix = &snake_case[..v_pos];
                    return format!("{prefix}_v{after_v}");
                }
            }
        }

        snake_case
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_info_creation() {
        let service_info = SdkModel::new("s3".to_string(), "2006-03-01".to_string());

        assert_eq!(service_info.name, "s3");
        assert_eq!(service_info.api_version, "2006-03-01");
    }

    #[test]
    fn test_pascal_to_snake_case() {
        let test_cases = vec![
            ("GetObject", "get_object"),
            ("ListBuckets", "list_buckets"),
            ("CreateBucket", "create_bucket"),
            ("DeleteObjectTagging", "delete_object_tagging"),
            ("PutBucketAcl", "put_bucket_acl"),
            ("S3", "s_3"), // Consecutive uppercase letters (convert_case handles this correctly)
            ("XMLParser", "xml_parser"), // Multiple consecutive uppercase (convert_case handles this better)
        ];

        for (input, expected) in test_cases {
            let result = input.to_case(Case::Snake);
            assert_eq!(result, expected, "Failed for input: {input}");
        }
    }

    #[test]
    fn test_operation_to_method_name() {
        // Test Python mapping
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::Python),
            "get_object"
        );

        // Test JavaScript mapping (PascalCase to match service index)
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::JavaScript),
            "GetObject"
        );

        // Test TypeScript mapping (PascalCase to match service index)
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::TypeScript),
            "GetObject"
        );

        // Test Go mapping
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::Go),
            "GetObject"
        );
    }

    #[test]
    fn test_sdk_service_definition() {
        let service = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2006-03-01".to_string(),
                service_id: "S3".to_string(),
            },
            operations: HashMap::new(),
            shapes: HashMap::new(),
        };

        assert_eq!(service.version, Some("2.0".to_string()));
    }

    #[tokio::test]
    async fn test_service_discovery_with_real_data() {
        match ServiceDiscovery::discover_services() {
            Ok(services) => {
                println!("✓ Discovered {} embedded services", services.len());

                // Look for some well-known services
                let service_names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();

                if service_names.contains(&"s3") {
                    println!("✓ Found S3 service");
                }
                if service_names.contains(&"ec2") {
                    println!("✓ Found EC2 service");
                }
                if service_names.contains(&"lambda") {
                    println!("✓ Found Lambda service");
                }

                // Print first few services for debugging
                for (i, service) in services.iter().take(5).enumerate() {
                    println!(
                        "Service {}: {} ({})",
                        i + 1,
                        service.name,
                        service.api_version
                    );
                }

                // We should have found at least some services
                assert!(
                    !services.is_empty(),
                    "Should discover at least some embedded services"
                );
            }
            Err(e) => {
                panic!("Service discovery failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_service_index_building() {
        match ServiceDiscovery::load_service_index(Language::Python).await {
            Ok(index) => {
                println!("✓ Successfully built service index");
                println!("Services loaded: {}", index.services.len());
                println!("Method lookup entries: {}", index.method_lookup.len());

                // Print some method lookup examples
                for (method_name, refs) in index.method_lookup.iter().take(5) {
                    println!(
                        "Method '{}' found in {} service(s)",
                        method_name,
                        refs.len()
                    );
                    for service_ref in refs {
                        println!(
                            "  - {}.{}",
                            service_ref.service_name, service_ref.operation_name,
                        );
                    }
                }
            }
            Err(e) => {
                panic!("Failed to build service index: {}", e);
            }
        }
    }

    #[test]
    fn test_method_name_conversion() {
        // Test Python (boto3) method name conversion
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::Python),
            "get_object"
        );
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("ListBuckets", Language::Python),
            "list_buckets"
        );
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("CreateBucket", Language::Python),
            "create_bucket"
        );
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("PutBucketAcl", Language::Python),
            "put_bucket_acl"
        );

        // Test JavaScript/TypeScript support (PascalCase to match service index)
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::JavaScript),
            "GetObject"
        );
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::TypeScript),
            "GetObject"
        );
        assert_eq!(
            ServiceDiscovery::operation_to_method_name("GetObject", Language::Go),
            "GetObject"
        );

        println!("✓ Method name conversion tests passed");
    }

    #[tokio::test]
    async fn test_service_index_caching() {
        let index1 = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index");

        let index2 = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index from cache");

        // Verify all indexes have the same content
        assert_eq!(index1.services.len(), index2.services.len());
        assert_eq!(index1.method_lookup.len(), index2.method_lookup.len());
    }
}
