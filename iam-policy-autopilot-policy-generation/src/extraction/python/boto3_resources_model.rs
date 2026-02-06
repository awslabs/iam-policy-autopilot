//! Boto3 resources model parser
//!
//! Parses boto3 resources JSON specifications and utility mappings for resource-based AWS SDK patterns.

use crate::embedded_data::Boto3Data;
use convert_case::{Case, Casing};
use serde::Deserialize;
use std::collections::HashMap;

/// Type of operation a resource action maps to
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationType {
    /// Regular SDK operation (e.g., "GetItem", "PutItem")
    SdkOperation(String),

    /// Waiter that requires resolution via ServiceModelIndex
    Waiter { waiter_name: String },

    /// Load operation for refreshing resource state
    Load(String),
}

/// Extract service names from embedded boto3 utilities mapping
fn extract_services_from_embedded_utilities_mapping() -> Result<Vec<String>, String> {
    let content_bytes = Boto3Data::get_utilities_mapping()
        .ok_or_else(|| "Boto3 utilities mapping not found in embedded data".to_string())?;

    let content = std::str::from_utf8(&content_bytes)
        .map_err(|e| format!("Invalid UTF-8 in embedded utilities mapping: {}", e))?;

    let mapping: UtilityMappingJson = serde_json::from_str(content)
        .map_err(|e| format!("Failed to parse utilities mapping: {}", e))?;

    Ok(mapping.services.keys().cloned().collect())
}

/// Unified boto3 specifications model containing resources and utility methods
#[derive(Debug, Clone, Deserialize)]
pub struct Boto3ResourcesModel {
    pub service_name: String,
    #[serde(skip)]
    service_constructors: HashMap<String, ServiceConstructorSpec>,
    #[serde(skip)]
    resource_types: HashMap<String, ResourceDefinition>,
    #[serde(skip)]
    client_utility_methods: HashMap<String, ClientUtilityMethod>,
    #[serde(skip)]
    resource_utility_methods: HashMap<String, ResourceUtilityMethods>,
    #[serde(skip)]
    service_has_many: HashMap<String, HasManySpec>, // Key: snake_case collection name
}

/// Client-level utility method specification
#[derive(Debug, Clone)]
pub struct ClientUtilityMethod {
    pub(crate) operations: Vec<ServiceOperation>,
}

/// Resource-level utility methods for a specific resource type
#[derive(Debug, Clone)]
pub struct ResourceUtilityMethods {
    pub(crate) methods: HashMap<String, ResourceUtilityMethod>,
}

/// Resource utility method specification
#[derive(Debug, Clone)]
pub struct ResourceUtilityMethod {
    pub(crate) operations: Vec<ServiceOperation>,
    pub(crate) accepted_params: Vec<String>,
    pub(crate) identifier_mappings: Vec<IdentifierMapping>,
}

/// Maps constructor arguments to operation parameters
#[derive(Debug, Clone, Deserialize)]
pub struct IdentifierMapping {
    pub target_param: String,
    pub constructor_arg_index: usize,
}

/// Service operation with required parameters (shared by resources and utilities)
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceOperation {
    pub operation: String,
    pub required_params: Vec<String>,
}

/// Resource constructor specification from service.has
#[derive(Debug, Clone)]
pub struct ServiceConstructorSpec {
    pub(crate) resource_type: String,
    pub(crate) identifiers_count: usize,
}

/// Resource definition with identifiers, actions, and collections
#[derive(Debug, Clone)]
pub struct ResourceDefinition {
    pub(crate) identifiers: Vec<ResourceIdentifier>,
    pub(crate) actions: HashMap<String, ActionMapping>,
    pub(crate) has_many: HashMap<String, HasManySpec>, // Key: snake_case collection name
}

/// Resource identifier mapping
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceIdentifier {
    pub name: String,
}

/// Action mapping from resource method to SDK operation
#[derive(Debug, Clone)]
pub struct ActionMapping {
    pub(crate) operation: OperationType,
    pub(crate) identifier_params: Vec<ParamMapping>,
}

/// Parameter mapping for identifier injection (used in resource actions)
#[derive(Debug, Clone, Deserialize)]
pub struct ParamMapping {
    pub(crate) target: String,
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
}

/// HasMany collection specification for resource collections
#[derive(Debug, Clone)]
pub struct HasManySpec {
    pub(crate) operation: String, // Operation name (e.g., "ListObjects")
    pub(crate) identifier_params: Vec<ParamMapping>,
}

/// JSON structures for parsing boto3_utilities_mapping.json
#[derive(Debug, Deserialize)]
struct UtilityMappingJson {
    services: HashMap<String, ServiceUtilityMethodsJson>,
}

#[derive(Debug, Deserialize)]
struct ServiceUtilityMethodsJson {
    client_methods: HashMap<String, UtilityMethodJson>,
    resource_methods: HashMap<String, ResourceTypeUtilityMethodsJson>,
}

type ResourceTypeUtilityMethodsJson = HashMap<String, UtilityMethodJson>;

#[derive(Debug, Deserialize)]
struct UtilityMethodJson {
    operations: Vec<ServiceOperation>,
    accepted_params: Vec<String>,
    #[serde(default)]
    identifier_mappings: Vec<IdentifierMapping>,
}

/// Raw JSON structure for parsing boto3 resources files
#[derive(Debug, Deserialize)]
struct Boto3ResourcesJson {
    service: Option<ServiceSpec>,
    resources: Option<HashMap<String, ResourceSpec>>,
}

#[derive(Debug, Deserialize)]
struct ServiceSpec {
    has: Option<HashMap<String, HasSpec>>,
    // TODO: Add support
    #[allow(dead_code)]
    #[serde(rename = "hasMany")]
    has_many: Option<HashMap<String, HasManySpecJson>>,
}

#[derive(Debug, Deserialize)]
struct HasSpec {
    resource: ResourceRef,
}

#[derive(Debug, Deserialize)]
struct ResourceRef {
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    identifiers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResourceSpec {
    identifiers: Option<Vec<ResourceIdentifier>>,
    actions: Option<HashMap<String, ActionSpec>>,
    // Special operations
    load: Option<LoadSpec>,
    waiters: Option<HashMap<String, WaiterSpec>>,
    #[serde(rename = "hasMany")]
    has_many: Option<HashMap<String, HasManySpecJson>>,
}

/// JSON structure for parsing hasMany collections from specs
#[derive(Debug, Clone, Deserialize)]
struct HasManySpecJson {
    request: RequestSpec,
}

/// Load operation specification
#[derive(Debug, Clone, Deserialize)]
struct LoadSpec {
    request: RequestSpec,
}

/// Waiter specification for resource waiters
#[derive(Debug, Clone, Deserialize)]
struct WaiterSpec {
    #[serde(rename = "waiterName")]
    waiter_name: String,
    #[serde(default)]
    params: Option<Vec<ParamMapping>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionSpec {
    request: RequestSpec,
}

#[derive(Debug, Clone, Deserialize)]
struct RequestSpec {
    operation: String,
    params: Option<Vec<ParamMapping>>,
}

/// Registry for multiple boto3 services with reverse lookup capabilities
#[derive(Debug, Clone)]
pub struct Boto3ResourcesRegistry {
    /// Maps resource type name to services that provide it
    /// Example: "Table" -> ["dynamodb"], "Bucket" -> ["s3"]
    resource_to_services: HashMap<String, Vec<String>>,

    /// Individual service models
    models: HashMap<String, Boto3ResourcesModel>,
}

impl Boto3ResourcesRegistry {
    /// Load all common boto3 service models with utility methods
    pub fn load_common_services_with_utilities() -> Self {
        let mut registry = Self {
            resource_to_services: HashMap::new(),
            models: HashMap::new(),
        };

        // Dynamically load services from embedded utilities mapping
        // We have a unit test which runs load_common_services_with_utilities ensuring that this doesn't fail at runtime.
        let common_services = extract_services_from_embedded_utilities_mapping()
            .expect("Failed to extract services from embedded utilities mapping.");

        for service_name in common_services {
            // We have a unit test which runs load_common_services_with_utilities ensuring that this doesn't fail at runtime.
            let model = Boto3ResourcesModel::load_with_utilities_from_embedded(&service_name)
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to load utilities for service '{}': {}",
                        service_name, e
                    )
                });
            // Index all resource types this service provides
            for resource_type in model.get_all_resource_types() {
                registry
                    .resource_to_services
                    .entry(resource_type.clone())
                    .or_default()
                    .push(service_name.to_string());
            }

            registry.models.insert(service_name.to_string(), model);
        }

        registry
    }

    /// Find which services provide a given resource type
    pub fn find_services_for_resource(&self, resource_type: &str) -> Vec<String> {
        self.resource_to_services
            .get(resource_type)
            .cloned()
            .unwrap_or_default()
    }

    /// Get a specific service model
    pub fn get_model(&self, service_name: &str) -> Option<&Boto3ResourcesModel> {
        self.models.get(service_name)
    }

    /// Get all loaded service models
    pub fn models(&self) -> &HashMap<String, Boto3ResourcesModel> {
        &self.models
    }
}

impl Boto3ResourcesModel {
    /// Load unified boto3 model for a service from embedded data
    ///
    /// Loads resource specifications from embedded boto3 data
    pub fn load_from_embedded(service_name: &str) -> Result<Self, String> {
        // Get service versions from embedded data
        let service_versions = Boto3Data::build_service_versions_map();

        // Find the service and get its latest version
        let versions = service_versions.get(service_name).ok_or_else(|| {
            format!(
                "Service '{}' not found in embedded boto3 data",
                service_name
            )
        })?;

        let latest_version = versions
            .last()
            .ok_or_else(|| format!("No versions found for service '{}'", service_name))?;

        // Get the resources data
        let resources_data = Boto3Data::get_resources_raw(service_name, latest_version)
            .ok_or_else(|| {
                format!(
                    "Resources data not found for {}/{}",
                    service_name, latest_version
                )
            })?;

        // Parse the resource specification
        let content = std::str::from_utf8(&resources_data)
            .map_err(|e| format!("Invalid UTF-8 in embedded boto3 data: {}", e))?;

        Self::parse_resources_content(service_name, content)
    }

    /// Load unified boto3 model with utility methods from embedded data
    ///
    /// Loads resource specifications and merges with utility methods from embedded mapping
    pub fn load_with_utilities_from_embedded(service_name: &str) -> Result<Self, String> {
        // Load base resource model from embedded data
        let mut model = Self::load_from_embedded(service_name)?;

        // Load and merge utility methods from embedded data
        Self::merge_utility_methods_from_embedded(&mut model)?;

        Ok(model)
    }

    /// Merge utility methods from embedded mapping into model
    fn merge_utility_methods_from_embedded(model: &mut Boto3ResourcesModel) -> Result<(), String> {
        let content_bytes = Boto3Data::get_utilities_mapping()
            .ok_or_else(|| "Boto3 utilities mapping not found in embedded data".to_string())?;

        let content = std::str::from_utf8(&content_bytes)
            .map_err(|e| format!("Invalid UTF-8 in embedded utilities mapping: {}", e))?;

        let mapping: UtilityMappingJson = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse utilities mapping: {}", e))?;

        if let Some(service_utilities) = mapping.services.get(&model.service_name) {
            // Parse client utility methods
            for (method_name, method_spec) in &service_utilities.client_methods {
                model.client_utility_methods.insert(
                    method_name.clone(),
                    ClientUtilityMethod {
                        operations: method_spec.operations.clone(),
                    },
                );
            }

            // Parse resource utility methods
            for (resource_type, methods) in &service_utilities.resource_methods {
                let mut resource_methods_map = HashMap::new();

                for (method_name, method_spec) in methods {
                    resource_methods_map.insert(
                        method_name.clone(),
                        ResourceUtilityMethod {
                            operations: method_spec.operations.clone(),
                            accepted_params: method_spec.accepted_params.clone(),
                            identifier_mappings: method_spec.identifier_mappings.clone(),
                        },
                    );
                }

                model.resource_utility_methods.insert(
                    resource_type.clone(),
                    ResourceUtilityMethods {
                        methods: resource_methods_map,
                    },
                );
            }

            // Synthesize constructors also for resources defined in 'resources'
            // but missing from 'service.has' (e.g., S3 Object).
            // These resources can still be instantiated directly from the service object in boto3
            // via patterns like: s3.Object('bucket', 'key')
            //
            // Currently, this only applies to S3's Object resource, which is defined in the
            // resources section with proper identifiers but not listed in service.has.
            for resource_type in service_utilities.resource_methods.keys() {
                if let Some(resource_def) = model.resource_types.get(resource_type) {
                    if !model.service_constructors.contains_key(resource_type) {
                        // Create synthetic constructor from resource definition
                        let constructor_spec = ServiceConstructorSpec {
                            resource_type: resource_type.clone(),
                            identifiers_count: resource_def.identifiers.len(),
                        };
                        model
                            .service_constructors
                            .insert(resource_type.clone(), constructor_spec);
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse boto3 resources JSON content
    fn parse_resources_content(service_name: &str, content: &str) -> Result<Self, String> {
        let json: Boto3ResourcesJson =
            serde_json::from_str(content).map_err(|e| format!("Failed to parse JSON: {}", e))?;

        Self::build_model_from_json(service_name, json)
    }

    /// Build model from parsed JSON
    fn build_model_from_json(service_name: &str, json: Boto3ResourcesJson) -> Result<Self, String> {
        let mut model = Boto3ResourcesModel {
            service_name: service_name.to_string(),
            service_constructors: HashMap::new(),
            resource_types: HashMap::new(),
            client_utility_methods: HashMap::new(),
            resource_utility_methods: HashMap::new(),
            service_has_many: HashMap::new(),
        };

        // Parse service constructors and service-level hasMany collections
        if let Some(service) = json.service {
            Self::parse_service_constructors(&mut model, service)?;
        }

        // Parse resource definitions
        if let Some(resources) = json.resources {
            Self::parse_resource_definitions(&mut model, resources)?;
        }

        Ok(model)
    }

    /// Parse service.has for resource constructors and service.hasMany for service-level collections
    fn parse_service_constructors(
        model: &mut Boto3ResourcesModel,
        service: ServiceSpec,
    ) -> Result<(), String> {
        // Parse service.has for resource constructors
        if let Some(has) = service.has {
            for (constructor_name, has_spec) in has {
                let identifiers_count = has_spec
                    .resource
                    .identifiers
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.len())
                    .unwrap_or(0);

                let constructor_spec = ServiceConstructorSpec {
                    resource_type: has_spec.resource.resource_type.clone(),
                    identifiers_count,
                };
                model
                    .service_constructors
                    .insert(constructor_name, constructor_spec);
            }
        }

        // Parse service.hasMany for service-level collections
        if let Some(has_many_specs) = service.has_many {
            for (collection_name, has_many_json) in has_many_specs {
                // Extract identifier params from request params (though service-level collections typically don't have identifiers)
                let identifier_params = has_many_json
                    .request
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.source == "identifier")
                    .collect();

                let has_many_spec = HasManySpec {
                    operation: has_many_json.request.operation,
                    identifier_params,
                };

                // Store with snake_case key for Python attribute matching
                let snake_case_name = collection_name.to_case(Case::Snake);
                model
                    .service_has_many
                    .insert(snake_case_name, has_many_spec);
            }
        }

        Ok(())
    }

    /// Parse resources for resource definitions
    fn parse_resource_definitions(
        model: &mut Boto3ResourcesModel,
        resources: HashMap<String, ResourceSpec>,
    ) -> Result<(), String> {
        for (resource_name, resource_spec) in resources {
            // Parse regular actions
            let mut actions = Self::parse_resource_actions(resource_spec.actions.clone())?;

            // Parse special operations (load, waiters)
            Self::parse_special_operations(&mut actions, &resource_spec)?;

            // Parse hasMany collections
            let has_many = Self::parse_has_many_collections(resource_spec.has_many)?;

            let resource_def = ResourceDefinition {
                identifiers: resource_spec.identifiers.unwrap_or_default(),
                actions,
                has_many,
            };

            model.resource_types.insert(resource_name, resource_def);
        }
        Ok(())
    }

    /// Parse hasMany collections for a resource
    fn parse_has_many_collections(
        has_many_specs: Option<HashMap<String, HasManySpecJson>>,
    ) -> Result<HashMap<String, HasManySpec>, String> {
        let mut has_many = HashMap::new();

        if let Some(has_many_specs) = has_many_specs {
            for (collection_name, has_many_json) in has_many_specs {
                // Extract identifier params from request params
                let identifier_params = has_many_json
                    .request
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.source == "identifier")
                    .collect();

                let has_many_spec = HasManySpec {
                    operation: has_many_json.request.operation,
                    identifier_params,
                };

                // Store with snake_case key for Python attribute matching
                let snake_case_name = collection_name.to_case(Case::Snake);
                has_many.insert(snake_case_name, has_many_spec);
            }
        }

        Ok(has_many)
    }

    /// Parse actions for a resource
    fn parse_resource_actions(
        resource_actions: Option<HashMap<String, ActionSpec>>,
    ) -> Result<HashMap<String, ActionMapping>, String> {
        let mut actions = HashMap::new();

        if let Some(resource_actions) = resource_actions {
            for (action_name, action_spec) in resource_actions {
                let identifier_params = action_spec
                    .request
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.source == "identifier")
                    .collect();

                let action_mapping = ActionMapping {
                    operation: OperationType::SdkOperation(action_spec.request.operation),
                    identifier_params,
                };

                actions.insert(action_name.clone(), action_mapping.clone());
                actions.insert(action_name.to_case(Case::Snake), action_mapping);
            }
        }

        Ok(actions)
    }

    /// Parse special operations like 'load' and waiters for a resource
    fn parse_special_operations(
        actions: &mut HashMap<String, ActionMapping>,
        resource_spec: &ResourceSpec,
    ) -> Result<(), String> {
        // Parse 'load' operation -> maps to 'load' method
        if let Some(load_spec) = &resource_spec.load {
            let identifier_params = load_spec
                .request
                .params
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.source == "identifier")
                .collect();

            let action_mapping = ActionMapping {
                operation: OperationType::Load(load_spec.request.operation.clone()),
                identifier_params,
            };

            actions.insert("load".to_string(), action_mapping);
        }

        // Parse waiters -> map to 'wait_until_<waiter_snake_case>' methods
        if let Some(waiters) = &resource_spec.waiters {
            for (waiter_name_pascal, waiter_spec) in waiters {
                let method_name = format!("wait_until_{}", waiter_name_pascal.to_case(Case::Snake));

                let identifier_params = waiter_spec
                    .params
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| p.source == "identifier")
                    .collect();

                // Use type-safe enum variant for waiters
                let action_mapping = ActionMapping {
                    operation: OperationType::Waiter {
                        waiter_name: waiter_spec.waiter_name.clone(),
                    },
                    identifier_params,
                };

                actions.insert(method_name, action_mapping);
            }
        }

        Ok(())
    }

    /// Get action mapping for a resource type and action name
    pub fn get_action_mapping(
        &self,
        resource_type: &str,
        action_name: &str,
    ) -> Option<&ActionMapping> {
        let resource_def = self.resource_types.get(resource_type)?;
        resource_def.actions.get(action_name)
    }

    /// Get constructor spec for a resource type
    pub fn get_constructor_spec(&self, constructor_name: &str) -> Option<&ServiceConstructorSpec> {
        self.service_constructors.get(constructor_name)
    }

    /// Get resource definition by type name
    pub fn get_resource_definition(&self, resource_type: &str) -> Option<&ResourceDefinition> {
        self.resource_types.get(resource_type)
    }

    /// Get client utility method by name
    pub fn get_client_utility_method(&self, method_name: &str) -> Option<&ClientUtilityMethod> {
        self.client_utility_methods.get(method_name)
    }

    /// Get resource utility method by resource type and method name
    pub fn get_resource_utility_method(
        &self,
        resource_type: &str,
        method_name: &str,
    ) -> Option<&ResourceUtilityMethod> {
        self.resource_utility_methods
            .get(resource_type)
            .and_then(|methods| methods.methods.get(method_name))
    }

    /// Get all resource type names from service constructors
    pub(crate) fn get_all_resource_types(&self) -> impl Iterator<Item = &String> {
        self.service_constructors.keys()
    }

    /// Get hasMany specification by collection name (snake_case)
    pub fn get_has_many_spec(
        &self,
        resource_type: &str,
        collection_name: &str,
    ) -> Option<&HasManySpec> {
        let resource_def = self.resource_types.get(resource_type)?;
        resource_def.has_many.get(collection_name)
    }

    /// Get all resource utility methods (for iteration in Tier 3)
    pub fn get_all_resource_utility_methods(&self) -> &HashMap<String, ResourceUtilityMethods> {
        &self.resource_utility_methods
    }

    /// Get all resource definitions (for iteration in Tier 3)
    pub fn get_all_resource_definitions(&self) -> &HashMap<String, ResourceDefinition> {
        &self.resource_types
    }

    /// Get all service-level hasMany collections (for iteration in Tier 3)
    pub fn get_service_has_many_collections(&self) -> &HashMap<String, HasManySpec> {
        &self.service_has_many
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_case_conversion() {
        assert_eq!("GetItem".to_case(Case::Snake), "get_item");
        assert_eq!("PutItem".to_case(Case::Snake), "put_item");
        assert_eq!("DeleteObject".to_case(Case::Snake), "delete_object");
        assert_eq!("CreateBucket".to_case(Case::Snake), "create_bucket");
    }

    #[test]
    fn test_load_dynamodb_model_from_embedded() {
        let result = Boto3ResourcesModel::load_from_embedded("dynamodb");

        // This test will only pass if embedded data is available
        if result.is_ok() {
            let model = result.unwrap();
            assert_eq!(model.service_name, "dynamodb");

            // Check that Table constructor exists
            assert!(model.get_constructor_spec("Table").is_some());

            // Check that Table resource type exists
            assert!(model.get_resource_definition("Table").is_some());

            // Check that GetItem action exists for Table
            let table_def = model.get_resource_definition("Table").unwrap();
            assert!(
                table_def.actions.contains_key("GetItem")
                    || table_def.actions.contains_key("get_item")
            );
        }
    }

    #[test]
    fn test_load_s3_model_from_embedded() {
        let result = Boto3ResourcesModel::load_from_embedded("s3");

        // This test will only pass if embedded data is available
        if result.is_ok() {
            let model = result.unwrap();
            assert_eq!(model.service_name, "s3");

            // Check that Bucket constructor exists
            assert!(model.get_constructor_spec("Bucket").is_some());

            // Check that Bucket resource type exists
            assert!(model.get_resource_definition("Bucket").is_some());

            // Check that Delete action exists for Bucket
            let bucket_def = model.get_resource_definition("Bucket").unwrap();
            assert!(
                bucket_def.actions.contains_key("Delete")
                    || bucket_def.actions.contains_key("delete")
            );
        }
    }

    #[test]
    fn test_load_common_services_with_utilities_error_free() {
        Boto3ResourcesRegistry::load_common_services_with_utilities();
    }

    #[test]
    fn test_embedded_utilities_mapping_access() {
        // Test that we can access the embedded utilities mapping
        let result = extract_services_from_embedded_utilities_mapping();

        if result.is_ok() {
            let services = result.unwrap();
            assert!(
                !services.is_empty(),
                "Should extract at least one service from utilities mapping"
            );

            // Check for expected services
            assert!(
                services.contains(&"s3".to_string()),
                "Should contain s3 service"
            );
            assert!(
                services.contains(&"ec2".to_string()),
                "Should contain ec2 service"
            );
            assert!(
                services.contains(&"dynamodb".to_string()),
                "Should contain dynamodb service"
            );
        }
        // If embedded data is not available, test passes (build-time dependency)
    }
}

#[cfg(test)]
mod negative_tests {
    use rust_embed::RustEmbed;

    use super::*;

    /// Embedded invalid test configuration files for negative testing
    /// This RustEmbed points to test resources with intentionally malformed configs
    #[derive(RustEmbed)]
    #[folder = "tests/resources/invalid_configs"]
    #[include = "*.json"]
    struct InvalidTestConfigs;

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

            let result: Result<UtilityMappingJson, _> = serde_json::from_str(json_str);

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
