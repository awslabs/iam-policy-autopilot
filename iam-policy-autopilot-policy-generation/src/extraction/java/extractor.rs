use ast_grep_config::from_yaml_string;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Java;
use async_trait::async_trait;
use convert_case::Case;
use convert_case::Casing;
use std::collections::HashSet;

use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::AstWithSourceFile;
use crate::SourceFile;

/// Java extractor for AWS SDK method calls
pub(crate) struct JavaExtractor {}

impl JavaExtractor {
    /// Create a new Java extractor instance
    pub(crate) fn new() -> Self {
        Self {}
    }

    fn parse_method_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Java>>,
    ) -> Option<crate::SdkMethodCall> {
        let env = node_match.get_env();

        // Extract the receiver (object before the dot)
        let _receiver = env.get_match("OBJ").map(|n| n.text().to_string());

        // Extract the method name
        let method_name = if let Some(method_node) = env.get_match("METHOD") {
            method_node.text()
        } else {
            return None;
        };

        let _arguments = if let Some(args_node) = env.get_match("ARGS") {
            args_node
                .children()
                .filter(|child| child.kind() != "," && child.kind() != "(" && child.kind() != ")")
                .map(|arg_node| arg_node.text().to_string())
                .collect::<Vec<String>>()
        } else {
            Vec::new()
        };

        // Extract method name from function name (capitalize and remove "Paginator" suffix)
        // e.g., listBucketsPaginator -> ListBuckets
        let method_name = method_name
            .strip_suffix("Paginator")
            .unwrap_or(&method_name)
            .to_string()
            .to_case(Case::Pascal);

        Some(crate::SdkMethodCall {
            name: method_name,
            possible_services: Vec::new(), // Will be determined later during service validation
            metadata: None,
        })
    }
}

#[async_trait]
impl Extractor for JavaExtractor {
    async fn parse(&self, source_file: &SourceFile) -> ExtractorResult {
        let ast_grep = Java.ast_grep(&source_file.content);
        let ast = AstWithSourceFile::new(ast_grep, source_file.clone());
        let root = ast.ast.root();
        let mut method_calls = Vec::new();
        let config = r#"
id: method_call_extraction
language: Java
rule:
  kind: method_invocation
  all:
    - has:
        field: object
        pattern: $OBJ
    - has:
        field: name
        pattern: $METHOD
        kind: identifier
    - has:
        field: arguments
        pattern: $ARGS
        kind: argument_list
        optional: true
        "#;
        let globals = ast_grep_config::GlobalRules::default();
        let config = &from_yaml_string::<Java>(config, &globals).expect("rule should parse")[0];
        for node_match in root.find_all(&config.matcher) {
            if let Some(method_call) = self.parse_method_call(&node_match) {
                method_calls.push(method_call);
            }
        }
        crate::extraction::extractor::ExtractorResult::Java(ast, method_calls)
    }

    fn filter_map(
        &self,
        extractor_results: &mut [ExtractorResult],
        service_index: &crate::ServiceModelIndex,
    ) {
        for extractor_result in extractor_results.iter_mut() {
            let method_calls = match extractor_result {
                ExtractorResult::Java(_ast, calls) => calls,
                _ => {
                    // This shouldn't happen in Java extractor
                    log::warn!("Received non-Java result during Java method filtering.");
                    continue;
                }
            };

            // First: Resolve waiter names to actual operations
            // For each call, check if it's a waiter name and replace with the actual operation
            for call in method_calls.iter_mut() {
                if let Some(service_methods) = service_index.waiter_lookup.get(&call.name) {
                    let matching_method = service_methods
                        .iter()
                        .find(|sm| call.possible_services.contains(&sm.service_name));
                    if let Some(method) = matching_method {
                        call.name = method.operation_name.clone();
                    } else {
                        log::warn!(
                            "Waiter '{}' found in services {:?} but imported from {:?}",
                            call.name,
                            service_methods
                                .iter()
                                .map(|sm| &sm.service_name)
                                .collect::<Vec<_>>(),
                            call.possible_services
                        );
                    }
                }
            }

            // Second: Validate method calls against service index
            method_calls.retain_mut(|call| {
                // Check if this method name exists in the SDK
                if let Some(service_refs) = service_index.method_lookup.get(&call.name) {
                    // Get valid services for this method from the service index
                    let valid_services: HashSet<String> = service_refs.iter()
                        .map(|service_ref| service_ref.service_name.clone())
                        .collect();

                    // Filter possible_services to only include services that actually contain this method
                    call.possible_services.retain(|service| valid_services.contains(service));

                    // FALLBACK: If no services matched from import, use all valid services for this operation
                    if call.possible_services.is_empty() {
                        log::debug!(
                            "Import-derived service(s) don't contain operation '{}'. Using all {} valid service(s) as fallback.",
                            call.name,
                            valid_services.len()
                        );
                        call.possible_services = valid_services.into_iter().collect();
                    }

                    // Keep method call - it now has at least one valid service
                    true
                } else {
                    // Method name doesn't exist in SDK - filter it out
                    log::warn!("Filtering out {}", call.name);
                    false
                }
            });

            // Then: Deduplicate by (operation_name, service) pairs
            let mut seen = HashSet::new();
            method_calls.retain(|call| {
                // Create a key from operation name and all possible services
                let key = (call.name.clone(), call.possible_services.clone());
                seen.insert(key)
            });
        }
    }

    fn disambiguate(
        &self,
        _extraction_results: &mut [ExtractorResult],
        _service_index: &crate::ServiceModelIndex,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::extraction::extractor::Extractor;

    #[tokio::test]
    async fn test_java_extractor() {
        let code = "s3.listBuckets(request);";
        let source_file =
            SourceFile::with_language(PathBuf::new(), code.to_string(), crate::Language::Java);

        let extractor = JavaExtractor::new();
        let result = extractor.parse(&source_file).await;
        assert_eq!(result.method_calls_ref().len(), 1);
        assert_eq!(result.method_calls_ref()[0].name, "ListBuckets");
    }
}
