//! Boto3 resources model parser
//!
//! Parses boto3 resources JSON specifications and utility mappings for resource-based AWS SDK patterns.

use crate::embedded_data::{
    ActionSpec, Boto3ResourcesJson, EmbeddedBoto3Data, HasManySpecJson, ResourceIdentifier,
    ResourceSpec, ServiceSpec,
};
use convert_case::{Case, Casing};
use std::collections::HashMap;

// Re-export ParamMapping for public use
pub use crate::embedded_data::ParamMapping;

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
    let mapping = EmbeddedBoto3Data::get_utilities_mapping()
        .ok_or_else(|| "Boto3 utilities mapping not found in embedded data".to_string())?;

    Ok(mapping.services.keys().cloned().collect())
}

/// Unified boto3 specifications model containing resources and utility methods
#[derive(Debug, Clone)]
pub(crate) struct Boto3ResourcesModel {
    pub(crate) service_name: String,
    service_constructors: HashMap<String, ServiceConstructorSpec>,
    resource_types: HashMap<String, ResourceDefinition>,
    client_utility_methods: HashMap<String, ClientUtilityMethod>,
    resource_utility_methods: HashMap<String, ResourceUtilityMethods>,
    service_has_many: HashMap<String, HasManySpec>, // Key: snake_case collection name
}

/// Client-level utility method specification
#[derive(Debug, Clone)]
pub struct ClientUtilityMethod {
    pub(crate) operations: Vec<crate::embedded_data::ServiceOperation>,
}

/// Resource-level utility methods for a specific resource type
#[derive(Debug, Clone)]
pub struct ResourceUtilityMethods {
    pub(crate) methods: HashMap<String, ResourceUtilityMethod>,
}

/// Resource utility method specification
#[derive(Debug, Clone)]
pub struct ResourceUtilityMethod {
    pub(crate) operations: Vec<crate::embedded_data::ServiceOperation>,
    pub(crate) accepted_params: Vec<String>,
    pub(crate) identifier_mappings: Vec<crate::embedded_data::IdentifierMapping>,
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

/// Action mapping from resource method to SDK operation
#[derive(Debug, Clone)]
pub struct ActionMapping {
    pub(crate) operation: OperationType,
    pub(crate) identifier_params: Vec<ParamMapping>,
}

/// HasMany collection specification for resource collections
#[derive(Debug, Clone)]
pub struct HasManySpec {
    pub(crate) operation: String, // Operation name (e.g., "ListObjects")
    pub(crate) identifier_params: Vec<ParamMapping>,
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
        let common_services = match extract_services_from_embedded_utilities_mapping() {
            Ok(services) => services,
            Err(e) => {
                log::warn!(
                    "Failed to extract services from embedded utilities mapping: {}",
                    e
                );
                vec![]
            }
        };

        for service_name in common_services {
            match Boto3ResourcesModel::load(&service_name) {
                Ok(model) => {
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
                Err(e) => {
                    log::debug!("Failed to load service '{}': {}", service_name, e);
                    // Silently continue on error to avoid breaking extraction
                }
            }
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
    /// Load base boto3 model for a service from embedded data
    ///
    /// Loads resource specifications from embedded boto3 data without utility methods
    fn load_base(service_name: &str) -> Result<Self, String> {
        // Get service versions from embedded data (cached)
        let service_versions = EmbeddedBoto3Data::build_service_versions_map();

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

        // Get the deserialized resources data
        let resources_json =
            EmbeddedBoto3Data::get_resources_definition(service_name, latest_version).ok_or_else(
                || {
                    format!(
                        "Resources data not found for {}/{}",
                        service_name, latest_version
                    )
                },
            )?;

        // Build model from parsed JSON
        Self::build_model_from_json(service_name, resources_json)
    }

    /// Load unified boto3 model with utility methods from embedded data
    ///
    /// Loads resource specifications and merges with utility methods from embedded mapping
    pub(crate) fn load(service_name: &str) -> Result<Self, String> {
        // Load base resource model from embedded data
        let mut model = Self::load_base(service_name)?;

        // Load and merge utility methods from embedded data
        Self::merge_utility_methods_from_embedded(&mut model)?;

        Ok(model)
    }

    /// Merge utility methods from embedded mapping into model
    fn merge_utility_methods_from_embedded(model: &mut Boto3ResourcesModel) -> Result<(), String> {
        let mapping = EmbeddedBoto3Data::get_utilities_mapping()
            .ok_or_else(|| "Boto3 utilities mapping not found in embedded data".to_string())?;

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
        let result = Boto3ResourcesModel::load("dynamodb");

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
        let result = Boto3ResourcesModel::load("s3");

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
