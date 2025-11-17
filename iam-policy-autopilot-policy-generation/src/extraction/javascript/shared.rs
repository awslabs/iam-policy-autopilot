//! Shared utilities for JavaScript and TypeScript AWS SDK extraction
//!
//! This module contains common functionality shared between JavaScript and TypeScript
//! extractors.

use std::collections::HashMap;
use crate::extraction::{Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata};
use crate::extraction::javascript::types::JavaScriptScanResults;

/// Shared extraction utilities for JavaScript/TypeScript AWS SDK method calls
pub(crate) struct ExtractionUtils;

impl ExtractionUtils {
    /// Extract operations from imported types and their usage patterns
    pub(crate) fn extract_operations_from_imports<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall> 
    where
        T: ast_grep_core::Doc + Clone,
    {
        let mut method_calls = Vec::new();

        // Extract operations from Command imports (e.g., PutObjectCommand -> PutObject operation)
        method_calls.extend(Self::extract_command_operations(scan_results, scanner));

        // Extract operations from paginate function imports (e.g., paginateQuery -> Query operation)  
        method_calls.extend(Self::extract_paginate_operations(scan_results, scanner));

        // Extract operations from waiter function imports (e.g., waitUntilBucketExists -> BucketExists waiter)
        method_calls.extend(Self::extract_waiter_operations(scan_results, scanner));

        // Extract operations from CommandInput imports (e.g., QueryCommandInput -> Query operation)
        method_calls.extend(Self::extract_command_input_operations(scan_results, scanner));

        method_calls
    }

    /// Extract operations from Command type imports and their constructor usage
    pub(crate) fn extract_command_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall> 
    where
        T: ast_grep_core::Doc + Clone,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find Command types
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) = Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary) else {
                    continue;
                };
                
                for import_info in &sublibrary_info.imports {
                    // Check if this is a Command type (ends with "Command")
                    if import_info.original_name.ends_with("Command") {
                        // Extract operation name by removing "Command" suffix
                        if let Some(operation_name) = import_info.original_name.strip_suffix("Command") {
                            // Try to find the actual constructor instantiation with arguments
                            // Use the local name for the search (handles renames)
                            let (usage_position, parameters) = scanner
                                .find_command_instantiation_with_args(&import_info.local_name)
                                .unwrap_or_else(|| (import_info.line, Vec::new())); // Fallback to import position with no params
                            
                            // Keep PascalCase operation name to match service index
                            // e.g., "CreateBucket" stays "CreateBucket"
                            let method_call = SdkMethodCall {
                                name: operation_name.to_string(),
                                possible_services: vec![service.clone()],
                                metadata: Some(SdkMethodCallMetadata {
                                    parameters, 
                                    return_type: None,
                                    start_position: (usage_position, 1),
                                    end_position: (usage_position, 1),
                                    receiver: None, // Commands are typically standalone
                                }),
                            };
                            operations.push(method_call);
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from paginate function imports
    pub(crate) fn extract_paginate_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall> 
    where
        T: ast_grep_core::Doc + Clone,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find paginate functions
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) = Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary) else {
                    continue;
                };
                
                for import_info in &sublibrary_info.imports {
                    // Check if this is a paginate function (starts with "paginate")
                    if import_info.original_name.starts_with("paginate") {
                        // Extract operation name by removing "paginate" prefix
                        if let Some(operation_name) = import_info.original_name.strip_prefix("paginate") {
                            // Try to find the actual paginate function call with arguments
                            // Use the local name for the search (handles renames)
                            let (usage_position, parameters) = scanner
                                .find_paginate_function_with_args(&import_info.local_name)
                                .unwrap_or_else(|| (import_info.line, Vec::new())); // Fallback to import position with no params
                            
                            // Keep PascalCase operation name to match service index
                            // e.g., "ListTables" stays "ListTables"
                            let method_call = SdkMethodCall {
                                name: operation_name.to_string(),
                                possible_services: vec![service.clone()],
                                metadata: Some(SdkMethodCallMetadata {
                                    parameters, // extracted from 2nd argument!
                                    return_type: None,
                                    start_position: (usage_position, 1),
                                    end_position: (usage_position, 1),
                                    receiver: None,
                                }),
                            };
                            operations.push(method_call);
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from waiter function imports
    /// Waiters like `waitUntilBucketExists` map to underlying operations like `HeadBucket`
    /// The waiter name is extracted here; actual operation resolution happens in filter_map
    pub(crate) fn extract_waiter_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall> 
    where
        T: ast_grep_core::Doc + Clone,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find waiter functions
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) = Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary) else {
                    continue;
                };
                
                for import_info in &sublibrary_info.imports {
                    // Check if this is a waiter function (starts with "waitUntil")
                    if import_info.original_name.starts_with("waitUntil") {
                        // Extract waiter name by removing "waitUntil" prefix
                        if let Some(waiter_name) = import_info.original_name.strip_prefix("waitUntil") {
                            // Try to find the actual waiter function call with arguments
                            // Use the local name for the search (handles renames)
                            let (usage_position, parameters) = scanner
                                .find_waiter_function_with_args(&import_info.local_name)
                                .unwrap_or_else(|| (import_info.line, Vec::new())); // Fallback to import position with no params
                            
                            // Keep PascalCase waiter name
                            // e.g., "BucketExists" from "waitUntilBucketExists"
                            // This will be resolved to the actual operation (e.g., "HeadBucket") in filter_map
                            let method_call = SdkMethodCall {
                                name: waiter_name.to_string(),
                                possible_services: vec![service.clone()],
                                metadata: Some(SdkMethodCallMetadata {
                                    parameters, // Extracted from 2nd argument (operation params)
                                    return_type: None,
                                    start_position: (usage_position, 1),
                                    end_position: (usage_position, 1), 
                                    receiver: None, // Waiter functions are standalone
                                }),
                            };
                            operations.push(method_call);
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from CommandInput type imports
    pub(crate) fn extract_command_input_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall> 
    where
        T: ast_grep_core::Doc + Clone,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find CommandInput types
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) = Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary) else {
                    continue;
                };
                
                for import_info in &sublibrary_info.imports {
                    // Check if this is a CommandInput or Input type
                    let operation_name = if import_info.original_name.ends_with("CommandInput") {
                        // Extract operation name by removing "CommandInput" suffix
                        import_info.original_name.strip_suffix("CommandInput")
                    } else {
                        None
                    };

                    if let Some(operation_name) = operation_name {
                        // Try to find the actual CommandInput type usage position (TypeScript-specific)
                        // Use the local name for the search (handles renames)
                        let usage_position = scanner
                            .find_command_input_usage_position(&import_info.local_name)
                            .unwrap_or(import_info.line); // Fallback to import position
                        
                        // Keep PascalCase operation name to match service index
                        // e.g., "Query" stays "Query"
                        let method_call = SdkMethodCall {
                            name: operation_name.to_string(),
                            possible_services: vec![service.clone()],
                            metadata: Some(SdkMethodCallMetadata {
                                parameters: Vec::new(), // TODO: Extract from variable assignments
                                return_type: None,
                                start_position: (usage_position, 1), // Using enhanced position tracking
                                end_position: (usage_position, 1),   // Using enhanced position tracking
                                receiver: None,
                            }),
                        };
                        operations.push(method_call);
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from direct client method calls (e.g., client.getObject())
    pub(crate) fn extract_operations_from_method_calls(
        scan_results: &JavaScriptScanResults,
    ) -> Vec<SdkMethodCall> {
        let mut operations = Vec::new();

        // Process method calls to find direct operations on clients
        for method_call in &scan_results.method_calls {
            // Skip send method calls (handled separately)
            if method_call.method_name == "send" {
                continue;
            }

            // Skip method calls from sublibraries that don't match known patterns
            let Some(service) = Self::extract_service_from_sublibrary(&method_call.client_sublibrary) else {
                continue;
            };

            // Convert camelCase to PascalCase to match service index
            // e.g., "getObject" -> "GetObject"
            let operation_name = Self::camel_case_to_pascal_case(&method_call.method_name);

            // Convert method arguments to parameters
            let parameters = Self::convert_arguments_to_parameters(&method_call.arguments);

            let sdk_method_call = SdkMethodCall {
                name: operation_name,
                possible_services: vec![service],
                metadata: Some(SdkMethodCallMetadata {
                    parameters,
                    return_type: None,
                    start_position: (method_call.line, 1),
                    end_position: (method_call.line, 1),
                    receiver: Some(method_call.client_variable.clone()),
                }),
            };
            
            operations.push(sdk_method_call);
        }

        operations
    }

    /// Convert camelCase to PascalCase for method names
    /// e.g., "getObject" -> "GetObject", "listTables" -> "ListTables"
    pub(crate) fn camel_case_to_pascal_case(input: &str) -> String {
        if input.is_empty() {
            return input.to_string();
        }
        
        let mut chars = input.chars();
        if let Some(first_char) = chars.next() {
            first_char.to_uppercase().collect::<String>() + chars.as_str()
        } else {
            input.to_string()
        }
    }

    /// Extract service name from sublibrary name
    /// Returns Some(service) if the sublibrary matches a known pattern, None otherwise
    pub(crate) fn extract_service_from_sublibrary(sublibrary: &str) -> Option<String> {
        // Handle common patterns:
        // "client-s3" -> Some("s3")
        // "lib-dynamodb" -> Some("dynamodb")
        // "client-lambda" -> Some("lambda")
        if let Some(service) = sublibrary.strip_prefix("client-") {
            Some(service.to_string())
        } else { sublibrary.strip_prefix("lib-").map(|service| service.to_string()) }
    }

    /// Convert argument HashMap to Parameter vector
    pub(crate) fn convert_arguments_to_parameters(
        arguments: &HashMap<String, String>,
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();

        // Convert each argument to a keyword parameter
        for (position, (name, value)) in arguments.iter().enumerate() {
            parameters.push(Parameter::Keyword {
                name: name.clone(),
                value: ParameterValue::Unresolved(value.clone()), // JavaScript values are typically unresolved
                position,
                type_annotation: None, // We don't infer types for JavaScript/TypeScript parameters for now
            });
        }

        parameters
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_service_from_sublibrary() {
        // Test successful pattern matching (Some cases)
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("client-s3"), Some("s3".to_string()));
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("lib-dynamodb"), Some("dynamodb".to_string()));
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("client-lambda"), Some("lambda".to_string()));
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("client-ec2"), Some("ec2".to_string()));
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("lib-storage"), Some("storage".to_string()));
        
        // Test unsuccessful pattern matching (None cases)
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("other"), None);
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("unknown-prefix-service"), None);
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary("service-s3"), None);
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary(""), None);
    }

    #[test]
    fn test_convert_arguments_to_parameters() {
        let mut arguments = HashMap::new();
        arguments.insert("Bucket".to_string(), "my-bucket".to_string());
        arguments.insert("Key".to_string(), "my-key".to_string());

        let parameters = ExtractionUtils::convert_arguments_to_parameters(&arguments);
        assert_eq!(parameters.len(), 2);

        // Check that both parameters are keyword parameters
        for param in &parameters {
            match param {
                Parameter::Keyword {
                    name,
                    value,
                    position,
                    ..
                } => {
                    assert!(name == "Bucket" || name == "Key");
                    assert!(value.as_string() == "my-bucket" || value.as_string() == "my-key");
                    assert!(*position < 2);
                }
                _ => panic!("Expected keyword parameter"),
            }
        }
    }

    #[test]
    fn test_camel_case_to_pascal_case() {
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case("getObject"), "GetObject");
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case("listTables"), "ListTables");
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case("createBucket"), "CreateBucket");
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case("query"), "Query");
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case(""), "");
    }
}
