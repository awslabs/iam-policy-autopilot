//! Resource Matcher for combining OperationAction maps with Service Reference data
//!
//! This module provides the ResourceMatcher that coordinates operation
//! action maps with Service Definition Files to generate enriched method calls
//! with complete IAM metadata.

use convert_case::{Case, Casing};
use std::collections::HashMap;
use std::sync::Arc;

use super::{Action, EnrichedSdkMethodCall, Resource};
use crate::enrichment::operation_fas_map::{FasOperation, OperationFasMap, OperationFasMaps};
use crate::enrichment::service_reference::ServiceReference;
use crate::enrichment::{Condition, ServiceReferenceLoader};
use crate::errors::{ExtractorError, Result};
use crate::service_configuration::ServiceConfiguration;
use crate::SdkMethodCall;

/// ResourceMatcher coordinates OperationAction maps and Service Reference data to generate enriched method calls
///
/// This struct provides the core functionality for the 3-stage enrichment pipeline,
/// combining parsed method calls with operation action maps and Service
/// Definition Files to produce complete IAM metadata.
#[derive(Debug, Clone)]
pub(crate) struct ResourceMatcher {
    service_cfg: Arc<ServiceConfiguration>,
    fas_maps: OperationFasMaps,
}

impl ResourceMatcher {
    /// Create a new ResourceMatcher instance
    #[must_use]
    pub(crate) fn new(service_cfg: Arc<ServiceConfiguration>, fas_maps: OperationFasMaps) -> Self {
        Self {
            service_cfg,
            fas_maps,
        }
    }

    /// Enrich a parsed method call with OperationAction maps and Service Reference data
    ///
    /// This is the main entry point for the enrichment process. For each possible
    /// service in the ParsedMethodCall, it creates one EnrichedMethodCall with
    /// complete IAM metadata.
    pub(crate) async fn enrich_method_call<'b>(
        &self,
        parsed_call: &'b SdkMethodCall,
        service_reference_loader: &ServiceReferenceLoader,
    ) -> Result<Vec<EnrichedSdkMethodCall<'b>>> {
        if parsed_call.possible_services.is_empty() {
            return Err(ExtractorError::enrichment_error(
                &parsed_call.name,
                "No matching services found for method call",
            ));
        }

        let mut enriched_calls: Vec<EnrichedSdkMethodCall<'_>> = Vec::new();

        // For each possible service in the parsed method call
        for service_name in &parsed_call.possible_services {
            // Find the corresponding OperationFas map (may be None for services without operation action maps)
            let operation_fas_map_option = self.find_operation_fas_map_for_service(service_name);

            // Create enriched method call for this service
            if let Some(enriched_call) = self
                .create_enriched_method_call(
                    parsed_call,
                    service_name,
                    operation_fas_map_option,
                    service_reference_loader,
                )
                .await?
            {
                enriched_calls.push(enriched_call);
            }
        }

        Ok(enriched_calls)
    }

    /// Find OperationFas map for a specific service
    fn find_operation_fas_map_for_service(
        &self,
        service_name: &str,
    ) -> Option<Arc<OperationFasMap>> {
        self.fas_maps
            .get(
                self.service_cfg
                    .rename_service_operation_action_map(service_name)
                    .as_ref(),
            )
            .cloned()
    }

    fn make_condition(context: &HashMap<String, String>) -> Vec<Condition> {
        let mut result = vec![];
        for (key, value) in context {
            result.push(Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: key.clone(),
                values: vec![value.clone()],
            })
        }
        result
    }

    /// Create an enriched method call for a specific service
    async fn create_enriched_method_call<'a>(
        &self,
        parsed_call: &'a SdkMethodCall,
        service_name: &str,
        operation_fas_map_option: Option<Arc<OperationFasMap>>,
        service_reference_loader: &ServiceReferenceLoader,
    ) -> Result<Option<EnrichedSdkMethodCall<'a>>> {
        log::debug!(
            "Creating method call for service: {}, and method name: {}",
            service_name,
            parsed_call.name
        );
        log::debug!("operation_fas_map_option: {:?}", operation_fas_map_option);

        let initial = FasOperation::new(
            parsed_call.name.to_case(Case::Pascal),
            service_name.to_string(),
            HashMap::new(),
        );
        let initial_service_operation_name = initial.service_operation_name(&self.service_cfg);

        let mut operations = vec![initial];

        match operation_fas_map_option {
            Some(operation_fas_map) => {
                log::debug!("Looking up {}", initial_service_operation_name);
                let result = operation_fas_map
                    .fas_operations
                    .get(&initial_service_operation_name);
                result.iter().for_each(|additional_operations| {
                    operations.extend_from_slice(additional_operations);
                });
                if result.is_none() {
                    log::debug!("Did not find {}", initial_service_operation_name);
                }
            }
            None => {
                log::debug!("None");
            }
        };

        log::debug!("operations: {:?}", operations);

        let mut enriched_actions = vec![];
        for operation in operations {
            let service = operation.service(&self.service_cfg);
            // Find the corresponding SDF using the cache
            let service_reference = service_reference_loader
                .load(&operation.service(&self.service_cfg))
                .await?;

            match service_reference {
                None => {
                    continue;
                }
                Some(service_reference) => {
                    log::debug!("Creating actions for {:?}", operation);
                    log::debug!("  with context {:?}", operation.context);
                    if let Some(operation_to_authorized_actions) = service_reference_loader
                        .get_operation_to_authorized_actions(&service)
                        .await?
                    {
                        log::debug!(
                            "Looking up {}",
                            &operation.service_operation_name(&self.service_cfg)
                        );
                        if let Some(operation_to_authorized_action) =
                            operation_to_authorized_actions
                                .get(&operation.service_operation_name(&self.service_cfg))
                        {
                            for action in &operation_to_authorized_action.authorized_actions {
                                let enriched_resources = self
                                    .find_resources_for_action_in_service_reference(
                                        &action.name,
                                        &service_reference,
                                    )?;
                                let conditions = Self::make_condition(&operation.context);

                                let enriched_action = Action::new(
                                    action.name.clone(),
                                    enriched_resources,
                                    conditions,
                                );

                                enriched_actions.push(enriched_action);
                            }
                        } else {
                            // Fallback: operation not found in operation action map, create basic action
                            // This ensures we don't filter out operations, only ADD additional ones from the map
                            if let Some(a) =
                                self.create_fallback_action(&parsed_call.name, &service_reference)?
                            {
                                enriched_actions.push(a)
                            }
                        }
                    } else {
                        // Fallback: operation action map does not exist, create basic action
                        if let Some(a) =
                            self.create_fallback_action(&parsed_call.name, &service_reference)?
                        {
                            enriched_actions.push(a)
                        }
                    }
                }
            }
        }

        if enriched_actions.is_empty() {
            return Ok(None);
        }

        Ok(Some(EnrichedSdkMethodCall {
            method_name: parsed_call.name.clone(),
            service: service_name.to_string(),
            actions: enriched_actions,
            sdk_method_call: parsed_call,
        }))
    }

    /// Create fallback action for services without OperationAction operation action maps
    ///
    /// This method generates an action from the method name and looks up
    /// corresponding resources in the SDF.
    fn create_fallback_action(
        &self,
        method_name: &str,
        service_reference: &ServiceReference,
    ) -> Result<Option<Action>> {
        let renamed_service = self
            .service_cfg
            .rename_service_service_reference(&service_reference.service_name);
        let renamed_action = &method_name.to_case(Case::Pascal);
        let action_name = format!("{}:{}", renamed_service, renamed_action);

        // Sanity check that the action exists in the SDF
        if !service_reference
            .actions
            .contains_key(renamed_action.as_str())
        {
            return Ok(None);
        }

        // Look up the action in the Service Reference to find associated resources
        let resources =
            self.find_resources_for_action_in_service_reference(&action_name, service_reference)?;

        Ok(Some(Action::new(
            action_name.to_string(),
            resources,
            vec![],
        )))
    }

    /// Find resources for an action by looking it up in the SDF
    fn find_resources_for_action_in_service_reference(
        &self,
        action_name: &str,
        service_reference: &ServiceReference,
    ) -> Result<Vec<Resource>> {
        // Extract the action part (remove service prefix)
        let action = action_name.split(':').nth(1).unwrap_or(action_name);

        log::debug!(
            "find_resources_for_action_in_service_reference: action = {}",
            action
        );
        log::debug!(
            "find_resources_for_action_in_service_reference: service_reference.actions = {:?}",
            service_reference.actions
        );
        let mut result = vec![];
        if let Some(action) = service_reference.actions.get(action) {
            let overrides = self.service_cfg.resource_overrides.get(action_name);
            for resource in &action.resources {
                let service_reference_resource =
                    if let Some(r#override) = overrides.and_then(|m| m.get(resource)) {
                        log::debug!(
                        "find_resources_for_action_in_service_reference: resource override = {}",
                        r#override
                    );
                        Resource::new(resource.clone(), Some(vec![r#override.clone()]))
                    } else {
                        log::debug!(
                        "find_resources_for_action_in_service_reference: looking up resource = {}",
                        resource
                    );
                        log::debug!(
                            "find_resources_for_action_in_service_reference: resources = {:?}",
                            service_reference.resources
                        );
                        let arn_patterns = service_reference.resources.get(resource).cloned();
                        log::debug!(
                            "find_resources_for_action_in_service_reference: arn_pattern = {:?}",
                            arn_patterns
                        );
                        Resource::new(resource.clone(), arn_patterns)
                    };
                result.push(service_reference_resource);
            }
        };

        // If no resources found, that's still valid (some actions don't require specific resources)
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrichment::mock_remote_service_reference;

    fn create_test_parsed_method_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_enrich_method_call() {
        use std::collections::HashMap;
        use tempfile::TempDir;

        fn create_test_service_configuration() -> ServiceConfiguration {
            let json_content = r#"{
                "NoOperationActionMap": [],
                "HasFasMap": [],
                "NoServiceReference": [],
                "RenameServicesOperationActionMap": {},
                "RenameServicesServiceReference": {},
                "RenameOperations": {},
                "ResourceOverrides": {}
            }"#;

            serde_json::from_str(json_content)
                .expect("Failed to deserialize test ServiceConfiguration JSON")
        }

        let service_cfg = create_test_service_configuration();

        let (_, service_reference_loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());
        let parsed_call = create_test_parsed_method_call();

        // Create operation action map file
        let temp_dir = TempDir::new().unwrap();
        let action_map_dir = temp_dir.path().join("action_maps");
        tokio::fs::create_dir_all(&action_map_dir).await.unwrap();
        let s3_action_file = action_map_dir.join("s3.json");
        let s3_action_json = r#"{
            "operations": [
                {
                    "operation": "s3:GetObject",
                    "actions": [
                        {
                            "name": "s3:GetObject"
                        }
                    ]
                }
            ]
        }"#;
        tokio::fs::write(&s3_action_file, s3_action_json)
            .await
            .unwrap();

        let result = matcher
            .enrich_method_call(&parsed_call, &service_reference_loader)
            .await;

        assert!(result.is_ok());

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1);
        assert_eq!(enriched_calls[0].method_name, "get_object");
        assert_eq!(enriched_calls[0].service, "s3");
    }

    #[tokio::test]
    async fn test_fallback_for_service_without_operation_action_map() {
        use std::collections::HashMap;

        let parsed_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["mediastore-data".to_string()],
            metadata: None,
        };

        // Create service configuration with mediastore-data in no_operation_action_map
        let service_cfg = ServiceConfiguration {
            rename_services_operation_action_map: [(
                "mediastore-data".to_string(),
                "mediastore".to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
            rename_services_service_reference: [(
                "mediastore-data".to_string(),
                "mediastore".to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
            rename_operations: [(
                "s3:ListObjectsV2".to_string(),
                crate::service_configuration::OperationRename {
                    service: "s3".to_string(),
                    operation: "ListObjects".to_string(),
                },
            )]
            .iter()
            .cloned()
            .collect(),
            resource_overrides: HashMap::new(),
        };

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());

        let (mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        mock_remote_service_reference::mock_server_service_reference_response(&mock_server, "mediastore", serde_json::json!(
             {
                                 "Name": "mediastore",
                                 "Actions": [
                                     {
                                         "Name": "GetObject",
                                         "Resources": [
                                             {
                                             "Name": "container"
                                             },
                                             {
                                             "Name": "object"
                                             }
                                         ]
                                     }
                                 ],
                                 "Resources": [
                                     {
                                         "Name": "container",
                                         "ARNFormats": [
                                             "arn:${Partition}:mediastore:${Region}:${Account}:container/${ContainerName}"
                                         ]
                                         },
                                     {
                                     "Name": "object",
                                     "ARNFormats": [
                                         "arn:${Partition}:mediastore:${Region}:${Account}:container/${ContainerName}/${ObjectPath}"
                                     ]
                                     }
                                 ]
                             }
         )).await;

        let result = matcher.enrich_method_call(&parsed_call, &loader).await;
        if let Err(ref e) = result {
            println!("Error: {:?}", e);
        }
        assert!(
            result.is_ok(),
            "Fallback enrichment should succeed: {:?}",
            result
        );

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1);
        assert_eq!(enriched_calls[0].method_name, "get_object");
        assert_eq!(enriched_calls[0].service, "mediastore-data");
        assert_eq!(enriched_calls[0].actions.len(), 1);

        let action = &enriched_calls[0].actions[0];
        assert_eq!(action.name, "mediastore:GetObject");
        assert_eq!(action.resources.len(), 2);
    }

    #[tokio::test]
    async fn test_error_for_missing_operation_action_map_when_required() {
        use std::collections::HashMap;

        // Service configuration without s3 in no_operation_action_map
        let service_cfg = ServiceConfiguration {
            rename_services_operation_action_map: HashMap::new(),
            rename_services_service_reference: HashMap::new(),
            rename_operations: HashMap::new(),
            resource_overrides: HashMap::new(),
        };

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());
        let parsed_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        let (_, loader) = mock_remote_service_reference::setup_mock_server_with_loader_without_operation_to_action_mapping().await;

        let result = matcher.enrich_method_call(&parsed_call, &loader).await;
        assert!(
            result.is_ok(),
            "Should succeed with fallback action when operation action map is missing"
        );

        let enriched_calls = result.unwrap();
        assert_eq!(
            enriched_calls.len(),
            1,
            "Should have one enriched call using fallback"
        );
        assert_eq!(enriched_calls[0].method_name, "get_object");
        assert_eq!(enriched_calls[0].service, "s3");

        // This below assertion fails intermittently, so adding this println here
        assert_eq!(
            enriched_calls[0].actions.len(),
            1,
            "Should have one fallback action, enriched_calls[0].action is: {:?}",
            enriched_calls[0].actions
        );

        let action = &enriched_calls[0].actions[0];
        assert_eq!(
            action.name, "s3:GetObject",
            "Should use fallback action name"
        );
    }

    #[tokio::test]
    async fn test_enrich_method_call_returns_empty_vec_for_missing_operation() {
        use std::collections::HashMap;

        // Create service configuration with connectparticipant -> execute-api mapping
        let service_cfg = ServiceConfiguration {
            rename_services_operation_action_map: [(
                "connectparticipant".to_string(),
                "execute-api".to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
            rename_services_service_reference: [(
                "connectparticipant".to_string(),
                "execute-api".to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
            rename_operations: HashMap::new(),
            resource_overrides: HashMap::new(),
        };

        // NOTE: execute-api:SendMessage is intentionally NOT included;

        let (mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        mock_remote_service_reference::mock_server_service_reference_response(&mock_server, "execute-api", serde_json::json!({
                    "Name": "execute-api",
                    "Resources": [
                        {
                            "Name": "execute-api-general",
                            "ARNFormats": ["arn:${Partition}:execute-api:${Region}:${Account}:${ApiId}/${Stage}/${Method}/${ApiSpecificResourcePath}"]
                        }
                    ],
                    "Actions": [
                        {
                            "Name": "Invoke",
                            "Resources": [
                                {
                                    "Name": "execute-api-general"
                                }
                            ]
                        },
                        {
                            "Name": "InvalidateCache",
                            "Resources": [
                                {
                                    "Name": "execute-api-general"
                                }
                            ]
                        },
                        {
                            "Name": "ManageConnections",
                            "Resources": [
                                {
                                    "Name": "execute-api-general"
                                }
                            ]
                        }
                    ],
                    "Operations" : [ {
                        "Name" : "DeleteConnection",
                        "SDK" : [ {
                        "Name" : "apigatewaymanagementapi",
                        "Method" : "delete_connection",
                        "Package" : "Boto3"
                        } ]
                    }, {
                        "Name" : "GetConnection",
                        "SDK" : [ {
                        "Name" : "apigatewaymanagementapi",
                        "Method" : "get_connection",
                        "Package" : "Boto3"
                        } ]
                    }, {
                        "Name" : "PostToConnection",
                        "SDK" : [ {
                        "Name" : "apigatewaymanagementapi",
                        "Method" : "post_to_connection",
                        "Package" : "Boto3"
                        } ]
                    } ]
                })).await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());

        // Create SdkMethodCall for connectparticipant:send_message
        let parsed_call = SdkMethodCall {
            name: "send_message".to_string(),
            possible_services: vec!["connectparticipant".to_string()],
            metadata: None,
        };

        let result = matcher.enrich_method_call(&parsed_call, &loader).await;

        // Assertions
        assert!(
            result.is_ok(),
            "enrich_method_call should succeed even when no operations match"
        );

        let enriched_calls = result.unwrap();
        assert_eq!(
            enriched_calls.len(),
            0,
            "Explicit check: enriched calls length should be 0"
        );

        println!(
            "✓ Test passed: enrich_method_call correctly returns empty Vec for missing operation"
        );
    }

    #[tokio::test]
    async fn test_resource_overrides_for_iam_get_user() {
        use std::collections::HashMap;

        // Create service configuration with resource overrides for iam:GetUser
        let mut resource_overrides = HashMap::new();
        let mut iam_overrides = HashMap::new();
        iam_overrides.insert("user".to_string(), "*".to_string());
        resource_overrides.insert("iam:GetUser".to_string(), iam_overrides);

        let service_cfg = ServiceConfiguration {
            rename_services_operation_action_map: HashMap::new(),
            rename_services_service_reference: HashMap::new(),
            rename_operations: HashMap::new(),
            resource_overrides,
        };

        let (mock_server, service_reference_loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        mock_remote_service_reference::mock_server_service_reference_response(
            &mock_server,
            "iam",
            serde_json::json!({
                "Name": "iam",
                "Resources": [
                    {
                        "Name": "user",
                        "ARNFormats": ["arn:${Partition}:iam::${Account}:user/${UserNameWithPath}"]
                    }
                ],
                "Actions": [
                    {
                        "Name": "GetUser",
                        "Resources": [
                            {
                                "Name": "user"
                            }
                        ]
                    }
                ],
                "Operations": [
                    {
                        "Name" : "GetUser",
                        "AuthorizedActions" : [ {
                            "Name" : "GetUser",
                            "Service" : "iam"
                            } ],
                        "SDK" : [ {
                            "Name" : "iam",
                            "Method" : "get_user",
                            "Package" : "Boto3"
                        } ]
                    }
                ]
            }),
        )
        .await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());

        // Create parsed method call for get_user
        let parsed_call = SdkMethodCall {
            name: "get_user".to_string(),
            possible_services: vec!["iam".to_string()],
            metadata: None,
        };

        // Test the enrichment
        let result = matcher
            .enrich_method_call(&parsed_call, &service_reference_loader)
            .await;
        assert!(
            result.is_ok(),
            "Enrichment should succeed for iam:GetUser with resource override"
        );

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1, "Should have one enriched call");

        let enriched_call = &enriched_calls[0];
        assert_eq!(enriched_call.method_name, "get_user");
        assert_eq!(enriched_call.service, "iam");
        assert_eq!(enriched_call.actions.len(), 1, "Should have one action");

        let action = &enriched_call.actions[0];
        assert_eq!(action.name, "iam:GetUser");
        assert_eq!(action.resources.len(), 1, "Should have one resource");

        let resource = &action.resources[0];
        assert_eq!(resource.name, "user");

        // This is the key test: verify that the resource override "*" is used
        assert!(
            resource.arn_patterns.is_some(),
            "Resource should have ARN patterns"
        );
        let arn_patterns = resource.arn_patterns.as_ref().unwrap();
        assert_eq!(
            arn_patterns.len(),
            1,
            "Should have exactly one ARN pattern from override"
        );
        assert_eq!(
            arn_patterns[0], "*",
            "Resource override should be '*' for iam:GetUser user resource"
        );

        println!(
            "✓ Test passed: iam:GetUser correctly uses resource override '*' for user resource"
        );
    }

    #[tokio::test]
    async fn test_resource_overrides_mixed_with_normal_resources() {
        use std::collections::HashMap;

        // Create service configuration with resource overrides for only one resource
        let mut resource_overrides = HashMap::new();
        let mut s3_overrides = HashMap::new();
        s3_overrides.insert("bucket".to_string(), "arn:aws:s3:::*".to_string()); // Override bucket but not object
        resource_overrides.insert("s3:GetObject".to_string(), s3_overrides);

        let service_cfg = ServiceConfiguration {
            rename_services_operation_action_map: HashMap::new(),
            rename_services_service_reference: HashMap::new(),
            rename_operations: HashMap::new(),
            resource_overrides,
        };

        let (_, service_reference_loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new());

        // Create parsed method call for get_object
        let parsed_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        // Test the enrichment
        let result = matcher
            .enrich_method_call(&parsed_call, &service_reference_loader)
            .await;
        assert!(
            result.is_ok(),
            "Enrichment should succeed for s3:GetObject with mixed overrides"
        );

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1, "Should have one enriched call");

        let enriched_call = &enriched_calls[0];
        let action = &enriched_call.actions[0];
        assert_eq!(action.resources.len(), 2, "Should have two resources");

        // Find bucket and object resources
        let bucket_resource = action
            .resources
            .iter()
            .find(|r| r.name == "bucket")
            .unwrap();
        let object_resource = action
            .resources
            .iter()
            .find(|r| r.name == "object")
            .unwrap();

        // Bucket should use override
        assert!(bucket_resource.arn_patterns.is_some());
        let bucket_patterns = bucket_resource.arn_patterns.as_ref().unwrap();
        assert_eq!(bucket_patterns.len(), 1);
        assert_eq!(
            bucket_patterns[0], "arn:aws:s3:::*",
            "Bucket should use override value"
        );

        // Object should use normal service reference lookup
        assert!(object_resource.arn_patterns.is_some());
        let object_patterns = object_resource.arn_patterns.as_ref().unwrap();
        assert_eq!(object_patterns.len(), 1);
        assert_eq!(
            object_patterns[0], "arn:${Partition}:s3:::${BucketName}/${ObjectName}",
            "Object should use normal service reference"
        );

        println!("✓ Test passed: Mixed resource overrides work correctly - overrides applied selectively");
    }
}
