//! Feature method extraction for Go AWS SDK v2
//!
//! This module handles extraction of Go AWS SDK v2 feature methods like S3 Upload/Download,
//! and other specialized SDK features.

use crate::extraction::go::features::{FeatureMethod, GoSdkV2Features};
use crate::extraction::go::types::GoImportInfo;
use crate::extraction::go::utils;
use crate::extraction::{AstWithSourceFile, SdkMethodCall, SdkMethodCallMetadata};
use crate::Location;
use ast_grep_config::from_yaml_string;
use ast_grep_language::Go;

/// Information about a discovered feature method call
#[derive(Debug, Clone)]
pub(crate) struct FeatureCallInfo {
    /// Method or function name (e.g., "Upload")
    pub(crate) method_name: String,
    /// Receiver variable name for methods (e.g., "uploader"), None for package functions
    pub(crate) receiver: Option<String>,
    /// Extracted arguments
    pub(crate) arguments: Vec<crate::extraction::Parameter>,
    /// Matched expression
    pub(crate) expr: String,
    /// Location of the call
    pub(crate) location: Location,
}

/// Extractor for Go AWS SDK v2 feature methods
///
/// This extractor discovers feature method calls in Go code and creates synthetic
/// SdkMethodCall objects that represent the actual AWS operations required.
pub(crate) struct GoFeaturesExtractor {
    features: &'static GoSdkV2Features,
}

impl GoFeaturesExtractor {
    /// Create a new Go features extractor
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let features = GoSdkV2Features::load()?;
        Ok(Self { features })
    }

    /// Extract feature method calls from the AST
    pub(crate) fn extract_feature_method_calls(
        &self,
        ast: &AstWithSourceFile<Go>,
        import_info: &mut GoImportInfo,
    ) -> Vec<SdkMethodCall> {
        let mut synthetic_calls = Vec::new();

        log::debug!("import_info: {import_info:?}");

        // Find all method calls (receiver.Method(...))
        let method_calls = self.find_method_calls(ast);
        for call_info in method_calls {
            if let Some(calls) = self.create_synthetic_calls_from_method(&call_info, import_info) {
                synthetic_calls.extend(calls);
            }
        }

        synthetic_calls
    }

    /// Find all method calls that might be feature methods
    /// This matches receiver.Method(...) patterns using proper ast-grep config
    fn find_method_calls(&self, ast: &AstWithSourceFile<Go>) -> Vec<FeatureCallInfo> {
        let root = ast.ast.root();
        let mut calls = Vec::new();

        // Use the same pattern as the main extractor for method calls
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

        for node_match in root.find_all(&config.matcher) {
            let env = node_match.get_env();

            // Get receiver and method name
            let receiver = env.get_match("OBJ").map(|n| n.text().to_string());
            let method_name = env.get_match("METHOD").map(|n| n.text().to_string());

            if let (Some(receiver), Some(method_name)) = (receiver, method_name) {
                // Extract arguments using the same approach as the main extractor
                let args_nodes = env.get_multiple_matches("ARGS");
                let arguments = utils::extract_arguments(&args_nodes);

                calls.push(FeatureCallInfo {
                    method_name,
                    receiver: Some(receiver),
                    arguments,
                    expr: node_match.text().to_string(),
                    location: Location::from_node(
                        ast.source_file.path.clone(),
                        node_match.get_node(),
                    ),
                });
            }
        }

        calls
    }

    /// Create synthetic calls from a method call
    fn create_synthetic_calls_from_method(
        &self,
        call_info: &FeatureCallInfo,
        import_info: &mut GoImportInfo,
    ) -> Option<Vec<SdkMethodCall>> {
        // Check if this method name matches any feature method
        for (service_name, service_features) in &self.features.services {
            if let Some(feature) = service_features.get(&call_info.method_name) {
                // Check if the import matches
                if !self.is_import_match(service_name, &feature.import, import_info) {
                    continue;
                }

                // Create synthetic calls for all operations
                return Some(self.create_synthetic_calls(service_name, feature, call_info));
            }
        }

        None
    }

    /// Check if an import path matches the feature's import
    fn is_import_match(
        &self,
        service_name: &str,
        feature_import: &str,
        import_info: &mut GoImportInfo,
    ) -> bool {
        // Check if any import matches the feature's import path
        for import in &mut import_info.imports {
            if import.original_name == feature_import {
                // Set the service name on the import
                // TODO: Refactor this, we should not need to mutate GoImportInfo here, but parsing the imports
                //      is done far away and we don't have the feature imports available there currently.
                //      We might not need this, I think s3/manager always comes with a regular s3 import for
                //      creating the s3.GetObjectInput (or similar).
                import.service_name = Some(service_name.to_string());
                // Also update the service_mappings HashMap so get_imported_services() can find it
                import_info
                    .service_mappings
                    .insert(import.local_name.clone(), service_name.to_string());
                return true;
            }
        }
        false
    }

    /// Create synthetic SDK method calls from a feature
    fn create_synthetic_calls(
        &self,
        service_name: &str,
        feature: &FeatureMethod,
        call_info: &FeatureCallInfo,
    ) -> Vec<SdkMethodCall> {
        // Create synthetic required arguments if none were extracted
        let parameters = if call_info.arguments.is_empty() {
            // Add synthetic required arguments based on min_arguments
            (0..feature.min_arguments)
                .map(|i| crate::extraction::Parameter::Positional {
                    value: crate::extraction::ParameterValue::Unresolved("synthetic".to_string()),
                    position: i,
                    type_annotation: None,
                    struct_fields: None,
                })
                .collect()
        } else {
            call_info.arguments.clone()
        };

        // Create one synthetic call per operation
        // For IAM actions with service prefix matching the feature service (e.g., "s3:PutObject" for service "s3"),
        // strip the prefix to get the operation name ("PutObject") so it can be found in service models.
        feature
            .operations
            .iter()
            .map(|operation| {
                let operation_name = if let Some(colon_pos) = operation.find(':') {
                    operation[colon_pos + 1..].to_string()
                } else {
                    // No colon, use as-is
                    operation.clone()
                };

                log::debug!(
                    "Feature {}\n  Creating SdkMethodCall for {} (from {})",
                    feature.method_name,
                    operation_name,
                    operation
                );

                let metadata =
                    SdkMethodCallMetadata::new(call_info.expr.clone(), call_info.location.clone())
                        .with_parameters(parameters.clone());
                let metadata = if let Some(r) = call_info.receiver.clone() {
                    metadata.with_receiver(r)
                } else {
                    metadata
                };

                SdkMethodCall {
                    name: operation_name,
                    possible_services: vec![service_name.to_string()],
                    metadata: Some(metadata),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{Language, SourceFile};

    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::Go;

    fn create_test_ast(source_code: &str) -> AstWithSourceFile<Go> {
        let source_file =
            SourceFile::with_language(PathBuf::new(), source_code.to_string(), Language::Go);
        let ast_grep = Go.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file)
    }

    fn create_test_import_info() -> GoImportInfo {
        let mut import_info = GoImportInfo::new();

        import_info.add_import(crate::extraction::go::types::ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/service/s3".to_string(),
            "s3".to_string(),
            1,
        ));
        import_info.add_import(crate::extraction::go::types::ImportInfo::new(
            "github.com/aws/aws-sdk-go-v2/feature/s3/manager".to_string(),
            "manager".to_string(),
            2,
        ));

        import_info
    }

    #[tokio::test]
    async fn test_s3_uploader_upload() {
        let extractor = GoFeaturesExtractor::new().expect("Failed to create extractor");
        // This mocks what we get from the import parser (it gets the ../manager import wrong)
        let mut import_info = create_test_import_info();

        let source_code = r#"
package main

import (
    "context"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/feature/s3/manager"
)

func main() {
    uploader := manager.NewUploader(client)
    result, err := uploader.Upload(context.TODO(), &s3.PutObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("my-key"),
    })
}
"#;

        let ast = create_test_ast(source_code);
        let calls = extractor.extract_feature_method_calls(&ast, &mut import_info);

        println!("Found {} calls", calls.len());
        for call in &calls {
            println!("  - {} (service: {:?})", call.name, call.possible_services);
        }

        assert!(!calls.is_empty(), "Should extract Upload call");

        // Verify we have all 5 expected S3 operations from the JSON config
        let operation_names: Vec<_> = calls.iter().map(|c| c.name.as_str()).collect();
        let expected_operations = vec![
            "PutObject",
            "CreateMultipartUpload",
            "UploadPart",
            "CompleteMultipartUpload",
            "AbortMultipartUpload",
        ];

        for expected_op in &expected_operations {
            assert!(
                operation_names.contains(expected_op),
                "Should have operation '{}', found: {:?}",
                expected_op,
                operation_names
            );
        }

        assert_eq!(
            calls.len(),
            expected_operations.len(),
            "Should have exactly {} operations",
            expected_operations.len()
        );

        // Assert that each operation has exactly ["s3"] as possible_services BEFORE disambiguation
        for call in &calls {
            assert_eq!(
                call.possible_services,
                vec!["s3"],
                "Operation '{}' should have exactly ['s3'] as possible_services before disambiguation, got: {:?}",
                call.name,
                call.possible_services
            );
        }

        // Test that these operations survive disambiguation
        // Load the service index for Go
        let service_index =
            crate::extraction::sdk_model::ServiceDiscovery::load_service_index(crate::Language::Go)
                .await
                .expect("Failed to load service index");

        let disambiguator =
            crate::extraction::go::disambiguation::GoMethodDisambiguator::new(&service_index);

        let disambiguated_calls =
            disambiguator.disambiguate_method_calls(calls.clone(), Some(&import_info));

        println!("After disambiguation: {} calls", disambiguated_calls.len());
        for call in &disambiguated_calls {
            println!("  - {} (service: {:?})", call.name, call.possible_services);
        }

        // All operations should survive disambiguation since they're valid S3 operations
        assert_eq!(
            disambiguated_calls.len(),
            expected_operations.len(),
            "All operations should survive disambiguation"
        );

        // Verify all expected operations are still present after disambiguation
        let disambiguated_names: Vec<_> = disambiguated_calls
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for expected_op in &expected_operations {
            assert!(
                disambiguated_names.contains(expected_op),
                "Operation '{}' should survive disambiguation, found: {:?}",
                expected_op,
                disambiguated_names
            );
        }

        // Assert that each operation has exactly ["s3"] as possible_services AFTER disambiguation
        for call in &disambiguated_calls {
            assert_eq!(
                call.possible_services,
                vec!["s3"],
                "Operation '{}' should have exactly ['s3'] as possible_services after disambiguation, got: {:?}",
                call.name,
                call.possible_services
            );
        }
    }
}
