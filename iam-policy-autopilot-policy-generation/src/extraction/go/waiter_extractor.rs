//! Waiter extraction for Go AWS SDK using ast-grep
//!
//! This module handles extraction of Go AWS SDK waiter patterns, which involve
//! creating a waiter from a client, then calling Wait() on the waiter.

use std::path::Path;

use crate::extraction::go::utils;
use crate::extraction::shared::extraction_utils::{
    WaiterCallInfo, WaiterCallPattern, WaiterCreationInfo,
};
use crate::extraction::{AstWithSourceFile, Parameter, ParameterValue, SdkMethodCall};
use crate::{Location, ServiceModelIndex};
use ast_grep_language::Go;

/// Extractor for Go AWS SDK waiter patterns
///
/// This extractor discovers waiter patterns in Go code and creates synthetic
/// SdkMethodCall objects that represent the actual AWS operations being polled.
///
/// Go waiter patterns involve:
/// 1. `waiter := client.NewInstanceTerminatedWaiter()`
/// 2. `err := waiter.Wait(ctx, &ec2.DescribeInstancesInput{...})`
pub(crate) struct GoWaiterExtractor<'a> {
    service_index: &'a ServiceModelIndex,
}

impl<'a> GoWaiterExtractor<'a> {
    /// Create a new Go waiter extractor
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        Self { service_index }
    }

    /// Extract waiter method calls from the AST
    pub(crate) fn extract_waiter_method_calls(
        &self,
        ast: &AstWithSourceFile<Go>,
    ) -> Vec<SdkMethodCall> {
        // Step 1: Find all waiter creation calls
        let waiters = self.find_waiter_creation_calls(ast);

        // Step 2: Find all Wait calls
        let wait_calls = self.find_wait_calls(ast);

        // Step 3: Match Wait calls to their waiters and create synthetic calls
        let mut synthetic_calls = Vec::new();
        let mut matched_waiter_indices = std::collections::HashSet::new();

        for wait_call in wait_calls {
            if let Some((waiter, waiter_idx)) = self.match_wait_to_waiter(&wait_call, &waiters) {
                let calls = WaiterCallPattern::Matched {
                    creation: waiter,
                    wait: &wait_call,
                }
                .create_synthetic_calls(
                    self.service_index,
                    |params| self.filter_waiter_parameters(params),
                    |service_name, operation_name| {
                        self.get_required_parameters(service_name, operation_name)
                    },
                    |operation_name| operation_name.to_string(), // Go uses operation name directly
                );
                synthetic_calls.extend(calls);
                matched_waiter_indices.insert(waiter_idx);
            }
        }

        // Step 4: Handle unmatched waiter creation calls
        for (idx, waiter) in waiters.iter().enumerate() {
            if !matched_waiter_indices.contains(&idx) {
                let calls = WaiterCallPattern::CreationOnly(waiter).create_synthetic_calls(
                    self.service_index,
                    |params| self.filter_waiter_parameters(params),
                    |service_name, operation_name| {
                        self.get_required_parameters(service_name, operation_name)
                    },
                    |operation_name| operation_name.to_string(), // Go uses operation name directly
                );
                synthetic_calls.extend(calls);
            }
        }

        synthetic_calls
    }

    /// Find all waiter creation calls (NewXxxWaiter functions)
    fn find_waiter_creation_calls(&self, ast: &AstWithSourceFile<Go>) -> Vec<WaiterCreationInfo> {
        let root = ast.ast.root();
        let mut waiters = Vec::new();

        // Pattern: $VAR := $PACKAGE.$FUNCTION($$$ARGS) where FUNCTION contains "New" and "Waiter"
        let waiter_pattern = "$VAR := $PACKAGE.$FUNCTION($$$ARGS)";

        for node_match in root.find_all(waiter_pattern) {
            if let Some(waiter_info) =
                self.parse_waiter_creation_call(&node_match, &ast.source_file.path)
            {
                waiters.push(waiter_info);
            }
        }

        waiters
    }

    /// Find all Wait method calls
    fn find_wait_calls(&self, ast: &AstWithSourceFile<Go>) -> Vec<WaiterCallInfo> {
        let root = ast.ast.root();
        let mut wait_calls = Vec::new();

        // Pattern: $WAITER.Wait($$$ARGS)
        let wait_pattern = "$WAITER.Wait($$$ARGS)";

        for node_match in root.find_all(wait_pattern) {
            if let Some(wait_info) = self.parse_wait_call(&node_match, &ast.source_file.path) {
                wait_calls.push(wait_info);
            }
        }

        wait_calls
    }

    /// Parse a waiter creation call
    fn parse_waiter_creation_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
        file_path: &Path,
    ) -> Option<WaiterCreationInfo> {
        let env = node_match.get_env();

        // Extract variable name
        let variable_name = env.get_match("VAR")?.text().to_string();

        // Extract function name
        let function_name = env.get_match("FUNCTION")?.text();

        // Check if this is a waiter creation call (contains "New" and "Waiter")
        if !function_name.contains("New") || !function_name.ends_with("Waiter") {
            return None;
        }

        // Extract client parameter from arguments (first argument)
        let args_nodes = env.get_multiple_matches("ARGS");
        let client_receiver = if let Some(first_arg) = args_nodes.first() {
            first_arg.text().to_string()
        } else {
            return None; // Waiter creation should have at least one argument (the client)
        };

        // Extract waiter name from function name (remove "New" prefix and "Waiter" suffix)
        // e.g., "NewInstanceTerminatedWaiter" -> "InstanceTerminated"
        let waiter_name = function_name
            .strip_prefix("New")
            .and_then(|s| s.strip_suffix("Waiter"));

        if let Some(waiter_name) = waiter_name {
            return Some(WaiterCreationInfo {
                variable_name,
                waiter_name: waiter_name.to_string(),
                client_receiver,
                expr: node_match.text().to_string(),
                location: Location::from_node(file_path.to_path_buf(), node_match.get_node()),
            });
        }

        None
    }

    /// Parse a Wait call into WaiterCallInfo
    fn parse_wait_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
        file_path: &Path,
    ) -> Option<WaiterCallInfo> {
        let env = node_match.get_env();

        // Extract waiter variable name
        let waiter_var = env.get_match("WAITER")?.text().to_string();

        // Extract arguments
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = utils::extract_arguments(&args_nodes);

        Some(WaiterCallInfo {
            waiter_var,
            arguments,
            expr: node_match.text().to_string(),
            location: Location::from_node(file_path.to_path_buf(), node_match.get_node()),
        })
    }

    /// Match a Wait call to its corresponding waiter creation call
    fn match_wait_to_waiter<'b>(
        &self,
        wait_call: &WaiterCallInfo,
        waiters: &'b [WaiterCreationInfo],
    ) -> Option<(&'b WaiterCreationInfo, usize)> {
        // Find waiter with matching variable name
        let mut best_match = None;
        let mut best_distance = usize::MAX;
        let mut best_idx = 0;

        for (idx, waiter) in waiters.iter().enumerate() {
            if waiter.variable_name == wait_call.waiter_var {
                // Only consider waiters that come before the wait call
                if waiter.location.start_line() < wait_call.location.start_line() {
                    let distance = wait_call.location.start_line() - waiter.location.start_line();
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

    /// Get required parameters for an operation from the service index
    fn get_required_parameters(&self, service_name: &str, operation_name: &str) -> Vec<Parameter> {
        let mut parameters = Vec::new();

        if let Some(service_def) = self.service_index.services.get(service_name) {
            if let Some(operation) = service_def.operations.get(operation_name) {
                // Get the input shape if it exists
                if let Some(input_ref) = &operation.input {
                    if let Some(input_shape) = service_def.shapes.get(&input_ref.shape) {
                        // Extract required parameters
                        if let Some(required_params) = &input_shape.required {
                            for (position, param_name) in required_params.iter().enumerate() {
                                parameters.push(Parameter::Positional {
                                    value: ParameterValue::Unresolved(param_name.clone()),
                                    position,
                                    type_annotation: None,
                                    struct_fields: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        parameters
    }

    /// Filter out Go waiter-specific parameters
    /// In Go SDK v2, waiter.Wait() takes: context, input struct, and timeout duration
    /// We keep context and input struct, filter out timeout (not part of AWS operation)
    fn filter_waiter_parameters(&self, parameters: Vec<Parameter>) -> Vec<Parameter> {
        parameters.into_iter().take(2).collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::sdk_model::ServiceMethodRef;
    use crate::{Language, SourceFile};

    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::Go;
    use std::{collections::HashMap, path::PathBuf};

    fn create_test_ast(source_code: &str) -> AstWithSourceFile<Go> {
        let source_file =
            SourceFile::with_language(PathBuf::new(), source_code.to_string(), Language::Go);
        let ast_grep = Go.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file)
    }

    fn create_test_service_index() -> ServiceModelIndex {
        use crate::extraction::sdk_model::{
            Operation, SdkServiceDefinition, ServiceMetadata, Shape, ShapeReference,
        };

        let mut services = HashMap::new();
        let mut waiter_lookup = HashMap::new();

        // Create EC2 service with DescribeInstances operation
        let mut ec2_operations = HashMap::new();
        let mut ec2_waiters = HashMap::new();
        let mut ec2_shapes = HashMap::new();

        // Create input shape with required parameters
        let mut input_shape_members = HashMap::new();
        input_shape_members.insert(
            "InstanceIds".to_string(),
            ShapeReference {
                shape: "InstanceIdStringList".to_string(),
            },
        );

        ec2_shapes.insert(
            "DescribeInstancesRequest".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: input_shape_members,
                required: Some(vec!["InstanceIds".to_string()]),
            },
        );

        let describe_instances_op = Operation {
            name: "DescribeInstances".to_string(),
            input: Some(ShapeReference {
                shape: "DescribeInstancesRequest".to_string(),
            }),
        };

        ec2_operations.insert(
            "DescribeInstances".to_string(),
            describe_instances_op.clone(),
        );
        ec2_waiters.insert(
            "InstanceTerminated".to_string(),
            describe_instances_op.clone(),
        );
        ec2_waiters.insert("InstanceRunning".to_string(), describe_instances_op);

        services.insert(
            "ec2".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2016-11-15".to_string(),
                    service_id: "EC2".to_string(),
                },
                operations: ec2_operations,
                shapes: ec2_shapes,
            },
        );

        // Use PascalCase for waiter_to_services index
        waiter_lookup.insert(
            "InstanceTerminated".to_string(),
            vec![ServiceMethodRef {
                service_name: "ec2".to_string(),
                operation_name: "DescribeInstances".to_string(),
            }],
        );
        waiter_lookup.insert(
            "InstanceRunning".to_string(),
            vec![ServiceMethodRef {
                service_name: "ec2".to_string(),
                operation_name: "DescribeInstances".to_string(),
            }],
        );

        ServiceModelIndex {
            services,
            method_lookup: HashMap::new(),
            waiter_lookup,
        }
    }

    #[test]
    fn test_find_waiter_creation_calls() {
        let source_code = r#"
package main

import (
    "context"
    "log"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }
    client := ec2.NewFromConfig(cfg)
    s3Client := s3.NewFromConfig(cfg)
    
    // Real Go SDK waiter creation pattern
    instanceWaiter := ec2.NewInstanceRunningWaiter(client)
    bucketWaiter := s3.NewBucketExistsWaiter(s3Client)
    
    // Use variables to avoid unused warnings
    _ = instanceWaiter
    _ = bucketWaiter
}
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = GoWaiterExtractor::new(&service_index);

        let waiters = extractor.find_waiter_creation_calls(&ast);

        println!("Found {} waiters", waiters.len());
        for waiter in &waiters {
            println!(
                "  - {} := {}.{}",
                waiter.variable_name, waiter.client_receiver, waiter.waiter_name
            );
        }

        // Should find waiter creation calls with correct Go SDK pattern
        assert_eq!(waiters.len(), 2);
        assert_eq!(waiters[0].variable_name, "instanceWaiter");
        assert_eq!(waiters[0].waiter_name, "InstanceRunning");
        assert_eq!(waiters[0].client_receiver, "client"); // Client parameter, not package name
    }

    #[test]
    fn test_matched_waiter_and_wait() {
        let source_code = r#"
package main

import (
    "context"
    "time"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }
    client := ec2.NewFromConfig(cfg)
    
    // Real Go SDK waiter pattern: creation + wait
    instanceWaiter := ec2.NewInstanceRunningWaiter(client)
    err = instanceWaiter.Wait(context.TODO(), &ec2.DescribeInstancesInput{
        InstanceIds: []string{"i-1234567890abcdef0"},
    }, 5*time.Minute)
    if err != nil {
        log.Printf("Error: %v", err)
    }
}
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = GoWaiterExtractor::new(&service_index);

        let calls = extractor.extract_waiter_method_calls(&ast);

        // Should extract synthetic call for matched waiter pattern
        assert!(!calls.is_empty());
        assert_eq!(calls[0].name, "DescribeInstances");
    }

    #[test]
    fn test_filter_waiter_parameters() {
        let service_index = create_test_service_index();
        let extractor = GoWaiterExtractor::new(&service_index);

        // Real Go SDK v2 waiter.Wait() parameters: context, input struct, timeout
        let parameters = vec![
            Parameter::expression("context.TODO()".to_string(), 0),
            Parameter::struct_literal(
                "ec2.DescribeInstancesInput".to_string(),
                vec![crate::extraction::go::extractor::StructField {
                    name: "InstanceIds".to_string(),
                    value: "[]string{\"i-123\"}".to_string(),
                }],
                1,
            ),
            Parameter::expression("5*time.Minute".to_string(), 2), // timeout - should be filtered out
        ];

        let filtered = extractor.filter_waiter_parameters(parameters);

        // Should keep context and input struct, filter out timeout
        assert_eq!(filtered.len(), 2);

        // First parameter should be context expression
        if let Parameter::Positional { value, .. } = &filtered[0] {
            assert_eq!(
                value,
                &ParameterValue::Unresolved("context.TODO()".to_string())
            );
        } else {
            panic!("Expected context expression parameter");
        }

        // Second parameter should be struct literal
        if let Parameter::Positional {
            type_annotation, ..
        } = &filtered[1]
        {
            assert!(type_annotation
                .as_ref()
                .unwrap()
                .contains("DescribeInstancesInput"));
        } else {
            panic!("Expected struct literal parameter");
        }
    }

    #[test]
    fn test_unmatched_waiter_creation() {
        let source_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }
    client := ec2.NewFromConfig(cfg)
    
    // Unmatched waiter - creation without Wait() call
    instanceWaiter := ec2.NewInstanceRunningWaiter(client)
    _ = instanceWaiter // Use variable to avoid unused warning
}
"#;

        let ast = create_test_ast(source_code);
        let service_index = create_test_service_index();
        let extractor = GoWaiterExtractor::new(&service_index);

        let calls = extractor.extract_waiter_method_calls(&ast);

        // Should create synthetic call for unmatched waiter
        assert!(!calls.is_empty());
        assert_eq!(calls[0].name, "DescribeInstances");
        // Should have required parameters with placeholder values
        assert!(!calls[0].metadata.as_ref().unwrap().parameters.is_empty());

        // Check that required parameter has parameter name as value
        let params = &calls[0].metadata.as_ref().unwrap().parameters;
        if let Parameter::Positional {
            value,
            type_annotation,
            ..
        } = &params[0]
        {
            assert_eq!(
                value,
                &ParameterValue::Unresolved("InstanceIds".to_string())
            );
            assert!(type_annotation.is_none());
        } else {
            panic!("Expected positional parameter with parameter name as value");
        }
    }
}
