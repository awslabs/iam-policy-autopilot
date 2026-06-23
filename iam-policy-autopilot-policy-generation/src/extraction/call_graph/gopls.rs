use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use lsp_types::{DocumentSymbolResponse, SymbolKind};

use super::{innermost_enclosing, CallGraph, CallGraphBuilder, CallGraphError, FunctionNode};
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
            open_document_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_mins(5),
            // gopls is slow to announce indexing on a cold module cache; give the
            // first progress token generous headroom before assuming there is none.
            progress_startup_grace: Duration::from_secs(10),
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
                window: Some(lsp_types::WindowClientCapabilities {
                    work_done_progress: Some(true),
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

        let t_open = std::time::Instant::now();
        for file in source_files {
            let content = std::fs::read_to_string(file).map_err(|e| {
                CallGraphError::Other(format!("Failed to read {}: {e}", file.display()))
            })?;
            client.open_document(file, &content).await?;
        }

        log::info!("Waiting for language server to finish indexing...");
        client.wait_for_idle().await?;
        log::info!("Language server ready");
        let open_idle_ms = t_open.elapsed().as_millis();

        let t_symbols = std::time::Instant::now();
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

        let symbols_ms = t_symbols.elapsed().as_millis();

        let t_hierarchy = std::time::Instant::now();
        let mut edges: BTreeMap<FunctionNode, Vec<FunctionNode>> = BTreeMap::new();
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

            let outgoing = match client.outgoing_calls(item).await {
                Ok(Some(outgoing)) => outgoing,
                Ok(None) => continue,
                Err(e) => {
                    log::debug!(
                        "outgoing_calls failed for '{}' at {}:{}: {e}",
                        node.name,
                        name_pos.line,
                        name_pos.character
                    );
                    continue;
                }
            };

            let callees: Vec<FunctionNode> = outgoing
                .into_iter()
                .filter_map(|call| {
                    let callee_path = call.to.uri.to_file_path().ok()?;
                    if !source_file_set.contains(&callee_path) {
                        return None;
                    }
                    // Match the callee to a discovered node by position, not name:
                    // documentSymbol qualifies methods as `(*Type).Method` while
                    // outgoing_calls reports a bare `Method`, so names don't line
                    // up. Both agree `call.to.range` sits inside the declaration,
                    // so we pick the node enclosing it (none ⇒ external call,
                    // dropped).
                    let callee_start =
                        Location::from_lsp(&call.to.uri, &call.to.range)?.start_position;
                    let hit = innermost_enclosing(&nodes, callee_start, |path| path == callee_path);
                    if hit.is_none() {
                        log::debug!(
                            "outgoing call to '{}' at {callee_start:?} matched no known node",
                            call.to.name,
                        );
                    }
                    hit.cloned()
                })
                .collect();

            edges.insert(node.clone(), callees);
        }

        // Sub-phase breakdown of call-graph build. The per-node call-hierarchy
        // loop issues 2 serial LSP round-trips per node and is expected to
        // dominate; this confirms it before optimizing.
        log::info!(
            "Call graph build: open+idle={open_idle_ms}ms symbols={symbols_ms}ms hierarchy={}ms ({} nodes, 2 round-trips each)",
            t_hierarchy.elapsed().as_millis(),
            nodes.len(),
        );

        Ok(CallGraph::new(nodes, edges))
    }

    fn is_running(&self) -> bool {
        self.client.is_alive()
    }

    async fn shutdown(self: Box<Self>) -> Result<(), CallGraphError> {
        self.client.shutdown().await?;
        Ok(())
    }
}

#[cfg(all(test, feature = "integ-test", feature = "model-generation"))]
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
        Box::new(builder).shutdown().await.unwrap();
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
        // Method callees are matched by position, so the recorded node keeps the
        // documentSymbol form `(*Server).fetchData`, not the bare name gopls
        // reports in the call hierarchy.
        assert!(
            callees.iter().any(|c| c.contains("fetchData")),
            "HandleRequest calls: {callees:?}"
        );
        assert!(
            callees.iter().any(|c| c.contains("format")),
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
