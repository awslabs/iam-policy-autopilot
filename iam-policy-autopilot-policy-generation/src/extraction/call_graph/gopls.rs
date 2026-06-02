use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use lsp_types::{DocumentSymbolResponse, SymbolKind};

use super::{CallGraph, CallGraphBuilder, CallGraphError, FunctionNode};
use crate::lsp::gopls::GoplsClient;
use crate::lsp::{file_url, LspClientOptions};
use crate::Location;

pub(crate) struct GoplsCallGraphBuilder {
    client: GoplsClient,
}

impl GoplsCallGraphBuilder {
    pub(crate) async fn new(workspace_root: &Path) -> Result<Self, CallGraphError> {
        let options = LspClientOptions {
            initialize_timeout: Duration::from_secs(30),
            open_document_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
            // gopls returns Flat (SymbolInformation) by default, which lacks selection_range.
            // Hierarchical mode gives us DocumentSymbol with selection_range — needed to
            // position prepare_call_hierarchy on the function name, not the `func` keyword.
            capabilities: Some(lsp_types::ClientCapabilities {
                text_document: Some(lsp_types::TextDocumentClientCapabilities {
                    document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        let client = GoplsClient::create_with_options(workspace_root, options).await?;
        Ok(Self { client })
    }
}

#[async_trait]
impl CallGraphBuilder for GoplsCallGraphBuilder {
    async fn build(
        &mut self,
        _workspace_root: &Path,
        source_files: &[PathBuf],
    ) -> Result<CallGraph, CallGraphError> {
        let client = &mut self.client;

        let source_file_set: BTreeSet<&PathBuf> = source_files.iter().collect();

        for file in source_files {
            let content = std::fs::read_to_string(file).map_err(|e| {
                CallGraphError::Other(format!("Failed to read {}: {e}", file.display()))
            })?;
            client.open_document(file, &content).await?;
        }

        let mut nodes = Vec::new();
        // Maps node index → (uri, selection_range start) for prepare_call_hierarchy
        let mut name_positions: Vec<(lsp_types::Url, lsp_types::Position)> = Vec::new();

        for file in source_files {
            let uri = file_url(file)?;

            let Some(symbols) = client.document_symbols(&uri).await? else {
                continue;
            };

            match symbols {
                DocumentSymbolResponse::Nested(syms) => {
                    for sym in syms {
                        if matches!(sym.kind, SymbolKind::FUNCTION | SymbolKind::METHOD) {
                            if let Some(location) = Location::from_lsp(&uri, &sym.range) {
                                name_positions.push((uri.clone(), sym.selection_range.start));
                                nodes.push(FunctionNode {
                                    name: sym.name,
                                    qualified_name: sym.detail,
                                    location,
                                });
                            }
                        }
                    }
                }
                DocumentSymbolResponse::Flat(infos) => {
                    for info in infos {
                        if matches!(info.kind, SymbolKind::FUNCTION | SymbolKind::METHOD) {
                            if let Some(location) = Location::from_lsp(&uri, &info.location.range) {
                                name_positions.push((uri.clone(), info.location.range.start));
                                nodes.push(FunctionNode {
                                    name: info.name,
                                    qualified_name: info.container_name,
                                    location,
                                });
                            }
                        }
                    }
                }
            }
        }

        let mut edges: HashMap<FunctionNode, Vec<FunctionNode>> = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            let (ref uri, ref name_pos) = name_positions[i];

            let item = match client
                .prepare_call_hierarchy(uri, name_pos.line, name_pos.character)
                .await
            {
                Ok(Some(mut items)) if !items.is_empty() => items.swap_remove(0),
                Ok(_) => continue,
                Err(e) => {
                    log::debug!(
                        "prepare_call_hierarchy failed for '{}' at {}:{}: {e}",
                        node.name,
                        name_pos.line,
                        name_pos.character
                    );
                    continue;
                }
            };

            let Some(outgoing) = client.outgoing_calls(item).await? else {
                continue;
            };

            let callees: Vec<FunctionNode> = outgoing
                .into_iter()
                .filter_map(|call| {
                    let callee_path = call.to.uri.to_file_path().ok()?;
                    if !source_file_set.contains(&callee_path) {
                        return None;
                    }
                    let location = Location::from_lsp(&call.to.uri, &call.to.range)?;
                    Some(FunctionNode {
                        name: call.to.name,
                        qualified_name: call.to.detail,
                        location,
                    })
                })
                .collect();

            edges.insert(node.clone(), callees);
        }

        Ok(CallGraph::new(nodes, edges))
    }

    async fn shutdown(self) -> Result<(), CallGraphError> {
        self.client.shutdown().await?;
        Ok(())
    }
}

#[cfg(all(test, feature = "integ-test", feature = "call-graph"))]
mod tests {
    use super::*;
    use crate::lsp::test_utils::go;
    use tempfile::TempDir;
    use tokio::fs;

    async fn build_graph(source: &str) -> CallGraph {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();
        fs::write(root.join("go.mod"), go::fixtures::GO_MOD)
            .await
            .unwrap();
        fs::write(root.join("main.go"), source).await.unwrap();

        let mut builder = GoplsCallGraphBuilder::new(&root).await.unwrap();
        let graph = builder.build(&root, &[root.join("main.go")]).await.unwrap();
        builder.shutdown().await.unwrap();
        graph
    }

    #[tokio::test]
    #[ignore]
    async fn test_build_simple_call_chain() {
        if !go::is_ready() {
            panic!("requires Go + gopls");
        }

        let graph = build_graph(go::fixtures::SIMPLE_CALL_CHAIN).await;

        let names: Vec<&str> = graph.nodes().iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"main"), "got: {names:?}");
        assert!(names.contains(&"helper"), "got: {names:?}");
        assert!(names.contains(&"deepHelper"), "got: {names:?}");
        assert!(names.contains(&"unrelated"), "got: {names:?}");

        let main_node = graph.nodes().iter().find(|n| n.name == "main").unwrap();
        let main_callees: Vec<&str> = graph
            .outgoing(main_node)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            main_callees.contains(&"helper"),
            "main calls: {main_callees:?}"
        );

        let helper_node = graph.nodes().iter().find(|n| n.name == "helper").unwrap();
        let helper_callees: Vec<&str> = graph
            .outgoing(helper_node)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            helper_callees.contains(&"deepHelper"),
            "helper calls: {helper_callees:?}"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_build_struct_methods() {
        if !go::is_ready() {
            panic!("requires Go + gopls");
        }

        let graph = build_graph(go::fixtures::STRUCT_METHODS).await;

        for node in graph.nodes() {
            println!(
                "node: name={:?}, qualified_name={:?}, location={:?}",
                node.name, node.qualified_name, node.location
            );
        }

        let handle = graph
            .nodes()
            .iter()
            .find(|n| n.name.contains("HandleRequest"))
            .expect("should find HandleRequest");

        let callees: Vec<&str> = graph
            .outgoing(handle)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            callees.contains(&"fetchData"),
            "HandleRequest calls: {callees:?}"
        );
        assert!(
            callees.contains(&"format"),
            "HandleRequest calls: {callees:?}"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_external_calls_filtered() {
        if !go::is_ready() {
            panic!("requires Go + gopls");
        }

        let graph = build_graph(go::fixtures::SIMPLE_CALL_CHAIN).await;

        let main_node = graph.nodes().iter().find(|n| n.name == "main").unwrap();
        let callees: Vec<&str> = graph
            .outgoing(main_node)
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            !callees.contains(&"Println"),
            "external calls should be filtered: {callees:?}"
        );
    }
}
