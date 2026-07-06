pub(crate) mod language_conventions;

use std::collections::HashSet;

use crate::extraction::call_graph::{CallGraph, FunctionNode};
use crate::extraction::external_library_models::{
    CallPattern, CallType, ExternalLibraryModel, SdkOperationMapping,
};
use crate::extraction::SdkMethodCall;
use crate::Language;

use language_conventions::LanguageConventions;

pub(crate) struct Engine;

impl Engine {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Generate an ExternalLibraryModel from call graph + SDK calls.
    ///
    /// Each entry point becomes a `CallPattern` in the output model, mapping
    /// the entry point function to the SDK operations reachable from it.
    pub(crate) fn generate(
        &self,
        call_graph: &CallGraph,
        entry_points: &[FunctionNode],
        sdk_calls: &[SdkMethodCall],
        library_name: &str,
        language: Language,
        conventions: &dyn LanguageConventions,
    ) -> ExternalLibraryModel {
        let partitioned = call_graph.partition_calls(entry_points, sdk_calls);

        ExternalLibraryModel {
            library_name: library_name.to_string(),
            language,
            version: None,
            call_patterns: partitioned
                .into_iter()
                .filter(|(_, calls)| !calls.is_empty())
                .map(|(func, calls)| self.build_call_pattern(&func, &calls, conventions))
                .collect(),
        }
    }

    fn build_call_pattern(
        &self,
        func: &FunctionNode,
        calls: &[SdkMethodCall],
        conventions: &dyn LanguageConventions,
    ) -> CallPattern {
        let parsed = conventions.parse_function_name(func);

        CallPattern {
            module_path: parsed.module_path,
            class_name: parsed.class_name.clone(),
            function_name: parsed.function_name,
            call_type: if parsed.class_name.is_some() {
                CallType::InstanceMethod
            } else {
                CallType::Function
            },
            sdk_operations: deduplicate_operations(calls),
        }
    }
}

fn deduplicate_operations(calls: &[SdkMethodCall]) -> Vec<SdkOperationMapping> {
    let mut seen = HashSet::new();
    let mut ops = Vec::new();

    for call in calls {
        if call.possible_services.len() > 1 {
            log::warn!(
                "Ambiguous SDK call '{}' could belong to multiple services: {:?}. \
                 Use --service-hints to specify which services your code uses.",
                call.name,
                call.possible_services
            );
        }
        for service in &call.possible_services {
            let key = (service.as_str(), call.name.as_str());
            if seen.insert(key) {
                ops.push(SdkOperationMapping {
                    service: service.clone(),
                    operation: call.name.clone(),
                });
            }
        }
    }

    // Sort for deterministic output: SDK calls are collected in call-graph
    // traversal order, which is not stable across runs, so an unsorted list
    // makes regenerated models differ only by ordering.
    ops.sort();

    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::call_graph::{graph_from_spec, CallGraph};
    use crate::extraction::SdkMethodCallMetadata;
    use crate::Location;
    use language_conventions::GoConventions;
    use std::path::PathBuf;

    fn sdk_call_in(
        graph: &CallGraph,
        operation: &str,
        service: &str,
        function_name: &str,
    ) -> SdkMethodCall {
        let node = graph
            .nodes()
            .iter()
            .find(|n| n.name == function_name)
            .unwrap();
        let line = node.location.start_line() + 1;
        SdkMethodCall {
            name: operation.to_string(),
            possible_services: vec![service.to_string()],
            metadata: Some(SdkMethodCallMetadata::new(
                format!("client.{operation}(...)"),
                Location::new(node.location.file_path.clone(), (line, 5), (line, 15)),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // generate
    // -----------------------------------------------------------------------

    #[test]
    fn test_generate_basic() {
        let graph = graph_from_spec(&["handler -> fetch", "fetch"]);
        let call = sdk_call_in(&graph, "GetObject", "s3", "fetch");

        let engine = Engine::new();
        let model = engine.generate(
            &graph,
            &[graph
                .nodes()
                .iter()
                .find(|n| n.name == "handler")
                .unwrap()
                .clone()],
            &[call],
            "my-lib",
            Language::Go,
            &GoConventions,
        );

        assert_eq!(model.library_name, "my-lib");
        assert_eq!(model.language, Language::Go);
        assert_eq!(model.call_patterns.len(), 1);

        let pattern = &model.call_patterns[0];
        assert_eq!(pattern.function_name, "handler");
        assert_eq!(pattern.call_type, CallType::Function);
        assert_eq!(pattern.class_name, None);
        assert_eq!(pattern.sdk_operations.len(), 1);
        assert_eq!(pattern.sdk_operations[0].service, "s3");
        assert_eq!(pattern.sdk_operations[0].operation, "GetObject");
    }

    #[test]
    fn test_generate_filters_empty_entry_points() {
        let graph = graph_from_spec(&["handler", "other"]);
        let call = sdk_call_in(&graph, "GetObject", "s3", "other");

        let engine = Engine::new();
        let model = engine.generate(
            &graph,
            &[graph
                .nodes()
                .iter()
                .find(|n| n.name == "handler")
                .unwrap()
                .clone()],
            &[call],
            "lib",
            Language::Go,
            &GoConventions,
        );

        assert!(model.call_patterns.is_empty());
    }

    #[test]
    fn test_generate_multiple_entry_points() {
        let graph = graph_from_spec(&["a -> shared", "b -> shared", "shared"]);
        let call = sdk_call_in(&graph, "PutItem", "dynamodb", "shared");

        let entries: Vec<FunctionNode> = ["a", "b"]
            .iter()
            .map(|name| {
                graph
                    .nodes()
                    .iter()
                    .find(|n| &n.name == name)
                    .unwrap()
                    .clone()
            })
            .collect();

        let engine = Engine::new();
        let model = engine.generate(
            &graph,
            &entries,
            &[call],
            "lib",
            Language::Go,
            &GoConventions,
        );

        assert_eq!(model.call_patterns.len(), 2);
        assert_eq!(
            model.call_patterns[0].sdk_operations[0].operation,
            "PutItem"
        );
        assert_eq!(
            model.call_patterns[1].sdk_operations[0].operation,
            "PutItem"
        );
    }

    #[test]
    fn test_generate_deduplicates_operations() {
        let graph = graph_from_spec(&["handler -> a, b", "a", "b"]);
        let call_a = sdk_call_in(&graph, "GetObject", "s3", "a");
        let call_b = sdk_call_in(&graph, "GetObject", "s3", "b");

        let engine = Engine::new();
        let model = engine.generate(
            &graph,
            &[graph
                .nodes()
                .iter()
                .find(|n| n.name == "handler")
                .unwrap()
                .clone()],
            &[call_a, call_b],
            "lib",
            Language::Go,
            &GoConventions,
        );

        assert_eq!(model.call_patterns.len(), 1);
        assert_eq!(model.call_patterns[0].sdk_operations.len(), 1);
    }

    #[test]
    fn test_generate_method_receiver() {
        let graph = graph_from_spec(&["(*Server).HandleRequest -> fetch", "fetch"]);
        let nodes = graph.nodes().to_vec();
        let handler = nodes
            .iter()
            .find(|n| n.name == "(*Server).HandleRequest")
            .unwrap();

        let call = sdk_call_in(&graph, "GetObject", "s3", "fetch");

        let engine = Engine::new();
        let model = engine.generate(
            &graph,
            &[handler.clone()],
            &[call],
            "lib",
            Language::Go,
            &GoConventions,
        );

        assert_eq!(model.call_patterns.len(), 1);
        let pattern = &model.call_patterns[0];
        assert_eq!(pattern.function_name, "HandleRequest");
        assert_eq!(pattern.class_name, Some("Server".to_string()));
        assert_eq!(pattern.call_type, CallType::InstanceMethod);
    }
}
