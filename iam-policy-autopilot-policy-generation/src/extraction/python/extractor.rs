//! SDK method extraction for Python using ast-grep

use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::python::common::ArgumentExtractor;
use crate::extraction::python::disambiguation::MethodDisambiguator;
use crate::extraction::python::paginator_extractor::PaginatorExtractor;
use crate::extraction::python::resource_direct_calls_extractor::ResourceDirectCallsExtractor;
use crate::extraction::python::waiters_extractor::WaitersExtractor;
use crate::extraction::{AstWithSourceFile, SdkMethodCall, SdkMethodCallMetadata};
use crate::{Location, ServiceModelIndex, SourceFile};
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Python;
use async_trait::async_trait;

pub(crate) struct PythonExtractor;

impl PythonExtractor {
    /// Create a new Python extractor instance
    pub(crate) fn new() -> Self {
        Self
    }

    /// Parse a single method call match into a SdkMethodCall
    fn parse_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Python>>,
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

        // Extract arguments - get_multiple_matches returns Vec<Node> directly
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        let method_call = SdkMethodCall {
            name: method_name.to_string(),
            possible_services: Vec::new(), // Will be determined later during service validation
            metadata: Some(SdkMethodCallMetadata {
                parameters: arguments,
                return_type: None, // We don't know the return type from the call site
                expr: node_match.text().to_string(),
                location: Location::from_node(
                    source_file.path.to_path_buf(),
                    node_match.get_node(),
                ),
                receiver,
            }),
        };
        log::debug!("Found method call: {:?}", method_call);

        Some(method_call)
    }
}

impl Default for PythonExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Extractor for PythonExtractor {
    async fn parse(
        &self,
        source_file: &SourceFile,
    ) -> crate::extraction::extractor::ExtractorResult {
        let ast_grep = Python.ast_grep(&source_file.content);
        let ast = AstWithSourceFile::new(ast_grep, source_file.clone());
        let root = ast.ast.root();

        let mut method_calls = Vec::new();

        let pattern = "$OBJ.$METHOD($$$ARGS)";

        // Find all method calls with attribute access: obj.method(args)
        for node_match in root.find_all(pattern) {
            if let Some(method_call) = self.parse_method_call(&node_match, source_file) {
                method_calls.push(method_call);
            }
        }

        ExtractorResult::Python(ast, method_calls)
    }

    fn filter_map(
        &self,
        extractor_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    ) {
        let method_disambiguator = MethodDisambiguator::new(service_index);
        let resource_extractor = ResourceDirectCallsExtractor::new(service_index);
        let waiters_extractor = WaitersExtractor::new(service_index);
        let paginator_extractor = PaginatorExtractor::new(service_index);

        for extractor_result in extractor_results.iter_mut() {
            match extractor_result {
                ExtractorResult::Python(ast, method_calls) => {
                    // Extract resource direct calls (with ServiceModelIndex access)
                    let resource_calls = resource_extractor.extract_resource_method_calls(ast);
                    method_calls.extend(resource_calls);

                    // Add waiters to extracted methods using the service model index directly
                    let waiter_calls = waiters_extractor.extract_waiter_method_calls(ast);
                    method_calls.extend(waiter_calls);

                    // Add paginators to extracted methods using the service model index directly
                    let paginator_calls = paginator_extractor.extract_paginate_method_calls(ast);
                    method_calls.extend(paginator_calls);

                    // Clone the method calls to pass to disambiguate_method_calls
                    let filtered_and_mapped =
                        method_disambiguator.disambiguate_method_calls(method_calls.clone());
                    // Replace the method calls in place
                    *method_calls = filtered_and_mapped;
                }
                ExtractorResult::Go(_, _, _) => {
                    // This shouldn't happen in Python extractor, but handle gracefully
                    panic!("Received Go result during Python method extraction.")
                }
                ExtractorResult::JavaScript(_, _) => {
                    // This shouldn't happen in Python extractor, but handle gracefully
                    panic!("Received JavaScript result during Python method extraction.")
                }
                ExtractorResult::TypeScript(_, _) => {
                    // This shouldn't happen in Python extractor, but handle gracefully
                    panic!("Received TypeScript result during Python method extraction.")
                }
            }
        }
    }

    fn disambiguate(
        &self,
        _extractor_result: &mut [ExtractorResult],
        _service_index: &ServiceModelIndex,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        extraction::{Parameter, ParameterValue},
        Language,
    };

    #[tokio::test]
    async fn test_basic_method_call_extraction() {
        let extractor = PythonExtractor::new();
        let source_code = "s3_client.get_object(Bucket='my-bucket', Key='my-key')";

        let source_file = SourceFile::with_language(
            std::path::PathBuf::new(),
            source_code.to_string(),
            crate::Language::Python,
        );
        let result = extractor.parse(&source_file).await;
        assert_eq!(result.method_calls_ref().len(), 1);
        assert_eq!(result.method_calls_ref()[0].name, "get_object");
    }

    #[tokio::test]
    async fn test_method_call_with_comments() {
        let extractor = PythonExtractor::new();
        // Test case with inline comments in arguments
        let source_code = r#"
cloudwatch_client.put_metric_alarm(
    AlarmName='test-alarm',
    # This is a comment that should be ignored
    ComparisonOperator='GreaterThanThreshold',
    EvaluationPeriods=1 # This comment should be ignored too
    ,
    # Another comment
    MetricName='Errors',
    Threshold=0.0
)
"#;
        let source_file =
            SourceFile::with_language(PathBuf::new(), source_code.to_string(), Language::Python);
        let result = extractor.parse(&source_file).await;

        // Verify exactly one method call is extracted
        assert_eq!(result.method_calls_ref().len(), 1);

        let method_call = &result.method_calls_ref()[0];
        assert_eq!(method_call.name, "put_metric_alarm");

        // Verify the metadata contains the correct number of parameters
        // Comments should NOT be counted as parameters
        if let Some(metadata) = &method_call.metadata {
            let param_count = metadata.parameters.len();
            // Should have 5 parameters (i.e., the 2 line comments and the comment
            // appended after the argument EvaluationPeriods should not affect extraction)
            assert_eq!(
                param_count, 5,
                "Expected 5 parameters (excluding comments), but got {}",
                param_count
            );

            // Verify all parameters are keyword arguments (not comments)
            for param in &metadata.parameters {
                match param {
                    Parameter::Keyword { name, .. } => {
                        // Ensure parameter names are actual parameter names, not comments
                        assert!(
                            !name.starts_with('#'),
                            "Parameter name should not start with #: {}",
                            name
                        );
                    }
                    _ => {
                        // All parameters in this test should be keyword arguments
                        panic!("Expected all parameters to be Keyword arguments");
                    }
                }
            }

            // Specifically verify that EvaluationPeriods doesn't include the trailing comment
            let eval_periods_param = metadata.parameters.iter().find(
                |p| matches!(p, Parameter::Keyword { name, .. } if name == "EvaluationPeriods"),
            );

            assert!(
                eval_periods_param.is_some(),
                "Expected to find EvaluationPeriods parameter"
            );

            if let Some(Parameter::Keyword { name: _, value, .. }) = eval_periods_param {
                match value {
                    ParameterValue::Unresolved(val) => {
                        assert_eq!(
                            val, "1",
                            "EvaluationPeriods value should be '1', not '{}'",
                            val
                        );
                        assert!(
                            !val.contains('#'),
                            "Parameter value '{}' should not contain comment character",
                            val
                        );
                    }
                    _ => panic!("Expected EvaluationPeriods to have an Unresolved value"),
                }
            }
        } else {
            panic!("Expected metadata to be present");
        }
    }
}
