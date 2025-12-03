//! Paginator method extraction for Python AWS SDK using ast-grep
//!
//! This module handles extraction of boto3 paginate patterns, which involve
//! two-phase operations: creating a paginator from a client, then executing
//! the paginator with operation arguments.

use crate::extraction::python::common::{ArgumentExtractor, ParameterFilter};
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::{Parameter, SdkMethodCall, SdkMethodCallMetadata};
use crate::Language;
use crate::ServiceModelIndex;
use ast_grep_language::Python;

/// Information about a discovered get_paginator call
#[derive(Debug, Clone)]
pub(crate) struct PaginatorInfo {
    /// Variable name assigned to the paginator (e.g., "paginator", "s3_paginator")
    pub variable_name: String,
    /// Operation name from get_paginator argument (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Client receiver variable name (e.g., "client", "s3_client")
    pub client_receiver: String,
    /// Line number where get_paginator was called
    pub get_paginator_line: usize,
}

/// Information about a paginate method call
#[derive(Debug, Clone)]
pub(crate) struct PaginateCallInfo {
    /// Paginator variable being called (e.g., "paginator")
    pub paginator_var: String,
    /// Extracted arguments (excluding pagination-specific ones)
    pub arguments: Vec<Parameter>,
    /// Line number where paginate was called (preferred for position reporting)
    pub paginate_line: usize,
    /// Start position of the paginate call node
    pub start_position: (usize, usize),
    /// End position of the paginate call node  
    pub end_position: (usize, usize),
}

/// Information about a chained paginator call (client.get_paginator().paginate())
#[derive(Debug, Clone)]
pub(crate) struct ChainedPaginatorCallInfo {
    /// Client receiver variable name (e.g., "s3_client")
    pub client_receiver: String,
    /// Operation name from get_paginator argument (e.g., "list_objects_v2")
    pub operation_name: String,
    /// Extracted arguments from paginate call (excluding pagination-specific ones)
    pub arguments: Vec<Parameter>,
    /// Line number where chained call was made
    #[allow(dead_code)]
    pub line: usize,
    /// Start position of the chained call node
    pub start_position: (usize, usize),
    /// End position of the chained call node
    pub end_position: (usize, usize),
}

/// Extractor for boto3 paginate method patterns
///
/// This extractor discovers paginate patterns in Python code and creates synthetic
/// SdkMethodCall objects that represent the actual AWS operations being paginated.
///
/// Paginate patterns involve two calls:
/// 1. `paginator = client.get_paginator('operation_name')`
/// 2. `page_iterator = paginator.paginate(Bucket='...', PaginationConfig=...)`
///
/// The extractor matches these patterns and creates synthetic method calls using:
/// - Method name from get_paginator argument
/// - Arguments from paginate call (filtered to remove pagination-specific params)
/// - Position information from the paginate call (most specific)
/// - Client receiver from get_paginator call
pub(crate) struct PaginatorExtractor<'a> {
    service_index: &'a ServiceModelIndex,
}

impl<'a> PaginatorExtractor<'a> {
    /// Create a new paginator extractor with a service model index
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        Self { service_index }
    }

    /// Convert paginator operation name to method name using ServiceDiscovery
    /// This ensures consistency with the method lookup index used in disambiguation
    fn convert_paginator_operation_to_method_name(&self, operation_name: &str) -> String {
        // Use ServiceDiscovery to convert operation name to Python method name
        // This should match the conversion used in the method lookup index
        ServiceDiscovery::operation_to_method_name(operation_name, Language::Python)
    }

    /// Extract paginate method calls from the AST
    ///
    /// This method discovers paginate patterns in boto3 code and creates synthetic
    /// SdkMethodCall objects that represent the actual AWS operations being paginated.
    ///
    /// Supports multiple paginate calls on the same paginator - each creates a separate
    /// synthetic method call with its own arguments and position information.
    ///
    /// Also handles unmatched get_paginator calls by creating synthetic calls with
    /// empty parameters, since paginators are often created but used elsewhere.
    pub(crate) fn extract_paginate_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<SdkMethodCall> {
        // Step 1: Find all get_paginator calls
        let paginators = self.find_get_paginator_calls(ast);

        // Step 2: Find all paginate calls
        let paginate_calls = self.find_paginate_calls(ast);

        // Step 3: Find all chained paginator calls (client.get_paginator().paginate())
        let chained_calls = self.find_chained_paginator_calls(ast);

        // Step 4: Match paginate calls to their paginators and create synthetic method calls
        let mut synthetic_calls = Vec::new();
        let mut matched_paginator_indices = std::collections::HashSet::new();

        for paginate_call in paginate_calls {
            if let Some((paginator, paginator_idx)) =
                self.match_paginate_to_paginator_with_index(&paginate_call, &paginators)
            {
                synthetic_calls.push(self.create_synthetic_method_call(&paginate_call, paginator));
                matched_paginator_indices.insert(paginator_idx);
            }
        }

        // Step 5: Handle chained paginator calls
        for chained_call in chained_calls {
            synthetic_calls.push(self.create_chained_synthetic_method_call(&chained_call));
        }

        // Step 6: Handle unmatched get_paginator calls by creating synthetic calls with empty parameters
        for (idx, paginator) in paginators.iter().enumerate() {
            if !matched_paginator_indices.contains(&idx) {
                synthetic_calls.push(self.create_fallback_synthetic_method_call(paginator));
            }
        }

        synthetic_calls
    }

    /// Find all get_paginator calls in the AST
    fn find_get_paginator_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<PaginatorInfo> {
        let root = ast.root();
        let mut paginators = Vec::new();

        // Pattern: $PAGINATOR = $CLIENT.get_paginator($OPERATION $$$ARGS)
        let get_paginator_pattern = "$PAGINATOR = $CLIENT.get_paginator($OPERATION $$$ARGS)";

        for node_match in root.find_all(get_paginator_pattern) {
            if let Some(paginator_info) = self.parse_get_paginator_call(&node_match) {
                paginators.push(paginator_info);
            }
        }

        paginators
    }

    /// Find all paginate calls in the AST
    fn find_paginate_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<PaginateCallInfo> {
        let root = ast.root();
        let mut paginate_calls = Vec::new();

        // Pattern: $PAGINATOR.paginate($$$ARGS) - flexible pattern without assignment requirement
        let paginate_pattern = "$PAGINATOR.paginate($$$ARGS)";

        for node_match in root.find_all(paginate_pattern) {
            if let Some(paginate_info) = self.parse_paginate_call(&node_match) {
                paginate_calls.push(paginate_info);
            }
        }

        paginate_calls
    }

    /// Find all chained paginator calls in the AST
    fn find_chained_paginator_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<ChainedPaginatorCallInfo> {
        let root = ast.root();
        let mut chained_calls = Vec::new();

        // Pattern: $CLIENT.get_paginator($OPERATION $$$GET_ARGS).paginate($$$PAGINATE_ARGS)
        let chained_pattern =
            "$CLIENT.get_paginator($OPERATION $$$GET_ARGS).paginate($$$PAGINATE_ARGS)";

        for node_match in root.find_all(chained_pattern) {
            if let Some(chained_info) = self.parse_chained_paginator_call(&node_match) {
                chained_calls.push(chained_info);
            }
        }

        chained_calls
    }

    /// Parse a get_paginator call into PaginatorInfo
    fn parse_get_paginator_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<PaginatorInfo> {
        let env = node_match.get_env();

        // Extract paginator variable name
        let variable_name = env.get_match("PAGINATOR")?.text().to_string();

        // Extract client receiver name
        let client_receiver = env.get_match("CLIENT")?.text().to_string();

        // Extract operation name (remove quotes)
        let operation_node = env.get_match("OPERATION")?;
        let operation_text = operation_node.text();
        let operation_name = self.extract_quoted_string(&operation_text)?;

        // Get line number
        let get_paginator_line = node_match.get_node().start_pos().line() + 1;

        Some(PaginatorInfo {
            variable_name,
            operation_name,
            client_receiver,
            get_paginator_line,
        })
    }

    /// Parse a paginate call into PaginateCallInfo
    fn parse_paginate_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<PaginateCallInfo> {
        let env = node_match.get_env();

        // Extract paginator variable name
        let paginator_var = env.get_match("PAGINATOR")?.text().to_string();

        // Extract arguments and filter out pagination-specific ones
        let args_nodes = env.get_multiple_matches("ARGS");
        let all_arguments = self.extract_arguments(&args_nodes);
        let filtered_arguments = self.filter_pagination_parameters(all_arguments);

        // Get position information from the paginate call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(PaginateCallInfo {
            paginator_var,
            arguments: filtered_arguments,
            paginate_line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Parse a chained paginator call into ChainedPaginatorCallInfo
    fn parse_chained_paginator_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<ChainedPaginatorCallInfo> {
        let env = node_match.get_env();

        // Extract client receiver name
        let client_receiver = env.get_match("CLIENT")?.text().to_string();

        // Extract operation name (remove quotes)
        let operation_node = env.get_match("OPERATION")?;
        let operation_text = operation_node.text();
        let operation_name = self.extract_quoted_string(&operation_text)?;

        // Extract paginate arguments and filter out pagination-specific ones
        let paginate_args_nodes = env.get_multiple_matches("PAGINATE_ARGS");
        let all_arguments = self.extract_arguments(&paginate_args_nodes);
        let filtered_arguments = self.filter_pagination_parameters(all_arguments);

        // Get position information from the chained call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(ChainedPaginatorCallInfo {
            client_receiver,
            operation_name,
            arguments: filtered_arguments,
            line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Extract arguments from argument nodes
    fn extract_arguments(
        &self,
        args_nodes: &[ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>],
    ) -> Vec<Parameter> {
        ArgumentExtractor::extract_arguments(args_nodes)
    }

    /// Match a paginate call to its corresponding get_paginator call, returning both paginator and index
    fn match_paginate_to_paginator_with_index<'b>(
        &self,
        paginate_call: &PaginateCallInfo,
        paginators: &'b [PaginatorInfo],
    ) -> Option<(&'b PaginatorInfo, usize)> {
        // Find paginator with matching variable name
        // Conservative approach: use the closest preceding paginator with the same name
        let mut best_match = None;
        let mut best_distance = usize::MAX;
        let mut best_idx = 0;

        for (idx, paginator) in paginators.iter().enumerate() {
            if paginator.variable_name == paginate_call.paginator_var {
                // Only consider paginators that come before the paginate call
                if paginator.get_paginator_line < paginate_call.paginate_line {
                    let distance = paginate_call.paginate_line - paginator.get_paginator_line;
                    if distance < best_distance {
                        best_distance = distance;
                        best_match = Some(paginator);
                        best_idx = idx;
                    }
                }
            }
        }

        best_match.map(|p| (p, best_idx))
    }

    /// Create a fallback synthetic SdkMethodCall for unmatched get_paginator calls
    ///
    /// This handles cases where get_paginator is found but no matching paginate call exists.
    /// The synthetic call uses the operation name from get_paginator with empty parameters.
    fn create_fallback_synthetic_method_call(
        &self,
        paginator_info: &PaginatorInfo,
    ) -> SdkMethodCall {
        // Convert paginator operation name to match method lookup index format
        let method_name =
            self.convert_paginator_operation_to_method_name(&paginator_info.operation_name);

        // Look up all services that provide this method
        let possible_services =
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_name) {
                service_refs
                    .iter()
                    .map(|service_ref| service_ref.service_name.clone())
                    .collect()
            } else {
                Vec::new() // No services found for this method
            };

        SdkMethodCall {
            name: method_name,
            possible_services,
            metadata: Some(SdkMethodCallMetadata {
                parameters: Vec::new(), // Empty parameters for unmatched paginators
                return_type: None,
                // Use get_paginator call position
                start_position: (paginator_info.get_paginator_line, 1),
                end_position: (paginator_info.get_paginator_line, 1),
                receiver: Some(paginator_info.client_receiver.clone()),
            }),
        }
    }

    /// Create a synthetic SdkMethodCall from a matched paginate pattern
    fn create_synthetic_method_call(
        &self,
        paginate_call: &PaginateCallInfo,
        paginator_info: &PaginatorInfo,
    ) -> SdkMethodCall {
        // Convert paginator operation name to match method lookup index format
        let method_name =
            self.convert_paginator_operation_to_method_name(&paginator_info.operation_name);

        // Look up all services that provide this method
        let possible_services =
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_name) {
                service_refs
                    .iter()
                    .map(|service_ref| service_ref.service_name.clone())
                    .collect()
            } else {
                Vec::new() // No services found for this method
            };

        SdkMethodCall {
            name: method_name,
            possible_services,
            metadata: Some(SdkMethodCallMetadata {
                parameters: paginate_call.arguments.clone(),
                return_type: None,
                // Use paginate call position (most specific)
                start_position: paginate_call.start_position,
                end_position: paginate_call.end_position,
                // Use client receiver from get_paginator call
                receiver: Some(paginator_info.client_receiver.clone()),
            }),
        }
    }

    /// Create a synthetic SdkMethodCall from a chained paginator call
    fn create_chained_synthetic_method_call(
        &self,
        chained_call: &ChainedPaginatorCallInfo,
    ) -> SdkMethodCall {
        // Convert paginator operation name to match method lookup index format
        let method_name =
            self.convert_paginator_operation_to_method_name(&chained_call.operation_name);

        // Look up all services that provide this method
        let possible_services =
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_name) {
                service_refs
                    .iter()
                    .map(|service_ref| service_ref.service_name.clone())
                    .collect()
            } else {
                Vec::new() // No services found for this method
            };

        SdkMethodCall {
            name: method_name,
            possible_services,
            metadata: Some(SdkMethodCallMetadata {
                parameters: chained_call.arguments.clone(),
                return_type: None,
                // Use chained call position
                start_position: chained_call.start_position,
                end_position: chained_call.end_position,
                // Use client receiver from chained call
                receiver: Some(chained_call.client_receiver.clone()),
            }),
        }
    }

    /// Filter out pagination-specific parameters
    fn filter_pagination_parameters(&self, parameters: Vec<Parameter>) -> Vec<Parameter> {
        ParameterFilter::filter_pagination_parameters(parameters)
    }

    /// Extract a quoted string, handling both single and double quotes
    fn extract_quoted_string(&self, text: &str) -> Option<String> {
        ArgumentExtractor::extract_quoted_string(text)
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::ParameterValue;

    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::Python;
    use std::collections::HashMap;

    fn create_test_ast(
        source_code: &str,
    ) -> ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>> {
        Python.ast_grep(source_code)
    }

    fn create_test_service_index() -> ServiceModelIndex {
        use crate::extraction::sdk_model::{
            Operation, SdkServiceDefinition, ServiceMetadata, ServiceMethodRef, Shape,
        };

        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // Create S3 service with ListObjectsV2 operation
        let mut s3_operations = HashMap::new();
        let mut s3_shapes = HashMap::new();

        // Create ListObjectsV2 operation
        s3_operations.insert(
            "ListObjectsV2".to_string(),
            Operation {
                name: "ListObjectsV2".to_string(),
                input: Some(crate::extraction::sdk_model::ShapeReference {
                    shape: "ListObjectsV2Request".to_string(),
                }),
            },
        );

        // Create input shape for ListObjectsV2
        let mut list_objects_members = HashMap::new();
        list_objects_members.insert(
            "Bucket".to_string(),
            crate::extraction::sdk_model::ShapeReference {
                shape: "String".to_string(),
            },
        );
        list_objects_members.insert(
            "Prefix".to_string(),
            crate::extraction::sdk_model::ShapeReference {
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

        services.insert(
            "s3".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2006-03-01".to_string(),
                    service_id: "S3".to_string(),
                },
                operations: s3_operations,
                shapes: s3_shapes,
            },
        );

        // Add method lookup entry for list_objects_v2
        method_lookup.insert(
            "list_objects_v2".to_string(),
            vec![ServiceMethodRef {
                service_name: "s3".to_string(),
                operation_name: "ListObjectsV2".to_string(),
            }],
        );

        ServiceModelIndex {
            services,
            method_lookup,
            waiter_lookup: HashMap::new(),
        }
    }

    #[test]
    fn test_extract_paginate_multiple_calls_same_paginator() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        // Test source with multiple paginate calls on same paginator
        let source_code = r#"
import boto3
client = boto3.client('s3')
paginator = client.get_paginator('list_objects_v2')

page_iterator1 = paginator.paginate(Bucket='bucket1')
page_iterator2 = paginator.paginate(Bucket='bucket2') 
page_iterator3 = paginator.paginate(Bucket='bucket3', Prefix='test/')
"#;

        let ast = create_test_ast(source_code);
        let paginate_calls = extractor.extract_paginate_method_calls(&ast);

        // Should extract all 3 paginate calls as separate synthetic method calls
        assert_eq!(paginate_calls.len(), 3);

        // All should have the same method name from get_paginator
        for call in &paginate_calls {
            assert_eq!(call.name, "list_objects_v2");
            assert_eq!(
                call.metadata.as_ref().unwrap().receiver,
                Some("client".to_string())
            );
        }

        // Should have different arguments
        let bucket_values: Vec<String> = paginate_calls
            .iter()
            .filter_map(|call| {
                call.metadata.as_ref().and_then(|meta| {
                    meta.parameters.iter().find_map(|param| {
                        if let Parameter::Keyword { name, value, .. } = param {
                            if name == "Bucket" {
                                Some(value.as_string().to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                })
            })
            .collect();

        assert_eq!(bucket_values.len(), 3);
        assert!(bucket_values.contains(&"bucket1".to_string()));
        assert!(bucket_values.contains(&"bucket2".to_string()));
        assert!(bucket_values.contains(&"bucket3".to_string()));
    }

    #[test]
    fn test_filter_pagination_parameters() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        let parameters = vec![
            Parameter::Keyword {
                name: "Bucket".to_string(),
                value: ParameterValue::Resolved("my-bucket".to_string()),
                position: 0,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "PaginationConfig".to_string(),
                value: ParameterValue::Unresolved("{'MaxItems': 10}".to_string()),
                position: 1,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "Prefix".to_string(),
                value: ParameterValue::Resolved("test/".to_string()),
                position: 2,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "StartingToken".to_string(),
                value: ParameterValue::Unresolved("token".to_string()),
                position: 3,
                type_annotation: None,
            },
        ];

        let filtered = extractor.filter_pagination_parameters(parameters);

        // Should keep Bucket and Prefix, filter out PaginationConfig and StartingToken
        assert_eq!(filtered.len(), 2);

        let param_names: Vec<String> = filtered
            .iter()
            .filter_map(|param| {
                if let Parameter::Keyword { name, .. } = param {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(param_names.contains(&"Bucket".to_string()));
        assert!(param_names.contains(&"Prefix".to_string()));
        assert!(!param_names.contains(&"PaginationConfig".to_string()));
        assert!(!param_names.contains(&"StartingToken".to_string()));
    }

    #[test]
    fn test_extract_quoted_string() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        // Test different quote styles
        assert_eq!(
            extractor.extract_quoted_string("'list_objects_v2'"),
            Some("list_objects_v2".to_string())
        );
        assert_eq!(
            extractor.extract_quoted_string("\"list_objects_v2\""),
            Some("list_objects_v2".to_string())
        );
        assert_eq!(
            extractor.extract_quoted_string(" 'list_objects_v2' "),
            Some("list_objects_v2".to_string())
        );

        // Test unquoted (edge case)
        assert_eq!(
            extractor.extract_quoted_string("operation_name"),
            Some("operation_name".to_string())
        );
    }

    #[test]
    fn test_paginate_call_position_tracking() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        // Test that position information is correctly extracted from paginate calls
        let source_code = r#"
import boto3
client = boto3.client('s3')
paginator = client.get_paginator('list_objects_v2')
page_iterator = paginator.paginate(Bucket='test-bucket')
"#;

        let ast = create_test_ast(source_code);
        let paginate_calls = extractor.extract_paginate_method_calls(&ast);

        assert_eq!(paginate_calls.len(), 1);

        let call = &paginate_calls[0];
        assert_eq!(call.name, "list_objects_v2");

        // Position should be from the paginate call (line 5), not get_paginator call (line 4)
        assert_eq!(call.metadata.as_ref().unwrap().start_position.0, 5);
        assert_eq!(
            call.metadata.as_ref().unwrap().receiver,
            Some("client".to_string())
        );
    }

    #[test]
    fn test_no_paginate_calls_without_paginator() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        // Test source with paginate call but no matching get_paginator
        let source_code = r#"
# No get_paginator call
page_iterator = some_paginator.paginate(Bucket='test-bucket')
"#;

        let ast = create_test_ast(source_code);
        let paginate_calls = extractor.extract_paginate_method_calls(&ast);

        // Should not extract any calls since there's no matching get_paginator
        assert_eq!(paginate_calls.len(), 0);
    }

    #[test]
    fn test_paginate_with_pagination_config_filtered() {
        let service_index = create_test_service_index();
        let extractor = PaginatorExtractor::new(&service_index);

        // Test source with PaginationConfig that should be filtered out
        let source_code = r#"
import boto3
client = boto3.client('s3')
paginator = client.get_paginator('list_objects_v2')
page_iterator = paginator.paginate(
    Bucket='test-bucket',
    Prefix='logs/',
    PaginationConfig={'MaxItems': 10, 'PageSize': 5}
)
"#;

        let ast = create_test_ast(source_code);
        let paginate_calls = extractor.extract_paginate_method_calls(&ast);

        assert_eq!(paginate_calls.len(), 1);

        let call = &paginate_calls[0];
        let param_names: Vec<String> = call
            .metadata
            .as_ref()
            .unwrap()
            .parameters
            .iter()
            .filter_map(|param| {
                if let Parameter::Keyword { name, .. } = param {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Should include Bucket and Prefix, but not PaginationConfig
        assert!(param_names.contains(&"Bucket".to_string()));
        assert!(param_names.contains(&"Prefix".to_string()));
        assert!(!param_names.contains(&"PaginationConfig".to_string()));
    }
}
