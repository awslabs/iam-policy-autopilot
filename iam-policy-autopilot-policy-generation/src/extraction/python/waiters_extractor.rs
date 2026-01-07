//! Waiter extraction for Python AWS SDK using ast-grep
//!
//! This module handles extraction of boto3 waiter patterns, which involve
//! two-phase operations: creating a waiter from a client, then calling wait()
//! on the waiter with operation arguments.

use crate::extraction::python::common::{ArgumentExtractor, ParameterFilter};
use crate::extraction::{Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata};
use crate::ServiceModelIndex;
use ast_grep_language::Python;

/// Information about a discovered get_waiter call
#[derive(Debug, Clone)]
pub(crate) struct WaiterInfo {
    /// Variable name assigned to the waiter (e.g., "waiter", "instance_waiter")
    pub variable_name: String,
    /// Waiter name from get_waiter argument in snake_case (e.g., "instance_terminated")
    pub waiter_name: String,
    /// Client receiver variable name (e.g., "client", "ec2_client")
    pub client_receiver: String,
    /// Line number where get_waiter was called
    pub get_waiter_line: usize,
}

// TODO: This should be refactored at a higher level, so this type can be removed.
// See https://github.com/awslabs/iam-policy-autopilot/issues/88.
enum CallInfo<'a> {
    None(&'a WaiterInfo),
    Simple(&'a WaiterInfo, &'a WaitCallInfo),
    Chained(&'a ChainedWaiterCallInfo),
}

impl<'a> CallInfo<'a> {
    fn waiter_name(&self) -> &'a str {
        match self {
            Self::None(waiter_info) | Self::Simple(waiter_info, ..) => &waiter_info.waiter_name,
            Self::Chained(waiter_call_info) => &waiter_call_info.waiter_name,
        }
    }
}

/// Information about a wait method call
#[derive(Debug, Clone)]
pub(crate) struct WaitCallInfo {
    /// Waiter variable being called (e.g., "waiter")
    pub waiter_var: String,
    /// Extracted arguments (including WaiterConfig)
    pub arguments: Vec<Parameter>,
    /// Line number where wait was called
    pub wait_line: usize,
    /// Start position of the wait call node
    pub start_position: (usize, usize),
    /// End position of the wait call node
    pub end_position: (usize, usize),
}

/// Information about a chained waiter call (client.get_waiter().wait())
#[derive(Debug, Clone)]
pub(crate) struct ChainedWaiterCallInfo {
    /// Client receiver variable name (e.g., "dynamodb_client")
    pub client_receiver: String,
    /// Waiter name from get_waiter argument (e.g., "table_exists")
    pub waiter_name: String,
    /// Extracted arguments from wait call (including WaiterConfig)
    pub arguments: Vec<Parameter>,
    /// Line number where chained call was made
    #[allow(dead_code)]
    pub line: usize,
    /// Start position of the chained call node
    pub start_position: (usize, usize),
    /// End position of the chained call node
    pub end_position: (usize, usize),
}

/// Extractor for boto3 waiter patterns
///
/// This extractor discovers waiter patterns in Python code and creates synthetic
/// SdkMethodCall objects that represent the actual AWS operations being polled.
///
/// Waiter patterns involve two calls:
/// 1. `waiter = client.get_waiter('instance_terminated')`
/// 2. `waiter.wait(InstanceIds=[...], WaiterConfig={...})`
///
/// The extractor matches these patterns and creates synthetic method calls using:
/// - Method name from service definition waiters (with language-specific conversion)
/// - Arguments from wait call
/// - Position information from the wait call
/// - Client receiver from get_waiter call
pub(crate) struct WaitersExtractor<'a> {
    service_index: &'a ServiceModelIndex,
}

impl<'a> WaitersExtractor<'a> {
    /// Create a new waiters extractor with a service model index
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        Self { service_index }
    }

    /// Extract waiter method calls from the AST
    ///
    /// This method discovers waiter patterns in boto3 code and creates synthetic
    /// SdkMethodCall objects that represent the actual AWS operations being polled.
    ///
    /// Handles three scenarios:
    /// 1. Matched waiter + wait: Creates one call with wait() arguments
    /// 2. Unmatched get_waiter: Creates calls with required params for all candidate services
    /// 3. Unmatched wait: Ignored (no waiter context)
    pub(crate) fn extract_waiter_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<SdkMethodCall> {
        // Step 1: Find all get_waiter calls
        let waiters = self.find_get_waiter_calls(ast);

        // Step 2: Find all wait calls
        let wait_calls = self.find_wait_calls(ast);

        // Step 3: Find all chained waiter calls (client.get_waiter().wait())
        let chained_calls = self.find_chained_waiter_calls(ast);

        // Step 4: Match wait calls to their waiters
        let mut synthetic_calls = Vec::new();
        let mut matched_waiter_indices = std::collections::HashSet::new();

        for wait_call in wait_calls {
            if let Some((waiter, waiter_idx)) = self.match_wait_to_waiter(&wait_call, &waiters) {
                // Create synthetic calls for matched waiter + wait (one per candidate service)
                let matched_calls = self.create_matched_synthetic_calls(&wait_call, waiter);
                synthetic_calls.extend(matched_calls);
                matched_waiter_indices.insert(waiter_idx);
            }
        }

        // Step 5: Handle chained waiter calls
        for chained_call in chained_calls {
            let chained_synthetic_calls = self.create_chained_synthetic_calls(&chained_call);
            synthetic_calls.extend(chained_synthetic_calls);
        }

        // Step 6: Handle unmatched get_waiter calls
        for (idx, waiter) in waiters.iter().enumerate() {
            if !matched_waiter_indices.contains(&idx) {
                // Create synthetic calls with required params for all candidate services
                let unmatched_calls = self.create_unmatched_synthetic_calls(waiter);
                synthetic_calls.extend(unmatched_calls);
            }
        }

        synthetic_calls
    }

    /// Find all get_waiter calls in the AST
    fn find_get_waiter_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<WaiterInfo> {
        let root = ast.root();
        let mut waiters = Vec::new();

        // Pattern: $WAITER = $CLIENT.get_waiter($NAME $$$ARGS)
        let get_waiter_pattern = "$WAITER = $CLIENT.get_waiter($NAME $$$ARGS)";

        for node_match in root.find_all(get_waiter_pattern) {
            if let Some(waiter_info) = self.parse_get_waiter_call(&node_match) {
                waiters.push(waiter_info);
            }
        }

        waiters
    }

    /// Find all wait calls in the AST
    fn find_wait_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<WaitCallInfo> {
        let root = ast.root();
        let mut wait_calls = Vec::new();

        // Pattern: $WAITER.wait($$$ARGS)
        // Note: We don't capture the result variable since wait() is typically not assigned
        let wait_pattern = "$WAITER.wait($$$ARGS)";

        for node_match in root.find_all(wait_pattern) {
            if let Some(wait_info) = self.parse_wait_call(&node_match) {
                wait_calls.push(wait_info);
            }
        }

        wait_calls
    }

    /// Find all chained waiter calls in the AST
    fn find_chained_waiter_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Vec<ChainedWaiterCallInfo> {
        let root = ast.root();
        let mut chained_calls = Vec::new();

        // Pattern: $CLIENT.get_waiter($NAME $$$WAITER_ARGS).wait($$$WAIT_ARGS)
        let chained_pattern = "$CLIENT.get_waiter($NAME $$$WAITER_ARGS).wait($$$WAIT_ARGS)";

        for node_match in root.find_all(chained_pattern) {
            if let Some(chained_info) = self.parse_chained_waiter_call(&node_match) {
                chained_calls.push(chained_info);
            }
        }

        chained_calls
    }

    /// Parse a get_waiter call into WaiterInfo
    fn parse_get_waiter_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<WaiterInfo> {
        let env = node_match.get_env();

        // Extract waiter variable name
        let variable_name = env.get_match("WAITER")?.text().to_string();

        // Extract client receiver name
        let client_receiver = env.get_match("CLIENT")?.text().to_string();

        // Extract waiter name (remove quotes and keep as-is from code, should be snake_case)
        let name_node = env.get_match("NAME")?;
        let name_text = name_node.text();
        let waiter_name = self.extract_quoted_string(&name_text)?;

        // Get line number
        let get_waiter_line = node_match.get_node().start_pos().line() + 1;

        Some(WaiterInfo {
            variable_name,
            waiter_name,
            client_receiver,
            get_waiter_line,
        })
    }

    /// Parse a wait call into WaitCallInfo
    fn parse_wait_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<WaitCallInfo> {
        let env = node_match.get_env();

        // Extract waiter variable name
        let waiter_var = env.get_match("WAITER")?.text().to_string();

        // Extract arguments (keep all, including WaiterConfig)
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        // Get position information from the wait call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(WaitCallInfo {
            waiter_var,
            arguments,
            wait_line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Parse a chained waiter call into ChainedWaiterCallInfo
    fn parse_chained_waiter_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> Option<ChainedWaiterCallInfo> {
        let env = node_match.get_env();

        // Extract client receiver name
        let client_receiver = env.get_match("CLIENT")?.text().to_string();

        // Extract waiter name (remove quotes and keep as-is from code, should be snake_case)
        let name_node = env.get_match("NAME")?;
        let name_text = name_node.text();
        let waiter_name = self.extract_quoted_string(&name_text)?;

        // Extract wait arguments (keep all, including WaiterConfig)
        let wait_args_nodes = env.get_multiple_matches("WAIT_ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&wait_args_nodes);

        // Get position information from the chained call node
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(ChainedWaiterCallInfo {
            client_receiver,
            waiter_name,
            arguments,
            line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Match a wait call to its corresponding get_waiter call
    fn match_wait_to_waiter<'b>(
        &self,
        wait_call: &WaitCallInfo,
        waiters: &'b [WaiterInfo],
    ) -> Option<(&'b WaiterInfo, usize)> {
        // Find waiter with matching variable name
        // Use the closest preceding waiter with the same name
        let mut best_match = None;
        let mut best_distance = usize::MAX;
        let mut best_idx = 0;

        for (idx, waiter) in waiters.iter().enumerate() {
            if waiter.variable_name == wait_call.waiter_var {
                // Only consider waiters that come before the wait call
                if waiter.get_waiter_line < wait_call.wait_line {
                    let distance = wait_call.wait_line - waiter.get_waiter_line;
                    if distance < best_distance {
                        best_distance = distance;
                        best_match = Some(waiter);
                        best_idx = idx;
                    }
                }
            }
        }

        best_match.map(|w| (w, best_idx))
    }

    /// Create synthetic SdkMethodCalls for a matched waiter + wait
    /// Creates one call per candidate service with the actual operation name
    fn create_synthetic_calls_internal(
        &self,
        wait_call: CallInfo,
        receiver: Option<String>,
    ) -> Vec<SdkMethodCall> {
        let mut synthetic_calls = Vec::new();

        // Get the operation for this service+waiter combination from service definition
        if let Some(service_defs) = self
            .service_index
            .waiter_lookup
            .get(wait_call.waiter_name())
        {
            for service_method in service_defs {
                let (parameters, start_position, end_position) = match wait_call {
                    CallInfo::Simple(_, wait_call) => {
                        // Filter out WaiterConfig from arguments - it's waiter-specific, not operation-specific
                        (
                            ParameterFilter::filter_waiter_parameters(wait_call.arguments.clone()),
                            wait_call.start_position,
                            wait_call.end_position,
                        )
                    }
                    CallInfo::Chained(chained_wait_call) => {
                        // Filter out WaiterConfig from arguments - it's waiter-specific, not operation-specific
                        (
                            ParameterFilter::filter_waiter_parameters(
                                chained_wait_call.arguments.clone(),
                            ),
                            // Use wait call position (most specific)
                            chained_wait_call.start_position,
                            chained_wait_call.end_position,
                        )
                    }
                    CallInfo::None(waiter_info) => {
                        let fallback_start_pos = (waiter_info.get_waiter_line, 1);
                        let fallback_end_pos = (waiter_info.get_waiter_line, 1);
                        let parameters = self.get_required_parameters(
                            &service_method.service_name,
                            &service_method.operation_name,
                            self.service_index,
                        );
                        (parameters, fallback_start_pos, fallback_end_pos)
                    }
                };

                // Create synthetic call with filtered wait() arguments
                synthetic_calls.push(SdkMethodCall {
                    name: wait_call.waiter_name().to_string(),
                    possible_services: vec![service_method.service_name.clone()],
                    metadata: Some(SdkMethodCallMetadata {
                        parameters,
                        return_type: None,
                        start_position,
                        end_position,
                        // Use client receiver from get_waiter call
                        receiver: receiver.clone(),
                    }),
                });
            }
        }

        synthetic_calls
    }

    fn create_matched_synthetic_calls(
        &self,
        wait_call: &WaitCallInfo,
        waiter_info: &WaiterInfo,
    ) -> Vec<SdkMethodCall> {
        self.create_synthetic_calls_internal(
            CallInfo::Simple(waiter_info, wait_call),
            Some(waiter_info.client_receiver.clone()),
        )
    }

    /// Create synthetic SdkMethodCalls for an unmatched get_waiter
    fn create_unmatched_synthetic_calls(&self, waiter_info: &WaiterInfo) -> Vec<SdkMethodCall> {
        self.create_synthetic_calls_internal(
            CallInfo::None(waiter_info),
            Some(waiter_info.client_receiver.clone()),
        )
    }

    /// Create synthetic SdkMethodCalls for a chained waiter call
    /// Creates one call per candidate service with the actual operation name
    fn create_chained_synthetic_calls(
        &self,
        chained_call: &ChainedWaiterCallInfo,
    ) -> Vec<SdkMethodCall> {
        self.create_synthetic_calls_internal(
            CallInfo::Chained(chained_call),
            Some(chained_call.client_receiver.clone()),
        )
    }

    /// Get required parameters for an operation from the service index
    fn get_required_parameters(
        &self,
        service_name: &str,
        operation_name: &str,
        service_index: &ServiceModelIndex,
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();

        // Look up the service and operation in the service index
        if let Some(service_def) = service_index.services.get(service_name) {
            if let Some(operation) = service_def.operations.get(operation_name) {
                // Get the input shape if it exists
                if let Some(input_ref) = &operation.input {
                    if let Some(input_shape) = service_def.shapes.get(&input_ref.shape) {
                        // Extract required parameters
                        if let Some(required_params) = &input_shape.required {
                            for (position, param_name) in required_params.iter().enumerate() {
                                parameters.push(Parameter::Keyword {
                                    name: param_name.clone(),
                                    value: ParameterValue::Unresolved("<unknown>".to_string()), // Placeholder for required param
                                    position,
                                    type_annotation: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        parameters
    }

    /// Extract a quoted string, handling both single and double quotes
    fn extract_quoted_string(&self, text: &str) -> Option<String> {
        ArgumentExtractor::extract_quoted_string(text)
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::sdk_model::ServiceMethodRef;

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
            Operation, SdkServiceDefinition, ServiceMetadata, Shape,
        };

        let mut services = HashMap::new();
        let mut operations = HashMap::new();
        let mut shapes = HashMap::new();
        let mut waiter_lookup = HashMap::new();

        // Create a mock DescribeInstances operation with required params
        let mut input_shape_members = HashMap::new();
        input_shape_members.insert(
            "InstanceIds".to_string(),
            crate::extraction::sdk_model::ShapeReference {
                shape: "InstanceIdStringList".to_string(),
            },
        );

        shapes.insert(
            "DescribeInstancesRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: input_shape_members,
                required: Some(vec!["InstanceIds".to_string()]),
            },
        );

        let describe_instances_op = Operation {
            name: "DescribeInstances".to_string(),
            input: Some(crate::extraction::sdk_model::ShapeReference {
                shape: "DescribeInstancesRequest".to_string(),
            }),
        };

        operations.insert(
            "DescribeInstances".to_string(),
            describe_instances_op.clone(),
        );

        waiter_lookup.insert(
            "instance_terminated".to_string(),
            vec![ServiceMethodRef {
                service_name: "ec2".to_string(),
                operation_name: "InstanceTerminated".to_string(),
            }],
        );

        // Create DynamoDB DescribeTables operation for table_exists waiter
        let mut describe_tables_members = HashMap::new();
        describe_tables_members.insert(
            "TableName".to_string(),
            crate::extraction::sdk_model::ShapeReference {
                shape: "String".to_string(),
            },
        );

        shapes.insert(
            "DescribeTablesRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: describe_tables_members,
                required: Some(vec!["TableName".to_string()]),
            },
        );

        let describe_tables_op = Operation {
            name: "DescribeTables".to_string(),
            input: Some(crate::extraction::sdk_model::ShapeReference {
                shape: "DescribeTablesRequest".to_string(),
            }),
        };

        operations.insert("DescribeTables".to_string(), describe_tables_op.clone());

        let mut dynamodb_operations = HashMap::new();
        let mut dynamodb_shapes = HashMap::new();
        let mut dynamodb_waiters = HashMap::new();

        dynamodb_operations.insert("DescribeTables".to_string(), describe_tables_op.clone());
        dynamodb_shapes.insert(
            "DescribeTablesRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: HashMap::from([(
                    "TableName".to_string(),
                    crate::extraction::sdk_model::ShapeReference {
                        shape: "String".to_string(),
                    },
                )]),
                required: Some(vec!["TableName".to_string()]),
            },
        );

        // Add TableExists waiter for DynamoDB
        dynamodb_waiters.insert("TableExists".to_string(), describe_tables_op);
        waiter_lookup.insert(
            "table_exists".to_string(),
            vec![ServiceMethodRef {
                service_name: "dynamodb".to_string(),
                operation_name: "TableExists".to_string(),
            }],
        );

        services.insert(
            "ec2".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2016-11-15".to_string(),
                    service_id: "EC2".to_string(),
                },
                operations,
                shapes,
            },
        );

        services.insert(
            "dynamodb".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2012-08-10".to_string(),
                    service_id: "DynamoDB".to_string(),
                },
                operations: dynamodb_operations,
                shapes: dynamodb_shapes,
            },
        );

        ServiceModelIndex {
            services,
            method_lookup: HashMap::new(),
            waiter_lookup,
        }
    }

    #[test]
    fn test_find_get_waiter_calls() {
        let source_code = r#"
import boto3
ec2_client = boto3.client('ec2')
waiter = ec2_client.get_waiter('instance_terminated')
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        let waiters = extractor.find_get_waiter_calls(&ast);

        assert_eq!(waiters.len(), 1);
        assert_eq!(waiters[0].variable_name, "waiter");
        assert_eq!(waiters[0].waiter_name, "instance_terminated");
        assert_eq!(waiters[0].client_receiver, "ec2_client");
        assert_eq!(waiters[0].get_waiter_line, 4);
    }

    #[test]
    fn test_find_wait_calls() {
        let source_code = r#"
waiter.wait(InstanceIds=['i-1234567890abcdef0'], WaiterConfig={'Delay': 15, 'MaxAttempts': 20})
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        let wait_calls = extractor.find_wait_calls(&ast);

        assert_eq!(wait_calls.len(), 1);
        assert_eq!(wait_calls[0].waiter_var, "waiter");
        assert_eq!(wait_calls[0].arguments.len(), 2); // InstanceIds + WaiterConfig
    }

    #[test]
    fn test_matched_waiter_and_wait() {
        let source_code = r#"
import boto3
ec2_client = boto3.client('ec2')
waiter = ec2_client.get_waiter('instance_terminated')
waiter.wait(InstanceIds=['i-1234567890abcdef0'])
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        let calls = extractor.extract_waiter_method_calls(&ast);

        // Should extract at least one call
        assert!(!calls.is_empty());
    }

    #[test]
    fn test_extract_quoted_string() {
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        assert_eq!(
            extractor.extract_quoted_string("'instance_terminated'"),
            Some("instance_terminated".to_string())
        );
        assert_eq!(
            extractor.extract_quoted_string("\"bucket_exists\""),
            Some("bucket_exists".to_string())
        );
        assert_eq!(
            extractor.extract_quoted_string(" 'waiter_name' "),
            Some("waiter_name".to_string())
        );
    }

    #[test]
    fn test_find_chained_waiter_calls() {
        let source_code = r#"
import boto3
dynamodb_client = boto3.client('dynamodb')
dynamodb_client.get_waiter('table_exists').wait(TableName=table_name)
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        let chained_calls = extractor.find_chained_waiter_calls(&ast);

        assert_eq!(chained_calls.len(), 1);
        assert_eq!(chained_calls[0].client_receiver, "dynamodb_client");
        assert_eq!(chained_calls[0].waiter_name, "table_exists");
        assert_eq!(chained_calls[0].arguments.len(), 1); // TableName
    }

    #[test]
    fn test_chained_waiter_extraction() {
        let source_code = r#"
import boto3
dynamodb_client = boto3.client('dynamodb')
dynamodb_client.get_waiter('table_exists').wait(TableName='test-table')
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = WaitersExtractor::new(&service_index);

        let calls = extractor.extract_waiter_method_calls(&ast);

        // Should extract at least one call for the chained waiter
        assert!(!calls.is_empty());
    }
}
