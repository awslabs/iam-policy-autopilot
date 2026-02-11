//! SDK method extraction for Go using ast-grep

use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::go::disambiguation::GoMethodDisambiguator;
use crate::extraction::go::features_extractor::GoFeaturesExtractor;
use crate::extraction::go::node_kinds;
use crate::extraction::go::paginator_extractor::GoPaginatorExtractor;
use crate::extraction::go::types::{GoImportInfo, ImportInfo};
use crate::extraction::go::waiter_extractor::GoWaiterExtractor;
use crate::extraction::{
    AstWithSourceFile, Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata,
};
use crate::{Location, ServiceModelIndex, SourceFile};
use ast_grep_config::from_yaml_string;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Go;
use async_trait::async_trait;

pub(crate) struct GoExtractor {}

impl GoExtractor {
    /// Create a new Go extractor instance
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// Extract import statements from Go source code using ast-grep
    fn extract_imports(&self, ast: &AstWithSourceFile<Go>) -> GoImportInfo {
        let mut import_info = GoImportInfo::new();
        let root = ast.ast.root();

        // AST-grep configuration for extracting import statements
        let import_config = r"
id: import_extraction
language: Go
rule:
  kind: import_spec
  has:
    field: path
    pattern: $PATH
    kind: interpreted_string_literal
";

        let globals = ast_grep_config::GlobalRules::default();
        let config =
            &from_yaml_string::<Go>(import_config, &globals).expect("import rule should parse")[0];

        // Find all import statements
        for node_match in root.find_all(&config.matcher) {
            if let Some(import) = self.parse_import(&node_match) {
                import_info.add_import(import);
            }
        }

        // Also handle import declarations with aliases
        let import_alias_config = r"
id: import_alias_extraction
language: Go
rule:
  kind: import_spec
  all:
    - has:
        field: name
        pattern: $ALIAS
        kind: package_identifier
    - has:
        field: path
        pattern: $PATH
        kind: interpreted_string_literal
";

        let alias_config = &from_yaml_string::<Go>(import_alias_config, &globals)
            .expect("import alias rule should parse")[0];

        // Find all aliased import statements
        for node_match in root.find_all(&alias_config.matcher) {
            if let Some(import) = self.parse_aliased_import(&node_match) {
                import_info.add_import(import);
            }
        }

        import_info
    }

    /// Parse a single import statement
    fn parse_import(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Option<ImportInfo> {
        let env = node_match.get_env();

        if let Some(path_node) = env.get_match("PATH") {
            let path_text = path_node.text();
            // Remove quotes from the import path
            let import_path = path_text.trim_matches('"');

            // Extract the package name (last part of the path)
            let package_name = import_path.split('/').next_back().unwrap_or(import_path);

            // Get line number
            let line = path_node.start_pos().line() + 1;

            return Some(ImportInfo::new(
                import_path.to_string(),
                package_name.to_string(),
                line,
            ));
        }

        None
    }

    /// Parse an aliased import statement
    fn parse_aliased_import(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Option<ImportInfo> {
        let env = node_match.get_env();

        if let (Some(alias_node), Some(path_node)) = (env.get_match("ALIAS"), env.get_match("PATH"))
        {
            let alias_text = alias_node.text();
            let path_text = path_node.text();
            // Remove quotes from the import path
            let import_path = path_text.trim_matches('"');

            // Get line number
            let line = path_node.start_pos().line() + 1;

            return Some(ImportInfo::new(
                import_path.to_string(),
                alias_text.to_string(),
                line,
            ));
        }

        None
    }

    /// Parse a single method call match into a SdkMethodCall
    fn parse_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
        source_file: &SourceFile,
    ) -> Option<SdkMethodCall> {
        let env = node_match.get_env();

        // Extract the receiver (object before the dot)
        let receiver = env
            .get_match("OBJ")
            .map(|obj_node| obj_node.text().to_string());

        // Extract the method name
        let method_name = if let Some(method_node) = env.get_match("METHOD") {
            method_node.text()
        } else {
            return None;
        };

        // Extract arguments - ARGS captures the entire argument_list node
        // We need to get its children to access individual arguments
        let arguments = if let Some(args_node) = env.get_match("ARGS") {
            log::debug!("Matched argument_list node: {:?}", args_node.text());

            // Get the children of the argument_list node (excluding parentheses)
            let arg_children: Vec<_> = args_node
                .children()
                .filter(|child| {
                    // Filter out parentheses and commas, keep only actual argument nodes
                    let kind = child.kind();
                    kind != node_kinds::LEFT_PAREN
                        && kind != node_kinds::RIGHT_PAREN
                        && kind != node_kinds::COMMA
                })
                .collect();

            log::debug!("Found {} argument children", arg_children.len());
            for (i, child) in arg_children.iter().enumerate() {
                log::debug!(
                    "Argument [{}]: kind={}, text={:?}",
                    i,
                    child.kind(),
                    child.text()
                );
            }

            self.extract_arguments_with_ast(&arg_children)
        } else {
            vec![]
        };

        let method_call = SdkMethodCall {
            name: method_name.to_string(),
            possible_services: Vec::new(), // Will be determined later during service validation
            metadata: Some(SdkMethodCallMetadata {
                parameters: arguments,
                return_type: None, // We don't know the return type from the call site
                expr: node_match.text().to_string(),
                location: Location::from_node(source_file.path.clone(), node_match.get_node()),
                receiver,
            }),
        };

        Some(method_call)
    }

    /// Extract arguments from argument nodes with AST-based field extraction
    fn extract_arguments_with_ast(
        &self,
        args_nodes: &[ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>],
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();

        for (position, arg_node) in args_nodes.iter().enumerate() {
            log::debug!("Extracting parameter from: {:?}", arg_node.text());
            let arg_text = arg_node.text().to_string();

            // Check if this is a context parameter (first parameter in Go AWS SDK calls)
            if position == 0 && self.is_context_parameter(arg_node) {
                parameters.push(Parameter::context(arg_text, position));
            }
            // Check if this is a struct literal (&Type{...})
            else if self.is_struct_literal(arg_node) {
                // Use AST-based extraction for struct fields
                let type_annotation = self.extract_type_from_struct_literal(&arg_node.text());
                let fields = self.extract_struct_fields_from_ast(arg_node);

                // Store as struct literal with proper field extraction
                parameters.push(Parameter::Positional {
                    value: ParameterValue::Unresolved(arg_text),
                    position,
                    type_annotation,
                    struct_fields: Some(fields.clone()),
                });

                log::debug!(
                    "Extracted {} struct fields from composite literal",
                    fields.len()
                );
            }
            // Otherwise, it's a general expression
            else {
                parameters.push(Parameter::expression(arg_text, position));
            }
        }

        log::debug!("Extracted {} parameters", parameters.len());
        parameters
    }

    /// Extract type name from struct literal text
    fn extract_type_from_struct_literal(&self, text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.starts_with('&') {
            if let Some(brace_pos) = trimmed.find('{') {
                let type_part = trimmed[1..brace_pos].trim();
                return Some(type_part.to_string());
            }
        }
        None
    }

    /// Extract top-level struct fields from AST node
    fn extract_struct_fields_from_ast(
        &self,
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Vec<String> {
        let field_names = Vec::new();

        log::debug!(
            "Extracting struct fields from AST node: kind={}",
            node.kind()
        );
        log::debug!(
            "Node text (first 100 chars): {:?}",
            &node.text().chars().take(100).collect::<String>()
        );

        // Check if this node is directly a composite_literal
        if node.kind() == node_kinds::COMPOSITE_LITERAL {
            log::debug!("Node is directly a composite_literal");
            return self.extract_fields_from_composite_literal(node);
        }

        // Check immediate children for unary_expression (for &Type{...} pattern)
        for child in node.children() {
            log::debug!("Checking child node: kind={}", child.kind());
            if child.kind() == node_kinds::UNARY_EXPRESSION {
                // Check the unary_expression's children for composite_literal
                for unary_child in child.children() {
                    log::debug!("Checking unary child: kind={}", unary_child.kind());
                    if unary_child.kind() == node_kinds::COMPOSITE_LITERAL {
                        log::debug!("Found composite_literal under unary_expression");
                        return self.extract_fields_from_composite_literal(&unary_child);
                    }
                }
            } else if child.kind() == node_kinds::COMPOSITE_LITERAL {
                log::debug!("Found composite_literal as direct child");
                return self.extract_fields_from_composite_literal(&child);
            }
        }

        log::debug!("No composite_literal found");
        field_names
    }

    /// Extract field names from a composite_literal node
    ///
    /// A `composite_literal` is a tree-sitter AST node type representing Go's composite literal syntax.
    /// In Go, composite literals construct values for structs, arrays, slices, and maps.
    ///
    /// For our purposes, we focus on struct literals with named fields.
    ///
    /// # References
    ///
    /// - Go Language Spec: <https://go.dev/ref/spec#Composite_literals>
    /// - Tree-sitter Go Grammar: <https://github.com/tree-sitter/tree-sitter-go/blob/master/grammar.js>
    ///   (search for `composite_literal` to see the grammar definition)
    ///
    /// # Examples of composite_literals
    ///
    /// ```go
    /// // Struct literal (what we extract from)
    /// &s3.GetObjectInput{
    ///     Bucket: aws.String("my-bucket"),  // Field: "Bucket"
    ///     Key:    aws.String("my-key"),     // Field: "Key"
    /// }
    ///
    /// // Array literal (not extracted)
    /// []string{"a", "b", "c"}
    ///
    /// // Map literal (not extracted - we only get top-level fields)
    /// map[string]string{
    ///     "key1": "value1",
    ///     "key2": "value2",
    /// }
    /// ```
    ///
    /// This function extracts only the top-level field names from struct literals,
    /// not nested structures or map keys.
    fn extract_fields_from_composite_literal(
        &self,
        composite_literal: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Vec<String> {
        let mut field_names = Vec::new();

        log::debug!("Extracting fields from composite_literal, looking for literal_value...");

        // Find the literal_value child which contains the fields.
        // A composite_literal has exactly one literal_value child in the AST.
        let literal_value = composite_literal
            .children()
            .find(|child| child.kind() == node_kinds::LITERAL_VALUE);

        if let Some(literal_value) = literal_value {
            log::debug!("Found literal_value, extracting keyed_elements...");
            // Extract field names from keyed_element nodes
            for element in literal_value.children() {
                log::debug!("Literal_value child: kind={}", element.kind());
                if element.kind() == node_kinds::KEYED_ELEMENT {
                    // Get the first child, which is the field name (literal_element)
                    if let Some(field_name_node) = element.children().next() {
                        log::debug!(
                            "Keyed_element first child: kind={}, text={:?}",
                            field_name_node.kind(),
                            field_name_node.text()
                        );
                        if field_name_node.kind() == node_kinds::LITERAL_ELEMENT {
                            field_names.push(field_name_node.text().to_string());
                        }
                    }
                }
            }
        }

        log::debug!("Field names extracted from AST: {field_names:?}");
        field_names
    }

    /// Check if a node represents a context parameter
    fn is_context_parameter(
        &self,
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> bool {
        let text = node.text();
        text.starts_with("context.") || text == "ctx" || text.contains("Context")
    }

    /// Check if a node represents a struct literal (&Type{...})
    fn is_struct_literal(
        &self,
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> bool {
        let text = node.text();
        let trimmed = text.trim();
        trimmed.starts_with('&') && trimmed.contains('{') && trimmed.ends_with('}')
    }
}

/// Represents a field in a Go struct literal
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructField {
    pub(crate) name: String,
    pub(crate) value: String,
}

// GoParameter enum removed - using the generic Parameter enum from the parent module instead

impl Default for GoExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Extractor for GoExtractor {
    async fn parse(
        &self,
        source_file: &SourceFile,
    ) -> crate::extraction::extractor::ExtractorResult {
        let ast_grep = Go.ast_grep(&source_file.content);
        let ast = AstWithSourceFile::new(ast_grep, source_file.clone());
        let root = ast.ast.root();

        let mut method_calls = Vec::new();

        let config = r"
id: method_call_extraction
language: Go
rule:
  kind: call_expression
  all:
    - has:
        field: function
        kind: selector_expression
        all:
          - has:
              field: field
              pattern: $METHOD
              kind: field_identifier
          - has:
              field: operand
              pattern: $OBJ
    - has:
        field: arguments
        pattern: $$$ARGS
        kind: argument_list
        ";

        let globals = ast_grep_config::GlobalRules::default();
        let config = &from_yaml_string::<Go>(config, &globals).expect("rule should parse")[0];

        // Find all method calls with attribute access: receiver.method(args)
        for node_match in root.find_all(&config.matcher) {
            if let Some(method_call) = self.parse_method_call(&node_match, source_file) {
                method_calls.push(method_call);
            }
        }

        // Extract import information
        let import_info = self.extract_imports(&ast);

        crate::extraction::extractor::ExtractorResult::Go(ast, method_calls, import_info)
    }

    fn filter_map(
        &self,
        extractor_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    ) {
        let method_disambiguator = GoMethodDisambiguator::new(service_index);
        let waiter_extractor = GoWaiterExtractor::new(service_index);

        for extractor_result in extractor_results.iter_mut() {
            match extractor_result {
                ExtractorResult::Python(_, _) => {
                    // This shouldn't happen in Go extractor, but handle gracefully
                    panic!("Received Python result during Go method extraction.");
                }
                ExtractorResult::Go(ast, method_calls, import_info) => {
                    // Add waiter method calls
                    let waiter_calls = waiter_extractor.extract_waiter_method_calls(ast);
                    method_calls.extend(waiter_calls);

                    // Add paginator method calls
                    let paginator_extractor = GoPaginatorExtractor::new(service_index);
                    let paginator_calls = paginator_extractor.extract_paginator_method_calls(ast);
                    method_calls.extend(paginator_calls);

                    // Add feature method calls
                    match GoFeaturesExtractor::new() {
                        Ok(features_extractor) => {
                            let feature_calls =
                                features_extractor.extract_feature_method_calls(ast, import_info);
                            method_calls.extend(feature_calls);
                        }
                        Err(e) => {
                            log::debug!("Failed to create GoFeaturesExtractor: {e}");
                        }
                    }

                    // Clone the method calls to pass to disambiguate_method_calls
                    let filtered_and_mapped = method_disambiguator
                        .disambiguate_method_calls(method_calls.clone(), Some(import_info));

                    // Replace the method calls in place
                    *method_calls = filtered_and_mapped;
                }
                ExtractorResult::JavaScript(_, _) => {
                    // This shouldn't happen in Go extractor, but handle gracefully
                    panic!("Received JavaScript result during Go method extraction.");
                }
                ExtractorResult::TypeScript(_, _) => {
                    // This shouldn't happen in Go extractor, but handle gracefully
                    panic!("Received TypeScript result during Go method extraction.");
                }
            }
        }
    }

    fn disambiguate(
        &self,
        _extractor_results: &mut [ExtractorResult],
        _service_index: &ServiceModelIndex,
    ) {
    }
}

// Helper methods for creating Go-specific parameters
impl Parameter {
    /// Create a context parameter
    pub(crate) fn context(expression: String, position: usize) -> Self {
        Self::Positional {
            value: ParameterValue::Unresolved(expression),
            position,
            type_annotation: Some("context.Context".to_string()),
            struct_fields: None,
        }
    }

    /// Create a struct literal parameter
    pub(crate) fn struct_literal(
        type_name: String,
        fields: Vec<StructField>,
        position: usize,
    ) -> Self {
        // Convert struct fields to a string representation
        let fields_str = fields
            .iter()
            .map(|f| format!("{}: {}", f.name, f.value))
            .collect::<Vec<_>>()
            .join(", ");

        Self::Positional {
            value: ParameterValue::Unresolved(format!("&{type_name}{{ {fields_str} }}")),
            position,
            type_annotation: Some(type_name),
            struct_fields: Some(fields.iter().map(|f| f.name.clone()).collect()),
        }
    }

    /// Create an expression parameter
    pub(crate) fn expression(value: String, position: usize) -> Self {
        Self::Positional {
            value: ParameterValue::Unresolved(value),
            position,
            type_annotation: None,
            struct_fields: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn test_go_method_call_parsing() {
        let extractor = GoExtractor::new();

        // Now test with the AWS SDK code
        let aws_code = r#"
package main

import (
    "context"
    "log"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    // Load the Shared AWS Configuration (~/.aws/config)
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }

    // Create an Amazon S3 service client
    client := s3.NewFromConfig(cfg)

    // Get the first page of results for ListObjectsV2 for a bucket
    output, err := client.ListObjectsV2(context.TODO(), &s3.ListObjectsV2Input{
        Bucket: aws.String("amzn-s3-demo-bucket"),
    })
    if err != nil {
        log.Fatal(err)
    }

    log.Println("first page results")
    for _, object := range output.Contents {
        log.Printf("key=%s size=%d", aws.ToString(object.Key), *object.Size)
    }
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), aws_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;
        let aws_method_calls = result.method_calls_ref();

        println!("AWS test - Found {} method calls:", aws_method_calls.len());
        for call in aws_method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        assert!(
            aws_method_calls.len() == 11,
            "Should find at least one method call in either test"
        );
    }

    #[tokio::test]
    async fn test_chained_method_call_parsing() {
        let extractor = GoExtractor::new();

        // Test the specific issue with chained method calls
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/sts"
)

func main() {
    // Case 1: This should match with OBJ = m.stsClient, METHOD = GetCallerIdentity
    result, err := m.stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
    
    // Case 2: This should match with OBJ = stsClient, METHOD = GetCallerIdentity
    result2, err2 := stsClient.GetCallerIdentity(ctx, &sts.GetCallerIdentityInput{})
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        println!(
            "Chained method call test - Found {} method calls:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        // We should find 2 GetCallerIdentity calls
        let get_caller_identity_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "GetCallerIdentity")
            .collect();

        assert_eq!(
            get_caller_identity_calls.len(),
            2,
            "Should find 2 GetCallerIdentity calls"
        );

        // Check the receivers
        let receivers: Vec<_> = get_caller_identity_calls
            .iter()
            .map(|call| {
                call.metadata
                    .as_ref()
                    .and_then(|m| m.receiver.as_ref())
                    .unwrap()
            })
            .collect();

        println!("Receivers found: {:?}", receivers);
    }

    #[tokio::test]
    async fn test_longer_chained_method_call_parsing() {
        let extractor = GoExtractor::new();

        // Test with longer chains to ensure the pattern works for complex scenarios
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
    // Case 1: Longer chain - app.services.s3Client.ListBuckets()
    result1, err1 := app.services.s3Client.ListBuckets(ctx, &s3.ListBucketsInput{})
    
    // Case 2: Even longer chain - config.aws.clients.ec2.DescribeInstances()
    result2, err2 := config.aws.clients.ec2.DescribeInstances(ctx, &ec2.DescribeInstancesInput{})
    
    // Case 3: Simple case for comparison - client.GetObject()
    result3, err3 := client.GetObject(ctx, &s3.GetObjectInput{})
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        println!(
            "Longer chain test - Found {} method calls:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        // We should find 3 method calls
        assert_eq!(method_calls.len(), 3, "Should find 3 method calls");

        // Check specific method calls and their receivers
        let list_buckets_call = method_calls
            .iter()
            .find(|call| call.name == "ListBuckets")
            .expect("Should find ListBuckets call");
        assert_eq!(
            list_buckets_call
                .metadata
                .as_ref()
                .unwrap()
                .receiver
                .as_ref()
                .unwrap(),
            "app.services.s3Client"
        );

        let describe_instances_call = method_calls
            .iter()
            .find(|call| call.name == "DescribeInstances")
            .expect("Should find DescribeInstances call");
        assert_eq!(
            describe_instances_call
                .metadata
                .as_ref()
                .unwrap()
                .receiver
                .as_ref()
                .unwrap(),
            "config.aws.clients.ec2"
        );

        let get_object_call = method_calls
            .iter()
            .find(|call| call.name == "GetObject")
            .expect("Should find GetObject call");
        assert_eq!(
            get_object_call
                .metadata
                .as_ref()
                .unwrap()
                .receiver
                .as_ref()
                .unwrap(),
            "client"
        );
    }

    #[tokio::test]
    async fn test_method_calls_in_receiver_chain() {
        let extractor = GoExtractor::new();

        // Test with method calls within the receiver chain
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
    // Case 1: Method call in receiver chain - app.getServices().s3Client.ListBuckets()
    result1, err1 := app.getServices().s3Client.ListBuckets(ctx, &s3.ListBucketsInput{})
    
    // Case 2: Multiple method calls in chain - config.getAWS().getClients().ec2.DescribeInstances()
    result2, err2 := config.getAWS().getClients().ec2.DescribeInstances(ctx, &ec2.DescribeInstancesInput{})
    
    // Case 3: Mixed chain - factory.createClient("s3").GetObject()
    result3, err3 := factory.createClient("s3").GetObject(ctx, &s3.GetObjectInput{})
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        println!(
            "Method calls in receiver chain test - Found {} method calls:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        // Let's see what we actually get and analyze the behavior
        // We expect to find multiple method calls, including the intermediate ones
        assert!(
            method_calls.len() >= 3,
            "Should find at least 3 method calls"
        );

        // Check if we can find the final method calls we're interested in
        let list_buckets_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "ListBuckets")
            .collect();
        let describe_instances_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "DescribeInstances")
            .collect();
        let get_object_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "GetObject")
            .collect();

        println!("Found {} ListBuckets calls", list_buckets_calls.len());
        println!(
            "Found {} DescribeInstances calls",
            describe_instances_calls.len()
        );
        println!("Found {} GetObject calls", get_object_calls.len());

        // We should find at least one of each final method call
        assert!(
            !list_buckets_calls.is_empty(),
            "Should find at least 1 ListBuckets call"
        );
        assert!(
            !describe_instances_calls.is_empty(),
            "Should find at least 1 DescribeInstances call"
        );
        assert!(
            !get_object_calls.is_empty(),
            "Should find at least 1 GetObject call"
        );

        println!("✅ Method calls in receiver chain test completed!");
    }

    #[tokio::test]
    async fn test_no_match_for_methods_without_receiver() {
        let extractor = GoExtractor::new();

        // Test code with plain function calls (no receiver) - these should NOT match
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    // These are plain function calls without a receiver - should NOT match
    ListObjectsV2(ctx, &s3.ListObjectsV2Input{
        Bucket: aws.String("my-bucket"),
    })
    
    GetObject(ctx, &s3.GetObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("my-key"),
    })
    
    // This has a receiver and SHOULD match
    client.ListBuckets(ctx, &s3.ListBucketsInput{})
    
    // Another plain function call - should NOT match
    DescribeInstances(ctx, &ec2.DescribeInstancesInput{})
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        println!(
            "No receiver test - Found {} method calls:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        // We expect to find method calls with receivers (aws.String calls + client.ListBuckets)
        // but NOT plain function calls without receivers
        assert!(
            !method_calls.is_empty(),
            "Should find at least 1 method call with receiver"
        );

        // Verify that we find the client.ListBuckets call
        let list_buckets_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "ListBuckets")
            .collect();
        assert_eq!(
            list_buckets_calls.len(),
            1,
            "Should find exactly 1 ListBuckets call"
        );
        assert_eq!(
            list_buckets_calls[0]
                .metadata
                .as_ref()
                .unwrap()
                .receiver
                .as_ref()
                .unwrap(),
            "client"
        );

        // Verify that plain function calls like ListObjectsV2, GetObject, DescribeInstances are NOT matched
        let plain_function_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| {
                matches!(
                    call.name.as_str(),
                    "ListObjectsV2" | "GetObject" | "DescribeInstances"
                )
            })
            .collect();

        assert_eq!(
            plain_function_calls.len(),
            0,
            "Plain function calls without receiver should not be matched by ast-grep configuration"
        );

        // Verify that aws.String calls are correctly matched (they have receivers)
        let aws_string_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| {
                call.name == "String"
                    && call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
                        == Some(&"aws".to_string())
            })
            .collect();
        assert!(
            !aws_string_calls.is_empty(),
            "Should find aws.String calls (they have receivers)"
        );
    }

    #[tokio::test]
    async fn test_import_extraction_and_filtering() {
        use crate::extraction::sdk_model::{
            Operation, SdkServiceDefinition, ServiceMetadata, ServiceMethodRef, ServiceModelIndex,
            Shape, ShapeReference,
        };
        use std::collections::HashMap;

        let extractor = GoExtractor::new();

        // Create a test service index with both s3 and s3control services having GetObject
        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // S3 service
        let mut s3_operations = HashMap::new();
        s3_operations.insert(
            "ListObjectsV2".to_string(),
            Operation {
                name: "ListObjectsV2".to_string(),
                input: Some(ShapeReference {
                    shape: "ListObjectsV2Request".to_string(),
                }),
            },
        );
        s3_operations.insert(
            "GetObject".to_string(),
            Operation {
                name: "GetObject".to_string(),
                input: Some(ShapeReference {
                    shape: "GetObjectRequest".to_string(),
                }),
            },
        );

        // Create shapes for S3
        let mut s3_shapes = HashMap::new();
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

        // S3Control service
        let mut s3control_operations = HashMap::new();
        s3control_operations.insert(
            "GetObject".to_string(),
            Operation {
                name: "GetObject".to_string(),
                input: Some(ShapeReference {
                    shape: "GetObjectRequest".to_string(),
                }),
            },
        );

        // Create shapes for S3Control
        let mut s3control_shapes = HashMap::new();
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

        services.insert(
            "s3control".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2018-08-20".to_string(),
                    service_id: "S3Control".to_string(),
                },
                operations: s3control_operations,
                shapes: s3control_shapes,
            },
        );

        // Method lookup
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

        let service_index = ServiceModelIndex {
            services,
            method_lookup,
            waiter_lookup: HashMap::new(),
        };

        // Test Go code with S3 import but GetObject call that exists in both s3 and s3control
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    // GetObject exists in both s3 and s3control services
    // Use variable input to avoid argument-based disambiguation
    getObjectInput := &s3.GetObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("my-key"),
    }
    
    // This should be filtered to only S3 service, NOT s3control, based on imports
    objectOutput, err := s3Client.GetObject(context.TODO(), getObjectInput)
    if err != nil {
        log.Fatal(err)
    }
    _ = objectOutput
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;

        // Verify import extraction
        if let Some(import_info) = result.go_import_info() {
            let imported_services = import_info.get_imported_services();
            println!("Imported services: {:?}", imported_services);

            // Should have extracted S3 service
            assert!(imported_services.contains(&"s3".to_string()));
            // Should not contain s3control
            assert!(!imported_services.contains(&"s3control".to_string()));
        } else {
            panic!("Expected Go import info to be present");
        }

        // Check method calls before disambiguation
        let method_calls_before = result.method_calls_ref();
        println!(
            "Found {} method calls before disambiguation:",
            method_calls_before.len()
        );
        for call in method_calls_before {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
            println!("     - possible services: {:?}", call.possible_services);
        }

        // Apply disambiguation with the service index
        let mut results = vec![result];
        extractor.filter_map(&mut results, &service_index);
        let result = &results[0];

        // Verify method call extraction after disambiguation
        let method_calls = result.method_calls_ref();
        println!(
            "Found {} method calls after disambiguation:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (possible_services: {:?})",
                call.name, call.possible_services
            );
        }

        // Find GetObject calls
        let get_object_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "GetObject")
            .collect();

        assert!(!get_object_calls.is_empty(), "Should find GetObject calls");

        // Critical test: GetObject exists in both s3 and s3control services
        // Since we only import s3 (not s3control), the possible_services should be filtered to only include s3
        for get_object_call in &get_object_calls {
            println!(
                "GetObject call possible_services: {:?}",
                get_object_call.possible_services
            );
            assert!(
                get_object_call
                    .possible_services
                    .contains(&"s3".to_string()),
                "GetObject should include s3 service"
            );
            assert!(
                !get_object_call
                    .possible_services
                    .contains(&"s3control".to_string()),
                "GetObject should NOT include s3control service when only s3 is imported"
            );
        }

        println!(
            "✅ Import-based filtering test passed - s3control filtered out when only s3 imported"
        );
    }

    #[tokio::test]
    async fn test_cloudwatch_logs_service_name_mismatch() {
        let extractor = GoExtractor::new();

        // Test Go code with CloudWatch Logs import (service name mismatch case)
        // Go uses "cloudwatchlogs" but AWS service is "logs"
        let test_code = r#"
package main

import (
    "context"
    "log"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/cloudwatchlogs"
)

func main() {
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }

    // CloudWatch Logs client
    logsClient := cloudwatchlogs.NewFromConfig(cfg)
    
    // This method exists in the "logs" service in AWS, but we import "cloudwatchlogs" in Go
    // The disambiguation should NOT filter this out even though service names don't match
    output, err := logsClient.CreateLogGroup(context.TODO(), &cloudwatchlogs.CreateLogGroupInput{
        LogGroupName: aws.String("my-log-group"),
    })
    if err != nil {
        log.Fatal(err)
    }
    
    _ = output
}
        "#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);

        let result = extractor.parse(&source_file).await;

        // Verify import extraction
        if let Some(import_info) = result.go_import_info() {
            let imported_services = import_info.get_imported_services();
            println!("Imported services: {:?}", imported_services);

            // Should have extracted "cloudwatchlogs" as the service name from the import
            assert!(imported_services.contains(&"cloudwatchlogs".to_string()));

            // Should NOT contain "logs" (the actual AWS service name)
            assert!(!imported_services.contains(&"logs".to_string()));
        } else {
            panic!("Expected Go import info to be present");
        }

        // Verify method call extraction
        let method_calls = result.method_calls_ref();
        println!("Found {} method calls:", method_calls.len());
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
        }

        // Should find the CreateLogGroup method call
        let create_log_group_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "CreateLogGroup")
            .collect();

        // This is the key test: even though we import "cloudwatchlogs" but the AWS service is "logs",
        // the disambiguation should NOT filter out the method call because the service names don't match.
        // This prevents false negatives when Go service names don't exactly match AWS service names.
        assert!(
            !create_log_group_calls.is_empty(),
            "Should find CreateLogGroup calls even with service name mismatch"
        );

        println!("✅ CloudWatch Logs service name mismatch test passed - method calls preserved when Go import name doesn't match AWS service name");
    }

    #[tokio::test]
    async fn test_multiline_struct_literal_argument_extraction() {
        let extractor = GoExtractor::new();

        // Test code with multi-line struct literal
        let test_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

type BucketBasics struct {
    S3Client *s3.Client
}

func (basics BucketBasics) DownloadFile(ctx context.Context, bucketName string, objectKey string, fileName string) error {
    result, err := basics.S3Client.GetObject(ctx, &s3.GetObjectInput{
        Bucket: aws.String(bucketName),
        Key:    aws.String(objectKey),
    })
    if err != nil {
        return err
    }
    return nil
}
        "#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), test_code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        println!(
            "Multi-line struct literal test - Found {} method calls:",
            method_calls.len()
        );
        for call in method_calls {
            println!(
                "  - {} (receiver: {:?})",
                call.name,
                call.metadata.as_ref().and_then(|m| m.receiver.as_ref())
            );
            if let Some(metadata) = &call.metadata {
                println!("    Parameters: {} params", metadata.parameters.len());
                for (i, param) in metadata.parameters.iter().enumerate() {
                    match param {
                        Parameter::Positional {
                            value,
                            type_annotation,
                            ..
                        } => {
                            println!(
                                "      [{}] value: {:?}, type: {:?}",
                                i,
                                value.as_string(),
                                type_annotation
                            );
                        }
                        _ => println!("      [{}] {:?}", i, param),
                    }
                }
            }
        }

        // Find the GetObject call
        let get_object_calls: Vec<_> = method_calls
            .iter()
            .filter(|call| call.name == "GetObject")
            .collect();

        assert_eq!(
            get_object_calls.len(),
            1,
            "Should find exactly 1 GetObject call"
        );

        let get_object_call = get_object_calls[0];
        assert_eq!(
            get_object_call
                .metadata
                .as_ref()
                .unwrap()
                .receiver
                .as_ref()
                .unwrap(),
            "basics.S3Client"
        );

        // Verify we extracted the parameters
        let params = &get_object_call.metadata.as_ref().unwrap().parameters;

        // Should have at least 2 parameters: ctx and the struct literal
        assert!(
            params.len() >= 2,
            "Should have at least 2 parameters (ctx and struct literal)"
        );

        // First parameter should be context
        match &params[0] {
            Parameter::Positional { value, .. } => {
                let value_str = value.as_string();
                assert!(
                    value_str == "ctx" || value_str.contains("context"),
                    "First parameter should be context, got: {}",
                    value_str
                );
            }
            _ => panic!("Expected positional parameter for context"),
        }

        // Second parameter should be the struct literal with Bucket and Key fields
        match &params[1] {
            Parameter::Positional { value, .. } => {
                let value_str = value.as_string();
                println!("Struct literal parameter: {}", value_str);

                // Verify it's a struct literal
                assert!(
                    value_str.contains("GetObjectInput"),
                    "Should contain GetObjectInput type"
                );
                assert!(value_str.contains("Bucket"), "Should contain Bucket field");
                assert!(value_str.contains("Key"), "Should contain Key field");
                assert!(
                    value_str.contains("aws.String"),
                    "Should contain aws.String calls"
                );
            }
            _ => panic!("Expected positional parameter for struct literal"),
        }

        println!("✅ Multi-line struct literal argument extraction test passed!");
    }
}
#[cfg(test)]
mod test_struct_fields {
    use std::path::PathBuf;

    use crate::extraction::extractor::Extractor;
    use crate::extraction::go::extractor::GoExtractor;
    use crate::extraction::Parameter;
    use crate::SourceFile;

    /// Test extraction of struct literals with multiple fields
    #[tokio::test]
    async fn test_extraction_multiple_fields() {
        let extractor = GoExtractor::new();

        let code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/aws"
)

func test() {
    client := s3.NewFromConfig(cfg)
    result, _ := client.GetObject(context.TODO(), &s3.GetObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("my-key"),
    })
}
"#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        let call = method_calls
            .iter()
            .find(|c| c.name == "GetObject")
            .expect("Should find GetObject call");

        let metadata = call.metadata.as_ref().expect("Should have metadata");
        assert_eq!(metadata.parameters.len(), 2);

        // Check context parameter
        if let Parameter::Positional {
            struct_fields,
            type_annotation,
            ..
        } = &metadata.parameters[0]
        {
            assert_eq!(type_annotation.as_deref(), Some("context.Context"));
            assert!(
                struct_fields.is_none(),
                "Context should not have struct_fields"
            );
        } else {
            panic!("Expected Positional parameter for context");
        }

        // Check struct literal parameter
        if let Parameter::Positional {
            struct_fields,
            type_annotation,
            ..
        } = &metadata.parameters[1]
        {
            assert_eq!(type_annotation.as_deref(), Some("s3.GetObjectInput"));
            assert!(
                struct_fields.is_some(),
                "struct_fields should be Some for struct literal"
            );
            let fields = struct_fields.as_ref().unwrap();
            assert_eq!(fields.len(), 2);
            assert!(fields.contains(&"Bucket".to_string()));
            assert!(fields.contains(&"Key".to_string()));
        } else {
            panic!("Expected Positional parameter for struct literal");
        }
    }

    /// Test that variable references don't extract struct fields
    #[tokio::test]
    async fn test_extraction_variable_parameter() {
        let extractor = GoExtractor::new();

        let code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func test() {
    client := s3.NewFromConfig(cfg)
    getObjectInput := &s3.GetObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("my-key"),
    }
    result, _ := client.GetObject(context.TODO(), getObjectInput)
}
"#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        let call = method_calls
            .iter()
            .find(|c| c.name == "GetObject")
            .expect("Should find GetObject call");

        let metadata = call.metadata.as_ref().expect("Should have metadata");

        // Check variable parameter - should NOT extract fields
        if let Parameter::Positional {
            struct_fields,
            value,
            ..
        } = &metadata.parameters[1]
        {
            assert_eq!(value.as_string(), "getObjectInput");
            assert!(
                struct_fields.is_none(),
                "struct_fields should be None for variable reference"
            );
        } else {
            panic!("Expected Positional parameter");
        }
    }

    /// Test that nested maps only extract top-level fields (critical for SQS CreateQueue case)
    #[tokio::test]
    async fn test_extraction_nested_map_only_top_level() {
        let extractor = GoExtractor::new();

        let code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/sqs"
    "github.com/aws/aws-sdk-go-v2/aws"
)

func test() {
    client := sqs.NewFromConfig(cfg)
    result, _ := client.CreateQueue(context.TODO(), &sqs.CreateQueueInput{
        QueueName: aws.String("my-queue"),
        Attributes: map[string]string{
            "VisibilityTimeout": "60",
            "MessageRetentionPeriod": "345600",
        },
    })
}
"#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        let call = method_calls
            .iter()
            .find(|c| c.name == "CreateQueue")
            .expect("Should find CreateQueue call");

        let metadata = call.metadata.as_ref().expect("Should have metadata");

        // Check struct literal parameter
        if let Parameter::Positional { struct_fields, .. } = &metadata.parameters[1] {
            assert!(struct_fields.is_some());
            let fields = struct_fields.as_ref().unwrap();

            // Should only extract top-level fields
            assert_eq!(fields.len(), 2);
            assert!(fields.contains(&"QueueName".to_string()));
            assert!(fields.contains(&"Attributes".to_string()));

            // Should NOT contain nested map keys
            assert!(!fields.contains(&"VisibilityTimeout".to_string()));
            assert!(!fields.contains(&"MessageRetentionPeriod".to_string()));
        } else {
            panic!("Expected Positional parameter");
        }
    }

    /// Test that JSON strings in payloads are not parsed as struct fields
    #[tokio::test]
    async fn test_extraction_json_payload_not_parsed() {
        let extractor = GoExtractor::new();

        let code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/lambda"
    "github.com/aws/aws-sdk-go-v2/aws"
)

func test() {
    client := lambda.NewFromConfig(cfg)
    result, _ := client.Invoke(context.TODO(), &lambda.InvokeInput{
        FunctionName: aws.String("my-function"),
        Payload:      []byte(`{"action": "process", "data": {"id": 123}}`),
    })
}
"#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        let call = method_calls
            .iter()
            .find(|c| c.name == "Invoke")
            .expect("Should find Invoke call");

        let metadata = call.metadata.as_ref().expect("Should have metadata");

        // Check struct literal parameter
        if let Parameter::Positional { struct_fields, .. } = &metadata.parameters[1] {
            assert!(struct_fields.is_some());
            let fields = struct_fields.as_ref().unwrap();

            // Should only extract Go struct fields, not JSON keys
            assert_eq!(fields.len(), 2);
            assert!(fields.contains(&"FunctionName".to_string()));
            assert!(fields.contains(&"Payload".to_string()));

            // Should NOT extract JSON keys from the string literal
            assert!(!fields.contains(&"action".to_string()));
            assert!(!fields.contains(&"data".to_string()));
            assert!(!fields.contains(&"id".to_string()));
        } else {
            panic!("Expected Positional parameter");
        }
    }

    /// Test that empty struct literals result in empty field list
    #[tokio::test]
    async fn test_extraction_empty_struct() {
        let extractor = GoExtractor::new();

        let code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/sts"
)

func test() {
    client := sts.NewFromConfig(cfg)
    result, _ := client.GetCallerIdentity(context.TODO(), &sts.GetCallerIdentityInput{})
}
"#;

        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Go);
        let result = extractor.parse(&source_file).await;
        let method_calls = result.method_calls_ref();

        let call = method_calls
            .iter()
            .find(|c| c.name == "GetCallerIdentity")
            .expect("Should find GetCallerIdentity call");

        let metadata = call.metadata.as_ref().expect("Should have metadata");

        // Check struct literal parameter
        if let Parameter::Positional { struct_fields, .. } = &metadata.parameters[1] {
            assert!(
                struct_fields.is_some(),
                "struct_fields should be Some even for empty struct"
            );
            let fields = struct_fields.as_ref().unwrap();
            assert_eq!(fields.len(), 0, "Should have 0 fields for empty struct");
        } else {
            panic!("Expected Positional parameter");
        }
    }
}
