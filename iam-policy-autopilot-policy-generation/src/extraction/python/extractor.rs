//! SDK method extraction for Python using ast-grep

use async_trait::async_trait;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Python;
use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::python::common::ArgumentExtractor;
use crate::extraction::python::disambiguation::MethodDisambiguator;
use crate::extraction::python::paginator_extractor::PaginatorExtractor;
use crate::extraction::python::waiters_extractor::WaitersExtractor;
use crate::extraction::python::resource_direct_calls_extractor::ResourceDirectCallsExtractor;
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata};
use crate::ServiceModelIndex;

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
    ) -> Option<SdkMethodCall> {
        let env = node_match.get_env();
        
        // Extract the receiver (object before the dot)
        let receiver = env.get_match("OBJ").map(|obj_node| obj_node.text().to_string());

        // Extract the method name
        let method_name = if let Some(method_node) = env.get_match("METHOD") {
            method_node.text()
        } else {
            return None;
        };

        // Extract arguments - get_multiple_matches returns Vec<Node> directly
        let args_nodes = env.get_multiple_matches("ARGS");
        let arguments = ArgumentExtractor::extract_arguments(&args_nodes);

        // Get position information
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();
        
        let method_call = SdkMethodCall {
            name: method_name.to_string(),
            possible_services: Vec::new(), // Will be determined later during service validation
            metadata: Some(SdkMethodCallMetadata {
                parameters: arguments,
                return_type: None, // We don't know the return type from the call site
                start_position: (start.line() + 1, start.column(node) + 1),
                end_position: (end.line() + 1, end.column(node) + 1),
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
    async fn parse(&self, source_code: &str) -> crate::extraction::extractor::ExtractorResult {
        let ast_grep = Python.ast_grep(source_code);
        let root = ast_grep.root();
        
        let mut method_calls = Vec::new();
        
        let pattern = "$OBJ.$METHOD($$$ARGS)";
        
        // Find all method calls with attribute access: obj.method(args)
        for node_match in root.find_all(pattern) {
            if let Some(method_call) = self.parse_method_call(&node_match) {
                method_calls.push(method_call);
            }
        }

        ExtractorResult::Python(ast_grep, method_calls)
    }

    fn filter_map(&self, extractor_results: &mut [ExtractorResult], service_index: &ServiceModelIndex) {
        let method_disambiguator = MethodDisambiguator::new(service_index);
        
        for extractor_result in extractor_results.iter_mut() {
            match extractor_result {
                ExtractorResult::Python(ast, method_calls) => {
                    // Extract resource direct calls (with ServiceModelIndex access)
                    let resource_extractor = ResourceDirectCallsExtractor::new(
                        service_index,
                    );
                    let resource_calls = resource_extractor.extract_resource_method_calls(ast);
                    method_calls.extend(resource_calls);
                    
                    // Add waiters to extracted methods using the service model index directly
                    let waiters_extractor = WaitersExtractor::new(service_index);
                    let waiter_calls = waiters_extractor.extract_waiter_method_calls(ast, service_index);
                    method_calls.extend(waiter_calls);
                    
                    // Add paginators to extracted methods using the service model index directly
                    let paginator_extractor = PaginatorExtractor::new(service_index);
                    let paginator_calls = paginator_extractor.extract_paginate_method_calls(ast);
                    method_calls.extend(paginator_calls);
                    
                    // Clone the method calls to pass to disambiguate_method_calls
                    let filtered_and_mapped = method_disambiguator.disambiguate_method_calls(method_calls.clone());
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

    fn disambiguate(&self, _extractor_result: &mut [ExtractorResult], _service_index: &ServiceModelIndex) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_method_call_extraction() {
        let extractor = PythonExtractor::new();
        let source_code = "s3_client.get_object(Bucket='my-bucket', Key='my-key')";
        
        let result = extractor.parse(source_code).await;
        assert_eq!(result.method_calls_ref().len(), 1);
        assert_eq!(result.method_calls_ref()[0].name, "get_object");
    }
}
