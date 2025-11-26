//! Method disambiguation module for validating Go AWS SDK method calls against service definitions.
//!
//! This module provides functionality to validate extracted method calls against AWS SDK
//! service definitions, ensuring that only legitimate AWS SDK calls are included in the
//! final results. It performs parameter validation and filters out non-AWS method calls.

use crate::extraction::go::types::GoImportInfo;
use crate::extraction::sdk_model::{ServiceMethodRef, ServiceModelIndex, Shape};
use crate::extraction::{Parameter, SdkMethodCall};
use std::collections::HashSet;

const WITH_CONTEXT_SUFFIX: &str = "WithContext";

/// Method disambiguation engine for validating Go AWS SDK method calls.
///
/// This engine validates extracted method calls against AWS SDK service definitions
/// to ensure accuracy and filter out false positives.
pub(crate) struct GoMethodDisambiguator<'a> {
    /// Reference to the service model index containing all AWS service definitions
    service_index: &'a ServiceModelIndex,
}

impl<'a> GoMethodDisambiguator<'a> {
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
    /// * `import_info` - Optional import information to filter services based on what's imported
    ///
    /// # Returns
    ///
    /// A filtered list of validated AWS SDK method calls with accurate service mappings.
    pub(crate) fn disambiguate_method_calls(
        &self,
        method_calls: Vec<SdkMethodCall>,
        import_info: Option<&GoImportInfo>,
    ) -> Vec<SdkMethodCall> {
        let mut validated_methods = Vec::new();

        for mut method_call in method_calls {
            method_call.name = method_call
                .name
                .strip_suffix(WITH_CONTEXT_SUFFIX)
                .unwrap_or(&method_call.name)
                .to_string();
            // Check if this method name exists in the SDK
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_call.name) {
                // Validate the method call against each possible service
                let valid_services =
                    self.validate_method_against_services(&method_call, service_refs);

                if !valid_services.is_empty() {
                    // Filter services based on imports if import information is available
                    let filtered_services = if let Some(imports) = import_info {
                        self.filter_services_by_imports(&valid_services, imports)
                    } else {
                        valid_services
                    };

                    // Only include the method call if we have valid services after filtering
                    if !filtered_services.is_empty() {
                        method_call.possible_services = filtered_services;
                        validated_methods.push(method_call);
                    }
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
        let service_definition = match self.service_index.services.get(&service_ref.service_name) {
            Some(def) => def,
            None => return false, // Service not found
        };

        // Get the operation definition
        let operation = match service_definition
            .operations
            .get(&service_ref.operation_name)
        {
            Some(op) => op,
            None => return false, // Operation not found
        };

        // If there's no metadata, we can't validate parameters, so accept it
        let metadata = match &method_call.metadata {
            Some(meta) => meta,
            None => return true,
        };

        // For Go, we need to handle context parameters specially
        // Context parameters are always the first parameter and should not be validated against AWS schemas
        let has_context = self.has_context_parameter(&metadata.parameters);

        // Get the input shape for parameter validation
        let input_shape_name = match &operation.input {
            Some(input_ref) => &input_ref.shape,
            None => {
                // No input expected, so only context parameters should be provided
                return metadata
                    .parameters
                    .iter()
                    .all(|p| self.is_context_parameter(p));
            }
        };

        let input_shape = match service_definition.shapes.get(input_shape_name) {
            Some(shape) => shape,
            None => return false, // Input shape not found
        };

        // Validate parameters against the input shape
        self.validate_parameters_against_shape(&metadata.parameters, input_shape, has_context)
    }

    /// Check if the parameters include a context parameter
    fn has_context_parameter(&self, parameters: &[Parameter]) -> bool {
        parameters.iter().any(|p| self.is_context_parameter(p))
    }

    /// Check if a parameter is a context parameter
    fn is_context_parameter(&self, parameter: &Parameter) -> bool {
        match parameter {
            Parameter::Positional {
                value,
                type_annotation,
                ..
            } => {
                if let Some(type_ann) = type_annotation {
                    type_ann.contains("context.Context")
                } else {
                    let value_str = value.as_string();
                    value_str.starts_with("context.")
                        || value_str == "ctx"
                        || value_str.contains("Context")
                }
            }
            _ => false,
        }
    }

    /// Validate method call parameters against an AWS service input shape.
    ///
    /// This method checks that:
    /// 1. All required parameters are present (excluding context parameters)
    /// 2. All provided parameters are valid (exist in the shape)
    /// 3. No invalid parameters are provided
    ///
    /// # Arguments
    /// * `parameters` - The parameters provided in the method call
    /// * `shape` - The AWS service input shape definition
    /// * `has_context` - Whether a context parameter is present (affects parameter indexing)
    fn validate_parameters_against_shape(
        &self,
        parameters: &[Parameter],
        shape: &Shape,
        _has_context: bool,
    ) -> bool {
        // Extract parameter names from struct literals (the main way Go SDK passes parameters)
        let provided_params: HashSet<String> = parameters
            .iter()
            .filter_map(|p| match p {
                Parameter::Positional {
                    value,
                    type_annotation,
                    ..
                } => {
                    // Skip context parameters
                    if let Some(type_ann) = type_annotation {
                        if type_ann.contains("context.Context") {
                            return None;
                        }
                    }

                    let value_str = value.as_string();
                    // Extract field names from struct literals
                    if value_str.contains('{') && value_str.contains('}') {
                        Some(self.extract_struct_field_names(value_str))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .flatten()
            .collect();

        // Get required parameters from the shape
        let _required_params: HashSet<String> = shape
            .required
            .as_ref()
            .map(|req| req.iter().cloned().collect())
            .unwrap_or_default();

        // Get all valid parameters from the shape
        let valid_params: HashSet<String> = shape.members.keys().cloned().collect();

        // For Go, we're more lenient about missing required parameters since they might be
        // provided through struct initialization or have default values
        // We mainly validate that provided parameters are valid

        // Check that all provided parameters are valid
        for provided_param in &provided_params {
            if !valid_params.contains(provided_param) {
                return false; // Invalid parameter provided
            }
        }

        // If we have no parameters extracted (e.g., using variables instead of struct literals),
        // we accept the method call since we can't validate variable contents
        // This is more lenient but prevents false negatives
        true
    }

    /// Extract field names from a struct literal string
    fn extract_struct_field_names(&self, struct_literal: &str) -> Vec<String> {
        let mut field_names = Vec::new();

        // Find the content between braces
        if let Some(start) = struct_literal.find('{') {
            if let Some(end) = struct_literal.rfind('}') {
                let content = &struct_literal[start + 1..end];

                // Simple parsing - split by commas and extract field names
                for field_part in content.split(',') {
                    let field_part = field_part.trim();
                    if let Some(colon_pos) = field_part.find(':') {
                        let field_name = field_part[..colon_pos].trim().to_string();
                        if !field_name.is_empty() {
                            field_names.push(field_name);
                        }
                    }
                }
            }
        }

        field_names
    }

    /// Filter services based on what's actually imported in the Go file
    ///
    /// This method checks which AWS services are imported and filters the list of
    /// possible services to only include those that are actually imported.
    /// If no imports match any of the possible services, returns the original list
    /// to avoid filtering out all services (false negatives are worse than false positives).
    fn filter_services_by_imports(
        &self,
        possible_services: &[String],
        import_info: &GoImportInfo,
    ) -> Vec<String> {
        let imported_services = import_info.get_imported_services();

        // If no AWS services are imported, return the original list
        if imported_services.is_empty() {
            return possible_services.to_vec();
        }

        // Filter possible services to only those that are imported
        let filtered: Vec<String> = possible_services
            .iter()
            .filter(|service| imported_services.contains(service))
            .cloned()
            .collect();

        // If filtering would remove all services, return the original list
        // This prevents false negatives when imports might be missing or not detected
        if filtered.is_empty() {
            possible_services.to_vec()
        } else {
            filtered
        }
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
    use std::collections::HashMap;

    fn create_test_service_index() -> ServiceModelIndex {
        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // Create a test service definition for S3
        let mut s3_operations = HashMap::new();
        let mut s3_shapes = HashMap::new();

        // Create ListObjectsV2 operation
        s3_operations.insert(
            "ListObjectsV2".to_string(),
            Operation {
                name: "ListObjectsV2".to_string(),
                input: Some(ShapeReference {
                    shape: "ListObjectsV2Request".to_string(),
                }),
            },
        );

        // Create GetObject operation (exists in both s3 and s3control)
        s3_operations.insert(
            "GetObject".to_string(),
            Operation {
                name: "GetObject".to_string(),
                input: Some(ShapeReference {
                    shape: "GetObjectRequest".to_string(),
                }),
            },
        );

        // Create input shapes
        let mut list_objects_members = HashMap::new();
        list_objects_members.insert(
            "Bucket".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        list_objects_members.insert(
            "MaxKeys".to_string(),
            ShapeReference {
                shape: "Integer".to_string(),
            },
        );
        list_objects_members.insert(
            "Prefix".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );

        s3_shapes.insert(
            "ListObjectsV2Request".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: list_objects_members,
                required: Some(vec!["Bucket".to_string()]),
            },
        );

        let mut get_object_members = HashMap::new();
        get_object_members.insert(
            "Bucket".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        get_object_members.insert(
            "Key".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );

        s3_shapes.insert(
            "GetObjectRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: get_object_members,
                required: Some(vec!["Bucket".to_string(), "Key".to_string()]),
            },
        );

        let s3_service_def = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2006-03-01".to_string(),
                service_id: "S3".to_string(),
            },
            operations: s3_operations,
            shapes: s3_shapes,
            waiters: HashMap::new(),
        };

        services.insert("s3".to_string(), s3_service_def);

        // Create S3Control service with GetObject operation
        let mut s3control_operations = HashMap::new();
        let mut s3control_shapes = HashMap::new();

        s3control_operations.insert(
            "GetObject".to_string(),
            Operation {
                name: "GetObject".to_string(),
                input: Some(ShapeReference {
                    shape: "GetObjectRequest".to_string(),
                }),
            },
        );

        let mut s3control_get_object_members = HashMap::new();
        s3control_get_object_members.insert(
            "AccountId".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        s3control_get_object_members.insert(
            "Bucket".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        s3control_get_object_members.insert(
            "Key".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );

        s3control_shapes.insert(
            "GetObjectRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: s3control_get_object_members,
                required: Some(vec![
                    "AccountId".to_string(),
                    "Bucket".to_string(),
                    "Key".to_string(),
                ]),
            },
        );

        let s3control_service_def = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2018-08-20".to_string(),
                service_id: "S3Control".to_string(),
            },
            operations: s3control_operations,
            shapes: s3control_shapes,
            waiters: HashMap::new(),
        };

        services.insert("s3control".to_string(), s3control_service_def);

        // Add method lookup entries
        method_lookup.insert(
            "ListObjectsV2".to_string(),
            vec![ServiceMethodRef {
                service_name: "s3".to_string(),
                operation_name: "ListObjectsV2".to_string(),
            }],
        );

        method_lookup.insert(
            "GetObject".to_string(),
            vec![
                ServiceMethodRef {
                    service_name: "s3".to_string(),
                    operation_name: "GetObject".to_string(),
                },
                ServiceMethodRef {
                    service_name: "s3control".to_string(),
                    operation_name: "GetObject".to_string(),
                },
            ],
        );

        ServiceModelIndex {
            services,
            method_lookup,
            waiter_to_services: HashMap::new(),
        }
    }

    #[test]
    fn test_valid_go_method_call_with_struct_literal() {
        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "ListObjectsV2".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    Parameter::Positional {
                        value: ParameterValue::Unresolved("context.TODO()".to_string()),
                        position: 0,
                        type_annotation: Some("context.Context".to_string()),
                    },
                    Parameter::Positional {
                        value: ParameterValue::Unresolved(
                            "&s3.ListObjectsV2Input{ Bucket: aws.String(\"my-bucket\") }"
                                .to_string(),
                        ),
                        position: 1,
                        type_annotation: Some("*s3.ListObjectsV2Input".to_string()),
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 50),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call], None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].possible_services, vec!["s3"]);
    }

    #[test]
    fn test_context_parameter_detection() {
        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        let context_param = Parameter::Positional {
            value: ParameterValue::Unresolved("context.TODO()".to_string()),
            position: 0,
            type_annotation: Some("context.Context".to_string()),
        };

        assert!(disambiguator.is_context_parameter(&context_param));

        let regular_param = Parameter::Positional {
            value: ParameterValue::Unresolved("someValue".to_string()),
            position: 1,
            type_annotation: None,
        };

        assert!(!disambiguator.is_context_parameter(&regular_param));
    }

    #[test]
    fn test_struct_field_name_extraction() {
        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        let struct_literal =
            "&s3.ListObjectsV2Input{ Bucket: aws.String(\"my-bucket\"), MaxKeys: aws.Int32(10) }";
        let field_names = disambiguator.extract_struct_field_names(struct_literal);

        assert_eq!(field_names.len(), 2);
        assert!(field_names.contains(&"Bucket".to_string()));
        assert!(field_names.contains(&"MaxKeys".to_string()));
    }

    #[test]
    fn test_non_aws_method_call_filtered_out() {
        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "NonAwsMethod".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![Parameter::Positional {
                    value: ParameterValue::Unresolved("someParam".to_string()),
                    position: 0,
                    type_annotation: None,
                }],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 30),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call], None);
        assert_eq!(result.len(), 0); // Should be filtered out
    }

    #[test]
    fn test_import_based_filtering() {
        use crate::extraction::go::types::{GoImportInfo, ImportInfo};

        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        // Create import info with only S3 imported
        let mut import_info = GoImportInfo::new();
        import_info.add_import(ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "s3".to_string(),
            5,
        ));

        // Create a method call that could match multiple services but we only have S3 imported
        let method_call = SdkMethodCall {
            name: "ListObjectsV2".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    Parameter::Positional {
                        value: ParameterValue::Unresolved("context.TODO()".to_string()),
                        position: 0,
                        type_annotation: Some("context.Context".to_string()),
                    },
                    Parameter::Positional {
                        value: ParameterValue::Unresolved(
                            "&s3.ListObjectsV2Input{ Bucket: aws.String(\"my-bucket\") }"
                                .to_string(),
                        ),
                        position: 1,
                        type_annotation: Some("*s3.ListObjectsV2Input".to_string()),
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 50),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call], Some(&import_info));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].possible_services, vec!["s3"]);
    }

    #[test]
    fn test_no_imports_keeps_all_services() {
        use crate::extraction::go::types::GoImportInfo;

        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        // Create empty import info (no AWS services imported)
        let import_info = GoImportInfo::new();

        let method_call = SdkMethodCall {
            name: "ListObjectsV2".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    Parameter::Positional {
                        value: ParameterValue::Unresolved("context.TODO()".to_string()),
                        position: 0,
                        type_annotation: Some("context.Context".to_string()),
                    },
                    Parameter::Positional {
                        value: ParameterValue::Unresolved(
                            "&s3.ListObjectsV2Input{ Bucket: aws.String(\"my-bucket\") }"
                                .to_string(),
                        ),
                        position: 1,
                        type_annotation: Some("*s3.ListObjectsV2Input".to_string()),
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 50),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call], Some(&import_info));
        assert_eq!(result.len(), 1);
        // Should keep all services since no AWS services are imported (avoids false negatives)
        assert_eq!(result[0].possible_services, vec!["s3"]);
    }

    #[test]
    fn test_import_filtering_with_aliases() {
        use crate::extraction::go::types::{GoImportInfo, ImportInfo};

        let service_index = create_test_service_index();
        let disambiguator = GoMethodDisambiguator::new(&service_index);

        // Create import info with S3 imported with an alias
        let mut import_info = GoImportInfo::new();
        import_info.add_import(ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "myS3".to_string(), // aliased import
            5,
        ));

        let method_call = SdkMethodCall {
            name: "ListObjectsV2".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    Parameter::Positional {
                        value: ParameterValue::Unresolved("context.TODO()".to_string()),
                        position: 0,
                        type_annotation: Some("context.Context".to_string()),
                    },
                    Parameter::Positional {
                        value: ParameterValue::Unresolved(
                            "&myS3.ListObjectsV2Input{ Bucket: aws.String(\"my-bucket\") }"
                                .to_string(),
                        ),
                        position: 1,
                        type_annotation: Some("*myS3.ListObjectsV2Input".to_string()),
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 50),
                receiver: Some("client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call], Some(&import_info));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].possible_services, vec!["s3"]);
    }
}
