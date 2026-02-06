//! Method disambiguation module for validating AWS SDK method calls against service definitions.
//!
//! This module provides functionality to validate extracted method calls against AWS SDK
//! service definitions, ensuring that only legitimate AWS SDK calls are included in the
//! final results. It performs parameter validation and filters out non-AWS method calls.

use crate::extraction::sdk_model::{ServiceMethodRef, ServiceModelIndex, Shape};
use crate::extraction::{Parameter, SdkMethodCall};
use std::collections::HashSet;

/// Method disambiguation engine for validating AWS SDK method calls.
///
/// This engine validates extracted method calls against AWS SDK service definitions
/// to ensure accuracy and filter out false positives.
pub(crate) struct MethodDisambiguator<'a> {
    /// Reference to the service model index containing all AWS service definitions
    service_index: &'a ServiceModelIndex,
}

impl<'a> MethodDisambiguator<'a> {
    /// Create a new method disambiguator with the given service index.
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        Self { service_index }
    }

    /// Disambiguate and validate a list of method calls.
    ///
    /// This method processes each method call and validates it against the AWS SDK
    /// service definitions. Method calls that don't match any valid AWS SDK operation
    /// or have invalid parameters are filtered out.
    ///
    /// # Arguments
    ///
    /// * `method_calls` - List of extracted method calls to validate
    ///
    /// # Returns
    ///
    /// A filtered list of validated AWS SDK method calls with accurate service mappings.
    pub(crate) fn disambiguate_method_calls(
        &self,
        method_calls: Vec<SdkMethodCall>,
    ) -> Vec<SdkMethodCall> {
        let mut validated_methods = Vec::new();

        for mut method_call in method_calls {
            // Check if this method name exists in the SDK
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_call.name) {
                // Validate the method call against each possible service
                let valid_services =
                    self.validate_method_against_services(&method_call, service_refs);

                if !valid_services.is_empty() {
                    // Update the method call with only the valid services
                    method_call.possible_services = valid_services;
                    validated_methods.push(method_call);
                }
                // If no valid services remain, the method call is filtered out
            }
            // If method name doesn't exist in SDK, it's filtered out
        }

        validated_methods
    }

    /// Validate a method call against a list of possible services.
    ///
    /// This method checks each service to see if the method call's parameters
    /// are compatible with the service's operation definition.
    fn validate_method_against_services(
        &self,
        method_call: &SdkMethodCall,
        service_refs: &[ServiceMethodRef],
    ) -> Vec<String> {
        let mut valid_services = Vec::new();

        for service_ref in service_refs {
            if self.validate_method_against_service(method_call, service_ref) {
                valid_services.push(service_ref.service_name.clone());
            }
        }

        valid_services
    }

    /// Validate a method call against a specific service operation.
    ///
    /// This method performs detailed parameter validation to ensure the method call
    /// is compatible with the AWS service operation definition.
    fn validate_method_against_service(
        &self,
        method_call: &SdkMethodCall,
        service_ref: &ServiceMethodRef,
    ) -> bool {
        // Get the service definition
        let service_definition =
            if let Some(def) = self.service_index.services.get(&service_ref.service_name) {
                def
            } else {
                log::debug!("in validate_method_against_service: Service not found");
                return false; // Service not found
            };

        // Get the operation definition
        let operation = if let Some(op) = service_definition
            .operations
            .get(&service_ref.operation_name)
        {
            op
        } else {
            log::debug!("in validate_method_against_service: operation not found");
            return false; // Operation not found
        };

        // If there's no metadata, we can't validate parameters, so accept it
        let metadata = match &method_call.metadata {
            Some(meta) => meta,
            None => return true,
        };

        // For mixed scenarios with dictionary unpacking, we still validate explicit parameters
        // but are more lenient about missing required parameters (they might be in the unpacked dict)
        let has_unpacking = metadata.has_dictionary_unpacking();

        // Get the input shape for parameter validation
        let input_shape_name = match &operation.input {
            Some(input_ref) => &input_ref.shape,
            None => return metadata.parameters.is_empty(), // No input expected, so no parameters should be provided
        };

        let input_shape = match service_definition.shapes.get(input_shape_name) {
            Some(shape) => shape,
            None => return false, // Input shape not found
        };

        // Validate parameters against the input shape
        // If unpacking is present, we're more lenient about missing required parameters
        self.validate_parameters_against_shape(&metadata.parameters, input_shape, has_unpacking)
    }

    /// Validate method call parameters against an AWS service input shape.
    ///
    /// This method checks that:
    /// 1. All required parameters are present (unless unpacking is used)
    /// 2. All provided parameters are valid (exist in the shape)
    /// 3. No invalid parameters are provided
    ///
    /// # Arguments
    /// * `parameters` - The explicit parameters provided in the method call
    /// * `shape` - The AWS service input shape definition
    /// * `has_unpacking` - Whether dictionary unpacking is used (affects required parameter validation)
    fn validate_parameters_against_shape(
        &self,
        parameters: &[Parameter],
        shape: &Shape,
        has_unpacking: bool,
    ) -> bool {
        // Extract parameter names from the method call, filtering by parameter type instead of name prefix
        let provided_params: HashSet<String> = parameters
            .iter()
            .filter_map(|p| match p {
                Parameter::Keyword { name, .. } => Some(name.clone()), // Only include keyword parameters for validation
                _ => None,
            })
            .collect();

        // Get required parameters from the shape
        let required_params: HashSet<String> = shape
            .required
            .as_ref()
            .map(|req| req.iter().cloned().collect())
            .unwrap_or_default();

        // Get all valid parameters from the shape
        // TODO: Make this case-insensitive like Go disambiguation to handle inconsistent
        // AWS model casing. See: https://github.com/awslabs/iam-policy-autopilot/issues/57
        let valid_params: HashSet<String> = shape.members.keys().cloned().collect();

        // Check that all required parameters are provided
        // If unpacking is present, missing required parameters might be provided via unpacking
        if !has_unpacking {
            for required_param in &required_params {
                if !provided_params.contains(required_param) {
                    return false; // Missing required parameter
                }
            }
        }

        // Check that all provided parameters are valid
        for provided_param in &provided_params {
            if !valid_params.contains(provided_param) {
                return false; // Invalid parameter provided
            }
        }

        true // All validations passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::sdk_model::{
        Operation, SdkServiceDefinition, ServiceMetadata, ServiceMethodRef, ServiceModelIndex,
        Shape, ShapeReference,
    };
    use crate::extraction::{Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata};
    use crate::Location;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn create_test_service_index() -> ServiceModelIndex {
        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // Create a test service definition for API Gateway V2
        let mut operations = HashMap::new();
        let mut shapes = HashMap::new();

        // Create CreateApiMapping operation
        operations.insert(
            "CreateApiMapping".to_string(),
            Operation {
                name: "CreateApiMapping".to_string(),
                input: Some(ShapeReference {
                    shape: "CreateApiMappingRequest".to_string(),
                }),
            },
        );

        // Create input shape with required and optional parameters
        let mut members = HashMap::new();
        members.insert(
            "DomainName".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        members.insert(
            "Stage".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        members.insert(
            "ApiId".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        members.insert(
            "ApiMappingKey".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );

        shapes.insert(
            "CreateApiMappingRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members,
                required: Some(vec![
                    "DomainName".to_string(),
                    "Stage".to_string(),
                    "ApiId".to_string(),
                ]),
            },
        );

        let service_def = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2018-11-29".to_string(),
                service_id: "ApiGatewayV2".to_string(),
            },
            operations,
            shapes,
        };

        services.insert("apigatewayv2".to_string(), service_def);

        // Add method lookup entry
        method_lookup.insert(
            "create_api_mapping".to_string(),
            vec![ServiceMethodRef {
                service_name: "apigatewayv2".to_string(),
                operation_name: "CreateApiMapping".to_string(),
            }],
        );

        ServiceModelIndex {
            services,
            method_lookup,
            waiter_lookup: HashMap::new(),
        }
    }

    #[test]
    fn test_valid_method_call_with_all_required_params() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "create_api_mapping".to_string(),
                parameters: vec![
                    Parameter::Keyword {
                        name: "DomainName".to_string(),
                        value: ParameterValue::Resolved("example.com".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "Stage".to_string(),
                        value: ParameterValue::Resolved("prod".to_string()),
                        position: 1,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "ApiId".to_string(),
                        value: ParameterValue::Resolved("abc123".to_string()),
                        position: 2,
                        type_annotation: None,
                    },
                ],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 50)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].possible_services, vec!["apigatewayv2"]);
    }

    #[test]
    fn test_invalid_method_call_missing_required_param() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "create_api_mapping".to_string(),
                parameters: vec![
                    Parameter::Keyword {
                        name: "DomainName".to_string(),
                        value: ParameterValue::Resolved("example.com".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    // Missing required Stage and ApiId parameters
                ],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 30)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out
    }

    #[test]
    fn test_method_call_with_dictionary_unpacking() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "create_api_mapping".to_string(),
                parameters: vec![Parameter::DictionarySplat {
                    expression: "**params".to_string(),
                    position: 0,
                }],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 30)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 1); // Should be kept for future analysis
        assert_eq!(result[0].possible_services, vec!["apigatewayv2"]);
    }

    #[test]
    fn test_non_aws_method_call_filtered_out() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "non_aws_method".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "non_aws_method".to_string(),
                parameters: vec![Parameter::Keyword {
                    name: "custom_param".to_string(),
                    value: ParameterValue::Resolved("value".to_string()),
                    position: 0,
                    type_annotation: None,
                }],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 30)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out
    }

    #[test]
    fn test_mixed_dictionary_unpacking_with_explicit_params() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        // Test case: explicit DomainName + dictionary unpacking for other params
        // This should be valid even though Stage and ApiId are missing explicitly
        // (they could be provided via **params)
        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "create_api_mapping".to_string(),
                parameters: vec![
                    Parameter::Keyword {
                        name: "DomainName".to_string(),
                        value: ParameterValue::Resolved("example.com".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "ApiMappingKey".to_string(), // Optional parameter
                        value: ParameterValue::Resolved("v1".to_string()),
                        position: 1,
                        type_annotation: None,
                    },
                    Parameter::DictionarySplat {
                        expression: "**params".to_string(),
                        position: 2,
                    },
                ],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 50)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 1); // Should be kept - explicit params are valid
        assert_eq!(result[0].possible_services, vec!["apigatewayv2"]);
    }

    #[test]
    fn test_mixed_dictionary_unpacking_with_invalid_explicit_param() {
        let service_index = create_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        // Test case: invalid explicit parameter + dictionary unpacking
        // This should be filtered out because the explicit parameter is invalid
        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                expr: "create_api_mapping".to_string(),
                parameters: vec![
                    Parameter::Keyword {
                        name: "DomainName".to_string(),
                        value: ParameterValue::Resolved("example.com".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "InvalidParam".to_string(), // This parameter doesn't exist in AWS API
                        value: ParameterValue::Resolved("invalid".to_string()),
                        position: 1,
                        type_annotation: None,
                    },
                ],
                return_type: None,
                location: Location::new(PathBuf::new(), (1, 1), (1, 50)),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out due to invalid explicit parameter
    }
}
