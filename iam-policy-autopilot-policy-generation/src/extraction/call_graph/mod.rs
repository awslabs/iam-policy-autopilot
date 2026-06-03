pub(crate) mod gopls;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::extraction::SdkMethodCall;
use crate::Location;

/// A node in the call graph representing a function or method.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FunctionNode {
    pub name: String,
    pub qualified_name: Option<String>,
    pub location: Location,
}

/// Error type for call graph operations.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CallGraphError {
    #[error("LSP error: {0}")]
    Lsp(#[from] crate::lsp::LspError),
    #[error("{0}")]
    Other(String),
}

/// The call graph for a set of source files.
pub(crate) struct CallGraph {
    edges: BTreeMap<FunctionNode, Vec<FunctionNode>>,
    nodes: Vec<FunctionNode>,
}

impl CallGraph {
    pub fn new(nodes: Vec<FunctionNode>, edges: BTreeMap<FunctionNode, Vec<FunctionNode>>) -> Self {
        Self { edges, nodes }
    }

    pub fn nodes(&self) -> &[FunctionNode] {
        &self.nodes
    }

    pub fn outgoing(&self, node: &FunctionNode) -> &[FunctionNode] {
        self.edges.get(node).map_or(&[], Vec::as_slice)
    }

    /// Partition SDK calls by entry point based on call graph reachability.
    ///
    /// Each entry point maps to the SDK calls whose location falls within a function
    /// reachable from that entry point. A call may appear under multiple entry points
    /// if multiple entry points reach the same function.
    pub fn partition_calls(
        &self,
        entry_points: &[FunctionNode],
        sdk_calls: &[SdkMethodCall],
    ) -> Vec<(FunctionNode, Vec<SdkMethodCall>)> {
        let mut results = Vec::new();

        for entry in entry_points {
            let reachable = self.reachable_set(entry);
            let mut calls_for_entry = Vec::new();

            for call in sdk_calls {
                let Some(location) = call.metadata.as_ref().map(|m| &m.location) else {
                    continue;
                };
                if let Some(enclosing) = self.enclosing_function(location) {
                    if reachable.contains(enclosing) {
                        calls_for_entry.push(call.clone());
                    }
                }
            }

            results.push((entry.clone(), calls_for_entry));
        }

        results
    }

    fn enclosing_function(&self, location: &Location) -> Option<&FunctionNode> {
        self.nodes
            .iter()
            .filter(|node| {
                node.location.file_path == location.file_path
                    && node.location.start_position <= location.start_position
                    && node.location.end_position >= location.end_position
            })
            .min_by_key(|node| {
                // Prefer the smallest enclosing range (innermost function)
                (
                    node.location.end_position.0 - node.location.start_position.0,
                    node.location.end_position.1,
                )
            })
    }

    fn reachable_set<'a>(&'a self, from: &'a FunctionNode) -> BTreeSet<&'a FunctionNode> {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();

        visited.insert(from);
        queue.push_back(from);

        while let Some(current) = queue.pop_front() {
            for callee in self.outgoing(current) {
                if visited.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }

        visited
    }
}

/// Trait for building a call graph from source files.
#[async_trait]
pub(crate) trait CallGraphBuilder: Send + Sync {
    /// Build the call graph for the workspace, scoped to the given source files.
    ///
    /// Only function calls where both caller and callee are within `source_files`
    /// appear as edges. Calls to external dependencies are not followed.
    async fn build(
        &mut self,
        workspace_root: &Path,
        source_files: &[PathBuf],
    ) -> Result<CallGraph, CallGraphError>;

    /// Shut down any underlying server/process.
    async fn shutdown(self: Box<Self>) -> Result<(), CallGraphError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    /// Build a CallGraph from a compact edge spec.
    ///
    /// Each string in `edge_specs` has the form `"caller -> callee1, callee2"`.
    /// Nodes are auto-created with non-overlapping ranges in "test.go".
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

        let mut edges: BTreeMap<FunctionNode, Vec<FunctionNode>> = BTreeMap::new();
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

    /// Place an SDK call inside a named function's range.
    fn sdk_call_in(graph: &CallGraph, operation: &str, function_name: &str) -> SdkMethodCall {
        use crate::extraction::SdkMethodCallMetadata;
        let node = graph
            .nodes()
            .iter()
            .find(|n| n.name == function_name)
            .unwrap();
        let line = node.location.start_line() + 1;
        SdkMethodCall {
            name: operation.to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: Some(SdkMethodCallMetadata::new(
                format!("client.{operation}(...)"),
                Location::new(node.location.file_path.clone(), (line, 5), (line, 15)),
            )),
        }
    }

    fn reachable_names(graph: &CallGraph, from: &str) -> Vec<String> {
        let node = graph.nodes().iter().find(|n| n.name == from).unwrap();
        graph
            .reachable_set(node)
            .into_iter()
            .map(|n| n.name.clone())
            .collect()
    }

    // -----------------------------------------------------------------------
    // Reachability
    // -----------------------------------------------------------------------

    #[rstest]
    #[case(&["a -> b", "b -> c"], "a", &["a", "b", "c"])]
    #[case(&["a -> b", "b -> c"], "b", &["b", "c"])]
    #[case(&["a -> b", "b -> c"], "c", &["c"])]
    #[case(&["a -> b, c", "b -> d", "c -> d"], "a", &["a", "b", "c", "d"])]
    #[case(&["a -> b", "b -> a"], "a", &["a", "b"])]
    #[case(&["a"], "a", &["a"])]
    fn test_reachable_set(
        #[case] edge_specs: &[&str],
        #[case] from: &str,
        #[case] expected: &[&str],
    ) {
        let graph = graph_from_spec(edge_specs);
        assert_eq!(reachable_names(&graph, from), expected);
    }

    // -----------------------------------------------------------------------
    // Partition calls
    // -----------------------------------------------------------------------

    #[rstest]
    // Basic: handler calls fetch which has GetObject; unrelated has DeleteBucket
    #[case(
        &["handler -> fetch", "fetch", "unrelated"],
        &[("GetObject", "fetch"), ("DeleteBucket", "unrelated")],
        "handler",
        &["GetObject"],
    )]
    // Shared callee: both entry points reach the same SDK call
    #[case(
        &["a -> shared", "b -> shared", "shared"],
        &[("PutItem", "shared")],
        "a",
        &["PutItem"],
    )]
    // Direct: SDK call in the entry point itself
    #[case(
        &["handler"],
        &[("GetObject", "handler")],
        "handler",
        &["GetObject"],
    )]
    // Unreachable: SDK call in a function not connected to entry point
    #[case(
        &["handler", "other"],
        &[("GetObject", "other")],
        "handler",
        &[],
    )]
    fn test_partition_calls_single_entry(
        #[case] edge_specs: &[&str],
        #[case] calls: &[(&str, &str)],
        #[case] entry_point: &str,
        #[case] expected_ops: &[&str],
    ) {
        let graph = graph_from_spec(edge_specs);

        let sdk_calls: Vec<SdkMethodCall> = calls
            .iter()
            .map(|(op, func)| sdk_call_in(&graph, op, func))
            .collect();

        let entry = graph
            .nodes()
            .iter()
            .find(|n| n.name == entry_point)
            .unwrap()
            .clone();

        let result = graph.partition_calls(&[entry], &sdk_calls);

        let actual_names: Vec<&str> = result[0].1.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(actual_names, expected_ops);
    }

    #[test]
    fn test_partition_calls_multiple_entry_points_share_call() {
        let graph = graph_from_spec(&["a -> shared", "b -> shared", "shared"]);
        let call = sdk_call_in(&graph, "PutItem", "shared");

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

        let result = graph.partition_calls(&entries, &[call]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].1.len(), 1);
        assert_eq!(result[1].1.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Enclosing function
    // -----------------------------------------------------------------------

    #[test]
    fn test_enclosing_function_finds_innermost() {
        let outer = FunctionNode {
            name: "outer".to_string(),
            qualified_name: None,
            location: Location::new(PathBuf::from("f.go"), (1, 1), (30, 1)),
        };
        let inner = FunctionNode {
            name: "inner".to_string(),
            qualified_name: None,
            location: Location::new(PathBuf::from("f.go"), (5, 1), (15, 1)),
        };

        let graph = CallGraph::new(vec![outer, inner], BTreeMap::new());

        let loc = Location::new(PathBuf::from("f.go"), (10, 5), (10, 15));
        assert_eq!(graph.enclosing_function(&loc).unwrap().name, "inner");
    }
}
