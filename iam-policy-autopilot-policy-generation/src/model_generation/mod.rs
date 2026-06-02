use std::collections::HashSet;

use crate::extraction::call_graph::{CallGraph, FunctionNode};
use crate::extraction::external_library_models::{
    CallPattern, CallType, ExternalLibraryModel, SdkOperationMapping,
};
use crate::extraction::SdkMethodCall;
use crate::Language;

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
    ) -> ExternalLibraryModel {
        let partitioned = call_graph.partition_calls(entry_points, sdk_calls);

        ExternalLibraryModel {
            library_name: library_name.to_string(),
            language,
            version: None,
            call_patterns: partitioned
                .into_iter()
                .filter(|(_, calls)| !calls.is_empty())
                .map(|(func, calls)| self.build_call_pattern(&func, &calls))
                .collect(),
        }
    }

    fn build_call_pattern(&self, func: &FunctionNode, calls: &[SdkMethodCall]) -> CallPattern {
        let (module_path, class_name, function_name) = parse_function_name(&func.name);

        CallPattern {
            module_path,
            class_name: class_name.clone(),
            function_name,
            call_type: if class_name.is_some() {
                CallType::InstanceMethod
            } else {
                CallType::Function
            },
            sdk_operations: deduplicate_operations(calls),
        }
    }
}

/// Parse a gopls function name into (module_path, class_name, function_name).
///
/// gopls uses these formats for `FunctionNode.name`:
///   - Functions: `"main"`, `"helper"`, `"fetchData"`
///   - Methods:   `"(*Server).HandleRequest"`, `"(*Server).fetchData"`
fn parse_function_name(name: &str) -> (String, Option<String>, String) {
    // Method pattern: "(*Type).Method" or "(Type).Method"
    if let Some(dot_pos) = name.find(").") {
        let receiver_part = &name[..=dot_pos];
        let method_name = &name[dot_pos + 2..];

        let type_name = receiver_part
            .trim_start_matches('(')
            .trim_start_matches('*')
            .trim_end_matches(')');

        return (
            String::new(),
            Some(type_name.to_string()),
            method_name.to_string(),
        );
    }

    (String::new(), None, name.to_string())
}

fn deduplicate_operations(calls: &[SdkMethodCall]) -> Vec<SdkOperationMapping> {
    let mut seen = HashSet::new();
    let mut ops = Vec::new();

    for call in calls {
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

    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::call_graph::CallGraph;
    use crate::extraction::SdkMethodCallMetadata;
    use crate::Location;
    use rstest::rstest;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // Reuse the graph_from_spec pattern from call_graph tests
    fn graph_from_spec(edge_specs: &[&str]) -> CallGraph {
        let mut all_names = Vec::new();
        let mut edge_pairs: Vec<(&str, &str)> = Vec::new();

        for spec in edge_specs {
            let parts: Vec<&str> = spec.split("->").collect();
            let caller = parts[0].trim();
            if !all_names.contains(&caller) {
                all_names.push(caller);
            }
            if parts.len() == 2 {
                for callee in parts[1].split(',') {
                    let callee = callee.trim();
                    if !all_names.contains(&callee) {
                        all_names.push(callee);
                    }
                    edge_pairs.push((caller, callee));
                }
            }
        }

        let nodes: Vec<FunctionNode> = all_names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let start_line = i * 10 + 1;
                FunctionNode {
                    name: name.to_string(),
                    qualified_name: None,
                    location: Location::new(
                        PathBuf::from("test.go"),
                        (start_line, 1),
                        (start_line + 9, 1),
                    ),
                }
            })
            .collect();

        let mut edges: HashMap<FunctionNode, Vec<FunctionNode>> = HashMap::new();
        for (caller_name, callee_name) in edge_pairs {
            let caller = nodes
                .iter()
                .find(|n| n.name == caller_name)
                .unwrap()
                .clone();
            let callee = nodes
                .iter()
                .find(|n| n.name == callee_name)
                .unwrap()
                .clone();
            edges.entry(caller).or_default().push(callee);
        }

        CallGraph::new(nodes, edges)
    }

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
    // parse_function_name
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("main", "", None, "main")]
    #[case("helper", "", None, "helper")]
    #[case("(*Server).HandleRequest", "", Some("Server"), "HandleRequest")]
    #[case("(*Server).fetchData", "", Some("Server"), "fetchData")]
    #[case("(Server).Method", "", Some("Server"), "Method")]
    fn test_parse_function_name(
        #[case] input: &str,
        #[case] expected_module: &str,
        #[case] expected_class: Option<&str>,
        #[case] expected_func: &str,
    ) {
        let (module_path, class_name, function_name) = parse_function_name(input);
        assert_eq!(module_path, expected_module);
        assert_eq!(class_name.as_deref(), expected_class);
        assert_eq!(function_name, expected_func);
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
        let model = engine.generate(&graph, &entries, &[call], "lib", Language::Go);

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
        let model = engine.generate(&graph, &[handler.clone()], &[call], "lib", Language::Go);

        assert_eq!(model.call_patterns.len(), 1);
        let pattern = &model.call_patterns[0];
        assert_eq!(pattern.function_name, "HandleRequest");
        assert_eq!(pattern.class_name, Some("Server".to_string()));
        assert_eq!(pattern.call_type, CallType::InstanceMethod);
    }
}
