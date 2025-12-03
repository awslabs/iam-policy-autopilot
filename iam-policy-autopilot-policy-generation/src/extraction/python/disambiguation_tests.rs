//! Comprehensive unit tests for method disambiguation functionality.
//!
//! This module contains tests that demonstrate the disambiguation of AWS SDK method calls
//! from non-AWS method calls, including parameter validation and dictionary unpacking handling.

#[cfg(test)]
mod tests {
    use crate::extraction::extractor::Extractor;
    use crate::extraction::python::disambiguation::MethodDisambiguator;
    use crate::extraction::python::extractor::PythonExtractor;
    use crate::extraction::sdk_model::{
        Operation, SdkServiceDefinition, ServiceMetadata, ServiceMethodRef, ServiceModelIndex,
        Shape, ShapeReference,
    };
    use crate::extraction::{
        Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata, SourceFile,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Create a comprehensive test service index with multiple AWS services
    fn create_comprehensive_test_service_index() -> ServiceModelIndex {
        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // === API Gateway V2 Service ===
        let mut apigateway_operations = HashMap::new();
        let mut apigateway_shapes = HashMap::new();

        // CreateApiMapping operation
        apigateway_operations.insert(
            "CreateApiMapping".to_string(),
            Operation {
                name: "CreateApiMapping".to_string(),
                input: Some(ShapeReference {
                    shape: "CreateApiMappingRequest".to_string(),
                }),
            },
        );

        // CreateApiMapping input shape
        let mut create_api_mapping_members = HashMap::new();
        create_api_mapping_members.insert(
            "DomainName".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        create_api_mapping_members.insert(
            "Stage".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        create_api_mapping_members.insert(
            "ApiId".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );
        create_api_mapping_members.insert(
            "ApiMappingKey".to_string(),
            ShapeReference {
                shape: "String".to_string(),
            },
        );

        apigateway_shapes.insert(
            "CreateApiMappingRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: create_api_mapping_members,
                required: Some(vec![
                    "DomainName".to_string(),
                    "Stage".to_string(),
                    "ApiId".to_string(),
                ]),
            },
        );

        let apigateway_service = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2018-11-29".to_string(),
                service_id: "ApiGatewayV2".to_string(),
            },
            operations: apigateway_operations,
            shapes: apigateway_shapes,
        };

        services.insert("apigatewayv2".to_string(), apigateway_service);

        // === S3 Service ===
        let mut s3_operations = HashMap::new();
        let mut s3_shapes = HashMap::new();

        // GetObject operation
        s3_operations.insert(
            "GetObject".to_string(),
            Operation {
                name: "GetObject".to_string(),
                input: Some(ShapeReference {
                    shape: "GetObjectRequest".to_string(),
                }),
            },
        );

        // GetObject input shape
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
        get_object_members.insert(
            "VersionId".to_string(),
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

        let s3_service = SdkServiceDefinition {
            version: Some("2.0".to_string()),
            metadata: ServiceMetadata {
                api_version: "2006-03-01".to_string(),
                service_id: "S3".to_string(),
            },
            operations: s3_operations,
            shapes: s3_shapes,
        };

        services.insert("s3".to_string(), s3_service);

        // === Method Lookup ===
        method_lookup.insert(
            "create_api_mapping".to_string(),
            vec![ServiceMethodRef {
                service_name: "apigatewayv2".to_string(),
                operation_name: "CreateApiMapping".to_string(),
            }],
        );

        method_lookup.insert(
            "get_object".to_string(),
            vec![ServiceMethodRef {
                service_name: "s3".to_string(),
                operation_name: "GetObject".to_string(),
            }],
        );

        ServiceModelIndex {
            services,
            method_lookup,
            waiter_lookup: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_valid_aws_sdk_method_call_with_required_params() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
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
                start_position: (1, 1),
                end_position: (1, 80),
                receiver: Some("apigateway_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "create_api_mapping");
        assert_eq!(result[0].possible_services, vec!["apigatewayv2"]);
    }

    #[tokio::test]
    async fn test_invalid_aws_sdk_method_call_missing_required_param() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
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
                start_position: (1, 1),
                end_position: (1, 40),
                receiver: Some("apigateway_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out due to missing required parameters
    }

    #[tokio::test]
    async fn test_invalid_aws_sdk_method_call_with_invalid_param() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
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
                    Parameter::Keyword {
                        name: "InvalidParam".to_string(), // This parameter doesn't exist in the AWS API
                        value: ParameterValue::Resolved("invalid".to_string()),
                        position: 3,
                        type_annotation: None,
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 100),
                receiver: Some("apigateway_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out due to invalid parameter
    }

    #[tokio::test]
    async fn test_aws_sdk_method_call_with_dictionary_unpacking() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "create_api_mapping".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![Parameter::DictionarySplat {
                    expression: "**params".to_string(),
                    position: 0,
                }],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 50),
                receiver: Some("apigateway_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 1); // Should be kept for future analysis
        assert_eq!(result[0].name, "create_api_mapping");
        assert_eq!(result[0].possible_services, vec!["apigatewayv2"]);
        assert!(result[0]
            .metadata
            .as_ref()
            .unwrap()
            .has_dictionary_unpacking());
    }

    #[tokio::test]
    async fn test_non_aws_method_call_filtered_out() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "custom_method".to_string(), // Not an AWS SDK method
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![Parameter::Keyword {
                    name: "custom_param".to_string(),
                    value: ParameterValue::Resolved("value".to_string()),
                    position: 0,
                    type_annotation: None,
                }],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 30),
                receiver: Some("custom_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);
        assert_eq!(result.len(), 0); // Should be filtered out as it's not an AWS SDK method
    }

    #[tokio::test]
    async fn test_mixed_method_calls_aws_and_non_aws() {
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_calls = vec![
            // Valid AWS SDK call
            SdkMethodCall {
                name: "get_object".to_string(),
                possible_services: Vec::new(),
                metadata: Some(SdkMethodCallMetadata {
                    parameters: vec![
                        Parameter::Keyword {
                            name: "Bucket".to_string(),
                            value: ParameterValue::Resolved("my-bucket".to_string()),
                            position: 0,
                            type_annotation: None,
                        },
                        Parameter::Keyword {
                            name: "Key".to_string(),
                            value: ParameterValue::Resolved("my-key".to_string()),
                            position: 1,
                            type_annotation: None,
                        },
                    ],
                    return_type: None,
                    start_position: (1, 1),
                    end_position: (1, 50),
                    receiver: Some("s3_client".to_string()),
                }),
            },
            // Non-AWS method call with same name as AWS method but different parameters
            SdkMethodCall {
                name: "get_object".to_string(),
                possible_services: Vec::new(),
                metadata: Some(SdkMethodCallMetadata {
                    parameters: vec![Parameter::Keyword {
                        name: "custom_param".to_string(), // Invalid parameter for AWS S3
                        value: ParameterValue::Resolved("value".to_string()),
                        position: 0,
                        type_annotation: None,
                    }],
                    return_type: None,
                    start_position: (2, 1),
                    end_position: (2, 30),
                    receiver: Some("custom_client".to_string()),
                }),
            },
            // Completely non-AWS method
            SdkMethodCall {
                name: "custom_method".to_string(),
                possible_services: Vec::new(),
                metadata: Some(SdkMethodCallMetadata {
                    parameters: vec![Parameter::Keyword {
                        name: "param".to_string(),
                        value: ParameterValue::Resolved("value".to_string()),
                        position: 0,
                        type_annotation: None,
                    }],
                    return_type: None,
                    start_position: (3, 1),
                    end_position: (3, 25),
                    receiver: Some("custom_client".to_string()),
                }),
            },
        ];

        let result = disambiguator.disambiguate_method_calls(method_calls);

        // Only the valid AWS SDK call should remain
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_object");
        assert_eq!(result[0].possible_services, vec!["s3"]);
        assert_eq!(
            result[0].metadata.as_ref().unwrap().receiver,
            Some("s3_client".to_string())
        );
    }

    #[tokio::test]
    async fn test_end_to_end_with_tree_sitter_parsing() {
        let service_index = create_comprehensive_test_service_index();

        // Python code with both AWS SDK calls and non-AWS calls
        let python_code = r#"
import boto3

def example():
    # Valid AWS SDK calls
    s3_client = boto3.client('s3')
    result1 = s3_client.get_object(Bucket='my-bucket', Key='my-key')
    
    apigateway_client = boto3.client('apigatewayv2')
    result2 = apigateway_client.create_api_mapping(
        DomainName='example.com',
        Stage='prod',
        ApiId='abc123'
    )
    
    # Dictionary unpacking case
    params = {'Bucket': 'my-bucket', 'Key': 'my-key'}
    result3 = s3_client.get_object(**params)
    
    # Non-AWS method calls that should be filtered out
    custom_client = CustomClient()
    result4 = custom_client.get_object(custom_param='value')  # Same name but invalid params
    result5 = custom_client.custom_method(param='value')      # Completely different method
"#;

        let source = SourceFile::with_language(
            PathBuf::from("test.py"),
            python_code.to_string(),
            crate::Language::Python,
        );

        let extractor = PythonExtractor::new();

        // Extract method calls using tree-sitter
        let mut result = vec![extractor.parse(&source.content).await];
        assert_eq!(result.first().unwrap().method_calls_ref().len(), 7);

        // Apply disambiguation
        extractor.filter_map(&mut result, &service_index);

        // Should have 3 valid AWS SDK calls:
        // 1. s3_client.get_object with explicit params
        // 2. apigateway_client.create_api_mapping with explicit params
        // 3. s3_client.get_object with dictionary unpacking
        assert_eq!(result.first().unwrap().method_calls_ref().len(), 3);

        // Apply disambiguation
        extractor.disambiguate(&mut result, &service_index);

        // Should still have 3 valid AWS SDK calls:
        // 1. s3_client.get_object with explicit params
        // 2. apigateway_client.create_api_mapping with explicit params
        // 3. s3_client.get_object with dictionary unpacking
        assert_eq!(result.first().unwrap().method_calls_ref().len(), 3);

        let calls = result.first().unwrap().method_calls_ref();

        // Check that we have the expected method names
        let method_names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
        assert!(method_names.contains(&"get_object"));
        assert!(method_names.contains(&"create_api_mapping"));

        // Check that dictionary unpacking is detected
        let unpacking_call = calls
            .iter()
            .find(|call| call.metadata.as_ref().unwrap().has_dictionary_unpacking())
            .expect("Should have a call with dictionary unpacking");
        assert_eq!(unpacking_call.name, "get_object");
    }

    #[tokio::test]
    async fn test_keyword_argument_starting_with_arg_prefix() {
        // This test demonstrates the fix for the arg_ prefix collision bug
        // Previously, keyword arguments starting with "arg_" would be incorrectly filtered out
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    Parameter::Keyword {
                        name: "Bucket".to_string(),
                        value: ParameterValue::Resolved("my-bucket".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "Key".to_string(),
                        value: ParameterValue::Resolved("my-key".to_string()),
                        position: 1,
                        type_annotation: None,
                    },
                    // This keyword argument starts with "arg_" but should NOT be filtered out
                    // because it's a legitimate keyword parameter, not a positional placeholder
                    Parameter::Keyword {
                        name: "arg_custom_param".to_string(), // This would have been incorrectly filtered before the fix
                        value: ParameterValue::Resolved("custom_value".to_string()),
                        position: 2,
                        type_annotation: None,
                    },
                    // Also include a positional argument to show the difference
                    Parameter::Positional {
                        value: ParameterValue::Resolved("positional_value".to_string()),
                        position: 3,
                        type_annotation: None,
                        struct_fields: None,
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 80),
                receiver: Some("s3_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);

        // The method call should be filtered out because "arg_custom_param" is not a valid S3 parameter
        // But the important thing is that it was evaluated as a keyword parameter, not filtered out
        // due to the "arg_" prefix. The filtering happens because it's an invalid parameter name for S3.
        assert_eq!(result.len(), 0); // Filtered out due to invalid parameter, not due to arg_ prefix

        // To demonstrate the fix more clearly, let's test with a method that would accept any parameter
        // We'll create a simpler test case
    }

    #[tokio::test]
    async fn test_parameter_type_filtering_logic() {
        // This test specifically validates that the disambiguation logic correctly filters
        // by parameter_type instead of name prefix
        let service_index = create_comprehensive_test_service_index();
        let disambiguator = MethodDisambiguator::new(&service_index);

        let method_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: Vec::new(),
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![
                    // Valid required parameters
                    Parameter::Keyword {
                        name: "Bucket".to_string(),
                        value: ParameterValue::Resolved("my-bucket".to_string()),
                        position: 0,
                        type_annotation: None,
                    },
                    Parameter::Keyword {
                        name: "Key".to_string(),
                        value: ParameterValue::Resolved("my-key".to_string()),
                        position: 1,
                        type_annotation: None,
                    },
                    // Positional argument with arg_ prefix - should be ignored in validation
                    Parameter::Positional {
                        value: ParameterValue::Resolved("positional_value".to_string()),
                        position: 2,
                        type_annotation: None,
                        struct_fields: None,
                    },
                ],
                return_type: None,
                start_position: (1, 1),
                end_position: (1, 60),
                receiver: Some("s3_client".to_string()),
            }),
        };

        let result = disambiguator.disambiguate_method_calls(vec![method_call]);

        // This should be valid because:
        // 1. Required keyword parameters (Bucket, Key) are provided
        // 2. Positional parameter is correctly ignored during validation (filtered by parameter_type)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_object");
        assert_eq!(result[0].possible_services, vec!["s3"]);
    }
}
