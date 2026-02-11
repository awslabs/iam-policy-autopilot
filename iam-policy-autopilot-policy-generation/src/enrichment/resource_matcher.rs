//! Resource Matcher for combining OperationAction maps with Service Reference data
//!
//! This module provides the ResourceMatcher that coordinates operation
//! action maps with Service Definition Files to generate enriched method calls
//! with complete IAM metadata.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Action, Context, EnrichedSdkMethodCall, Explanation, Reason, Resource};
use crate::enrichment::operation_fas_map::{OperationFasMap, OperationFasMaps};
use crate::enrichment::service_reference::ServiceReference;
use crate::enrichment::{Condition, Operation, ServiceReferenceLoader};
use crate::errors::{ExtractorError, Result};
use crate::service_configuration::ServiceConfiguration;
use crate::{SdkMethodCall, SdkType};

#[derive(Clone, Debug)]
struct FasExpansion {
    dependency_graph: HashMap<Arc<Operation>, Vec<Arc<Operation>>>,
}

impl FasExpansion {
    fn new(
        service_cfg: &ServiceConfiguration,
        fas_maps: &OperationFasMaps,
        initial: Operation,
    ) -> Self {
        let mut dependency_graph: HashMap<Arc<Operation>, Vec<Arc<Operation>>> = HashMap::new();
        let initial_arc = Arc::new(initial);

        dependency_graph.insert(Arc::clone(&initial_arc), Vec::new()); // Root has no dependencies

        let mut to_process = vec![Arc::clone(&initial_arc)];

        while !to_process.is_empty() {
            let mut newly_discovered = Vec::new();

            for current in &to_process {
                let service_name = &current.service;

                match Self::find_operation_fas_map_for_service(service_cfg, fas_maps, service_name)
                {
                    Some(operation_fas_map) => {
                        let service_operation_name = current.service_operation_name();
                        log::debug!("Looking up operation {service_operation_name}");

                        if let Some(additional_operations) = operation_fas_map
                            .fas_operations
                            .get(&service_operation_name)
                        {
                            for additional_op in additional_operations {
                                let new_op = Arc::new(Operation::from(additional_op.clone()));

                                if let Some(existing_deps) = dependency_graph.get_mut(&new_op) {
                                    // Operation already exists, add this dependency relationship
                                    existing_deps.push(Arc::clone(current));
                                } else {
                                    // New operation
                                    dependency_graph
                                        .insert(Arc::clone(&new_op), vec![Arc::clone(current)]);
                                    newly_discovered.push(Arc::clone(&new_op));
                                }
                            }
                        } else {
                            log::debug!("Did not find {service_operation_name}");
                        }
                    }
                    None => {
                        log::debug!("No FAS map found for service: {service_name}");
                    }
                }
            }

            let newly_discovered_count = newly_discovered.len();
            to_process = newly_discovered;

            log::debug!("FAS expansion discovered {newly_discovered_count} new operations");
        }

        log::debug!(
            "FAS expansion completed with {} total operations",
            dependency_graph.len()
        );
        Self { dependency_graph }
    }

    /// Find OperationFas map for a specific service
    fn find_operation_fas_map_for_service(
        service_cfg: &ServiceConfiguration,
        fas_maps: &OperationFasMaps,
        service_name: &str,
    ) -> Option<Arc<OperationFasMap>> {
        fas_maps
            .get(
                service_cfg
                    .rename_service_operation_action_map(service_name)
                    .as_ref(),
            )
            .cloned()
    }

    fn operations(&self) -> impl Iterator<Item = &Arc<Operation>> {
        self.dependency_graph.keys()
    }

    fn complete_provenance_chain(&self, op: Arc<Operation>) -> Vec<Arc<Operation>> {
        let mut result = vec![];
        if let Some(deps) = self.dependency_graph.get(&op) {
            for dep in deps {
                result.push(Arc::clone(dep));
            }
        }
        // Add the initial operation
        result.push(Arc::clone(&op));
        result
    }
}

/// ResourceMatcher coordinates OperationAction maps and Service Reference data to generate enriched method calls
///
/// This struct provides the core functionality for the 3-stage enrichment pipeline,
/// combining parsed method calls with operation action maps and Service
/// Definition Files to produce complete IAM metadata.
#[derive(derive_new::new, Debug, Clone)]
pub(crate) struct ResourceMatcher {
    service_cfg: Arc<ServiceConfiguration>,
    fas_maps: OperationFasMaps,
    sdk: SdkType,
}

// TODO: Make this configurable: https://github.com/awslabs/iam-policy-autopilot/issues/19
const RESOURCE_CUTOFF: usize = 5;

impl ResourceMatcher {
    /// Enrich a parsed method call with OperationAction maps, FAS maps, and Service
    /// Reference data
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
            // Create enriched method call for this service
            if let Some(enriched_call) = self
                .create_enriched_method_call(parsed_call, service_name, service_reference_loader)
                .await?
            {
                enriched_calls.push(enriched_call);
            }
        }

        Ok(enriched_calls)
    }

    fn make_condition<T: Context>(context: &[T]) -> Vec<Condition> {
        let mut result = vec![];
        for ctx in context {
            result.push(Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: ctx.key().to_string(),
                values: ctx.values().to_vec(),
            });
        }
        result
    }

    /// Create an enriched method call for a specific service
    async fn create_enriched_method_call<'a>(
        &self,
        parsed_call: &'a SdkMethodCall,
        service_name: &str,
        service_reference_loader: &ServiceReferenceLoader,
    ) -> Result<Option<EnrichedSdkMethodCall<'a>>> {
        log::debug!(
            "Creating method call for service: {}, and method name: {}",
            service_name,
            parsed_call.name
        );

        // Store the original service name from parsed_call for use in explanations
        let original_service_name = service_name;

        let initial = Operation::from_call(
            parsed_call,
            service_name,
            &self.service_cfg,
            self.sdk,
            service_reference_loader,
        )
        .await?;

        log::debug!("Expanded {initial:?}");
        // Use fixed-point algorithm to safely expand FAS operations until no new operations are found
        let fas_expansion = FasExpansion::new(&self.service_cfg, &self.fas_maps, initial);

        log::debug!("to\n{:?}", fas_expansion.dependency_graph);

        let mut enriched_actions = vec![];

        for op in fas_expansion.operations() {
            log::debug!(
                "Creating actions for operation {:?}",
                op.service_operation_name()
            );
            log::debug!("  with context {:?}", op.context());

            // Find the corresponding SDF using the cache
            let service_reference = service_reference_loader.load(&op.service).await?;
            match service_reference {
                None => {
                    log::debug!("Skipping operation due to no service reference");
                    continue;
                }
                Some(service_reference) => {
                    if let Some(operation_to_authorized_actions) =
                        &service_reference.operation_to_authorized_actions
                    {
                        log::debug!("Looking up {}", &op.service_operation_name());
                        if let Some(operation_to_authorized_action) =
                            operation_to_authorized_actions.get(&op.service_operation_name())
                        {
                            log::debug!(
                                "Found operation action map for {:?}",
                                operation_to_authorized_action.name
                            );
                            for action in &operation_to_authorized_action.authorized_actions {
                                let enriched_resources = self
                                    .find_resources_for_action_in_service_reference(
                                        &action.name,
                                        &service_reference,
                                    )?;
                                let enriched_resources =
                                    if RESOURCE_CUTOFF <= enriched_resources.len() {
                                        vec![Resource::new("*".to_string(), None)]
                                    } else {
                                        enriched_resources
                                    };

                                // Combine conditions from FAS operation context and AuthorizedAction context
                                let mut conditions = Self::make_condition(op.context());

                                // Add conditions from AuthorizedAction context if present
                                if let Some(auth_context) = &action.context {
                                    conditions.extend(Self::make_condition(std::slice::from_ref(
                                        auth_context,
                                    )));
                                }

                                let ops = fas_expansion.complete_provenance_chain(Arc::clone(op));

                                // Create explanation for this action
                                let explanation = Explanation {
                                    reasons: vec![Reason::new(ops)],
                                };
                                let enriched_action = Action::new(
                                    action.name.clone(),
                                    enriched_resources,
                                    conditions,
                                    explanation,
                                );
                                log::debug!("Created action: {enriched_action:?}");
                                enriched_actions.push(enriched_action);
                            }
                        } else {
                            // Fallback: operation not found in operation action map, create basic action
                            // This ensures we don't filter out operations, only ADD additional ones from the map
                            if let Some(a) =
                                self.create_fallback_action(op, &fas_expansion, &service_reference)?
                            {
                                log::debug!("Created fallback action due to no entry in operation action map: {a:?}");
                                enriched_actions.push(a);
                            }
                        }
                    } else {
                        // Fallback: operation action map does not exist, create basic action
                        if let Some(a) =
                            self.create_fallback_action(op, &fas_expansion, &service_reference)?
                        {
                            log::debug!("Created fallback action due to no operation action map for service: {a:?}");
                            enriched_actions.push(a);
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
            service: original_service_name.to_string(),
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
        op: &Arc<Operation>,
        fas_expansion_result: &FasExpansion,
        service_reference: &ServiceReference,
    ) -> Result<Option<Action>> {
        let action_name = op.service_operation_name();

        // Sanity check that the action exists in the SDF
        if !service_reference.actions.contains_key(&op.name) {
            log::debug!(
                "Not creating fallback action: service reference doesn't contain key: {action_name:?}"
            );
            return Ok(None);
        }

        // Look up the action in the Service Reference to find associated resources
        let resources =
            self.find_resources_for_action_in_service_reference(&action_name, service_reference)?;

        // Create explanation for fallback action
        let explanation = Explanation {
            reasons: vec![Reason::new(
                fas_expansion_result.complete_provenance_chain(Arc::clone(op)),
            )],
        };

        Ok(Some(Action::new(
            action_name.clone(),
            resources,
            vec![],
            explanation,
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

        log::debug!("find_resources_for_action_in_service_reference: action = {action}");
        log::debug!(
            "find_resources_for_action_in_service_reference: service_reference.actions = {:?}",
            service_reference.actions
        );
        let mut result = vec![];
        if let Some(action) = service_reference.actions.get(action) {
            let overrides = self.service_cfg.resource_overrides.get(action_name);
            for resource in &action.resources {
                let service_reference_resource = if let Some(r#override) =
                    overrides.and_then(|m| m.get(resource))
                {
                    log::debug!(
                        "find_resources_for_action_in_service_reference: resource override = {override}"
                    );
                    Resource::new(resource.clone(), Some(vec![r#override.clone()]))
                } else {
                    log::debug!(
                        "find_resources_for_action_in_service_reference: looking up resource = {resource}"
                    );
                    log::debug!(
                        "find_resources_for_action_in_service_reference: resources = {:?}",
                        service_reference.resources
                    );
                    let arn_patterns = service_reference.resources.get(resource).cloned();
                    log::debug!(
                            "find_resources_for_action_in_service_reference: arn_pattern = {arn_patterns:?}"
                        );
                    Resource::new(resource.clone(), arn_patterns)
                };
                result.push(service_reference_resource);
            }
        }

        // If no resources found, that's still valid (some actions don't require specific resources)
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::enrichment::operation_fas_map::{FasContext, FasOperation, OperationFasMap};
    use crate::enrichment::{mock_remote_service_reference, OperationSource};

    fn create_test_parsed_method_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
    }

    fn create_empty_service_config() -> Arc<ServiceConfiguration> {
        Arc::new(ServiceConfiguration {
            rename_services_operation_action_map: HashMap::new(),
            rename_services_service_reference: HashMap::new(),
            smithy_botocore_service_name_mapping: HashMap::new(),
            resource_overrides: HashMap::new(),
        })
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
                "SmithyBotocoreServiceNameMapping": {},
                "ResourceOverrides": {}
            }"#;

            serde_json::from_str(json_content)
                .expect("Failed to deserialize test ServiceConfiguration JSON")
        }

        let service_cfg = create_test_service_configuration();

        let (_, service_reference_loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new(), SdkType::Boto3);
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
            smithy_botocore_service_name_mapping: HashMap::new(),
            resource_overrides: HashMap::new(),
        };

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new(), SdkType::Boto3);

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
        let service_cfg = create_empty_service_config();

        let matcher = ResourceMatcher::new(service_cfg, HashMap::new(), SdkType::Boto3);
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
            smithy_botocore_service_name_mapping: HashMap::new(),
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

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new(), SdkType::Boto3);

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
            smithy_botocore_service_name_mapping: HashMap::new(),
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

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new(), SdkType::Boto3);

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
            smithy_botocore_service_name_mapping: HashMap::new(),
            resource_overrides,
        };

        let (_, service_reference_loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        let matcher = ResourceMatcher::new(Arc::new(service_cfg), HashMap::new(), SdkType::Boto3);

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

    #[tokio::test]
    async fn test_fas_expansion_fixed_point_no_cycles() {
        use std::collections::HashMap;

        // Create a simple service configuration
        let service_cfg = create_empty_service_config();

        // Create a mock FAS map with no cycles: A -> B -> C (linear chain)
        let fas_maps = {
            let mut fas_maps = HashMap::new();

            // Service A: GetObject -> Service B: Decrypt
            let mut service_a_operations = HashMap::new();
            service_a_operations.insert(
                "service-a:GetObject".to_string(),
                vec![FasOperation::new(
                    "Decrypt".to_string(),
                    "service-b".to_string(),
                    vec![FasContext::new(
                        "test".to_string(),
                        vec!["value".to_string()],
                    )],
                )],
            );

            // Service B: Decrypt -> Service C: Log
            let mut service_b_operations = HashMap::new();
            service_b_operations.insert(
                "service-b:Decrypt".to_string(),
                vec![FasOperation::new(
                    "Log".to_string(),
                    "service-c".to_string(),
                    vec![FasContext::new(
                        "test2".to_string(),
                        vec!["value2".to_string()],
                    )],
                )],
            );

            // Service C: Log -> nothing (terminal)
            let service_c_operations = HashMap::new();

            fas_maps.insert(
                "service-a".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_a_operations,
                }),
            );
            fas_maps.insert(
                "service-b".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_b_operations,
                }),
            );
            fas_maps.insert(
                "service-c".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_c_operations,
                }),
            );
            fas_maps
        };

        // Test expansion starting from GetObject
        let initial = Operation::new(
            "service-a".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let fas_expansion = FasExpansion::new(&service_cfg, &fas_maps, initial);

        assert_eq!(
            fas_expansion.dependency_graph.len(),
            3,
            "Should have exactly 3 operations: GetObject, Decrypt, Log"
        );

        // Verify all expected operations are present
        let operation_names: std::collections::HashSet<String> = fas_expansion
            .operations()
            .map(|op| op.service_operation_name())
            .collect();

        assert!(operation_names.contains("service-a:GetObject"));
        assert!(operation_names.contains("service-b:Decrypt"));
        assert!(operation_names.contains("service-c:Log"));

        println!(
            "✓ Test passed: Fixed-point expansion works correctly for non-cyclic FAS operations"
        );
    }

    #[tokio::test]
    async fn test_fas_expansion_cycle_detection() {
        use std::collections::HashMap;

        // Create a simple service configuration
        let service_cfg = create_empty_service_config();

        // Create a mock FAS map with a cycle: A -> B -> A
        let fas_maps = {
            let mut fas_maps = HashMap::new();

            // Service A: GetObject -> Service B: Decrypt
            let mut service_a_operations = HashMap::new();
            service_a_operations.insert(
                "service-a:GetObject".to_string(),
                vec![FasOperation::new(
                    "Decrypt".to_string(),
                    "service-b".to_string(),
                    vec![FasContext::new(
                        "test".to_string(),
                        vec!["value".to_string()],
                    )],
                )],
            );

            // Service B: Decrypt -> Service A: GetObject (creates cycle!)
            let mut service_b_operations = HashMap::new();
            service_b_operations.insert(
                "service-b:Decrypt".to_string(),
                vec![FasOperation::new(
                    "GetObject".to_string(),
                    "service-a".to_string(),
                    vec![FasContext::new(
                        "test2".to_string(),
                        vec!["value2".to_string()],
                    )],
                )],
            );

            fas_maps.insert(
                "service-a".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_a_operations,
                }),
            );
            fas_maps.insert(
                "service-b".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_b_operations,
                }),
            );
            fas_maps
        };

        // Test expansion starting from GetObject - should detect cycle and terminate
        let initial = Operation::new(
            "service-a".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let fas_expansion = FasExpansion::new(&service_cfg, &fas_maps, initial);

        // Debug: print what operations we actually got
        let operation_names: std::collections::HashSet<String> = fas_expansion
            .operations()
            .map(|op| op.service_operation_name())
            .collect();

        // 3 operations, note that GetObject occurs twice, once with and once without context
        assert!(
            fas_expansion.dependency_graph.len() == 3,
            "Should have 3 operations"
        );

        // Verify expected operations are present
        assert!(operation_names.contains("service-a:GetObject"));
        assert!(operation_names.contains("service-b:Decrypt"));

        println!(
            "✓ Test passed: Fixed-point expansion handles cycles correctly without infinite loops"
        );
    }

    #[tokio::test]
    async fn test_fas_expansion_complex_cycle_with_max_iterations() {
        use std::collections::HashMap;

        // Create a service configuration
        let service_cfg = create_empty_service_config();

        let fas_maps = {
            let mut fas_maps = HashMap::new();

            // Create a chain that loops back: A -> B -> C -> D -> A
            let operations_data = vec![
                ("service-a", "GetObject", "service-b", "Decrypt"),
                ("service-b", "Decrypt", "service-c", "Validate"),
                ("service-c", "Validate", "service-d", "Log"),
                ("service-d", "Log", "service-a", "GetObject"), // Back to start
            ];

            for (from_service, from_op, to_service, to_op) in operations_data {
                let mut operations = HashMap::new();
                operations.insert(
                    format!("{}:{}", from_service, from_op),
                    vec![FasOperation::new(
                        to_op.to_string(),
                        to_service.to_string(),
                        vec![FasContext::new(
                            "cycle".to_string(),
                            vec!["test".to_string()],
                        )],
                    )],
                );

                fas_maps.insert(
                    from_service.to_string(),
                    Arc::new(OperationFasMap {
                        fas_operations: operations,
                    }),
                );
            }
            fas_maps
        };

        let initial = Operation::new(
            "service-a".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let fas_expansion = FasExpansion::new(&service_cfg, &fas_maps, initial);

        // We have 5 operations, note that GetObject occurs twice, once with context and the initial one without
        assert!(
            fas_expansion.dependency_graph.len() == 5,
            "Should have 5 operations in the cycle"
        );
    }

    #[tokio::test]
    async fn test_fas_expansion_empty_initial() {
        use std::collections::HashMap;

        let service_cfg = create_empty_service_config();
        let fas_maps = HashMap::new();

        let initial = Operation::new(
            "non-existent-service".to_string(),
            "NonExistentOperation".to_string(),
            OperationSource::Provided,
        );

        let fas_expansion = FasExpansion::new(&service_cfg, &fas_maps, initial.clone());
        assert_eq!(
            fas_expansion.dependency_graph.len(),
            1,
            "Should contain only the initial operation"
        );

        let operations: Vec<_> = fas_expansion.operations().collect();
        assert_eq!(
            **operations[0], initial,
            "Should contain the initial operation"
        );
        assert!(
            !matches!(operations[0].source, OperationSource::Fas(_)),
            "Initial operation should not be from FAS expansion"
        );

        println!("✓ Test passed: Handles case with no additional FAS operations");
    }

    #[tokio::test]
    async fn test_fas_expansion_self_cycle_empty_context() {
        use std::collections::HashMap;

        // Create a simple service configuration
        let service_cfg = create_empty_service_config();

        // Create a FAS map where A -> A with empty context (self-referential)
        let fas_maps = {
            let mut fas_maps = HashMap::new();

            // Service A: GetObject -> Service A: GetObject (with empty context)
            let mut service_a_operations = HashMap::new();
            service_a_operations.insert(
                "service-a:GetObject".to_string(),
                vec![FasOperation::new(
                    "GetObject".to_string(),
                    "service-a".to_string(),
                    Vec::new(), // Empty context - same as initial
                )],
            );

            fas_maps.insert(
                "service-a".to_string(),
                Arc::new(OperationFasMap {
                    fas_operations: service_a_operations,
                }),
            );
            fas_maps
        };

        // Test expansion starting from GetObject with empty context
        let initial = Operation::new(
            "service-a".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let fas_expansion = FasExpansion::new(&service_cfg, &fas_maps, initial.clone());

        // Should have exactly 1 operation since A->A with same context creates no new operations
        assert_eq!(
            fas_expansion.dependency_graph.len(),
            1,
            "Self-cycle with identical context should result in exactly 1 operation"
        );

        let operations: Vec<_> = fas_expansion.operations().collect();
        assert_eq!(
            **operations[0], initial,
            "Should contain the initial operation"
        );
        assert!(
            !matches!(operations[0].source, OperationSource::Fas(_)),
            "Initial operation should not be from FAS expansion"
        );

        println!("✓ Test passed: Self-cycle with empty context handled correctly");
    }

    /// Helper function to create RDS service reference mock with multiple DB operations
    /// Includes operations with and without SDK method mappings to test different scenarios
    async fn mock_rds_service_reference(mock_server: &wiremock::MockServer) {
        mock_remote_service_reference::mock_server_service_reference_response(
            mock_server,
            "rds",
            serde_json::json!({
                "Name": "rds",
                "Resources": [
                    {
                        "Name": "database-cluster",
                        "ARNFormats": ["arn:${Partition}:rds:${Region}:${Account}:cluster:${DatabaseClusterIdentifier}"]
                    }
                ],
                "Actions": [
                    {
                        "Name": "ModifyDBCluster",
                        "Resources": [{"Name": "database-cluster"}]
                    }
                ],
                "Operations": [
                    {
                        "Name": "ModifyDBCluster",
                        "AuthorizedActions": [{"Name": "ModifyDBCluster", "Service": "rds"}],
                        "SDK": [{"Name": "rds", "Method": "modify_db_cluster", "Package": "Boto3"}]
                    }
                ]
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn test_boto3_method_name_requires_lookup() {
        // Test that boto3 methods are correctly mapped using service reference SDK mapping
        let config = create_empty_service_config();
        let matcher = ResourceMatcher::new(config, HashMap::new(), SdkType::Boto3);

        let (mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        mock_rds_service_reference(&mock_server).await;

        let parsed_method = SdkMethodCall {
            name: "modify_db_cluster".to_string(),
            possible_services: vec!["rds".to_string()],
            metadata: None,
        };

        let result = matcher.enrich_method_call(&parsed_method, &loader).await;
        assert!(result.is_ok());

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1);
        assert_eq!(enriched_calls[0].method_name, "modify_db_cluster");
        assert_eq!(enriched_calls[0].service, "rds");
        assert_eq!(enriched_calls[0].actions[0].name, "rds:ModifyDBCluster");
    }

    #[tokio::test]
    async fn test_non_boto3_sdk_uses_extracted_name_directly() {
        // Test that non-Boto3 SDKs (e.g., Go) use the extracted operation name directly without renaming
        let config = create_empty_service_config();
        let matcher = ResourceMatcher::new(config, HashMap::new(), SdkType::Other);

        let (mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        mock_rds_service_reference(&mock_server).await;

        // Go SDK extracts operation names in PascalCase (e.g., CreateDBCluster)
        let parsed_method = SdkMethodCall {
            name: "ModifyDBCluster".to_string(),
            possible_services: vec!["rds".to_string()],
            metadata: None,
        };

        let result = matcher.enrich_method_call(&parsed_method, &loader).await;
        assert!(result.is_ok());

        let enriched_calls = result.unwrap();
        assert_eq!(enriched_calls.len(), 1);
        assert_eq!(enriched_calls[0].method_name, "ModifyDBCluster");
        assert_eq!(enriched_calls[0].service, "rds");
        // Should use the operation name directly without any transformation
        assert_eq!(enriched_calls[0].actions[0].name, "rds:ModifyDBCluster");
    }
}
