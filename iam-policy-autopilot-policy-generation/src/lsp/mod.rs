//! LSP (Language Server Protocol) client for type information extraction.
//!
//! This module provides a generic, async LSP client built on [`async_lsp`] that can
//! communicate with any language server. It currently supports the
//! [ty](https://github.com/astral-sh/ty) Python type checker, and is designed to be
//! extended to other servers (e.g., gopls for Go) via the [`LspServerConfig`] trait.

mod error;
#[doc(hidden)]
pub mod gopls;

#[cfg(any(test, feature = "integ-test"))]
pub mod test_utils;

pub use error::LspError;

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::tracing::TracingLayer;
use async_lsp::{LanguageServer, MainLoop, ServerSocket};
use lsp_types::notification::PublishDiagnostics;
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, HoverParams, InitializeParams,
    InitializedParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Url, WorkDoneProgressParams,
};
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tower::ServiceBuilder;

/// Configuration for a specific language server.
pub trait LspServerConfig {
    /// Binary name to locate in PATH (e.g., "ty", "gopls").
    fn binary_name(&self) -> &'static str;

    /// Command-line arguments to start the server (e.g., &["server"], &["serve"]).
    fn args(&self) -> &[&str];

    /// LSP language identifier (e.g., "python", "go").
    fn language_id(&self) -> &'static str;

    /// Check if the server binary is available in PATH.
    fn is_available(&self) -> bool {
        which::which(self.binary_name()).is_ok()
    }
}

/// Configuration for the ty Python type checker.
pub struct TyConfig;

impl LspServerConfig for TyConfig {
    fn binary_name(&self) -> &'static str {
        "ty"
    }

    fn args(&self) -> &[&str] {
        &["server"]
    }

    fn language_id(&self) -> &'static str {
        "python"
    }
}

/// Options for configuring `LspClient` behavior.
#[derive(Debug)]
pub struct LspClientOptions {
    /// Time to wait after opening a document for the server to analyze it.
    pub open_document_timeout: Duration,
    /// Timeout for the initialize handshake.
    pub initialize_timeout: Duration,
    /// Timeout for hover requests.
    pub hover_timeout: Duration,
    /// Timeout for shutdown.
    pub shutdown_timeout: Duration,
}

impl Default for LspClientOptions {
    fn default() -> Self {
        Self {
            open_document_timeout: Duration::from_secs(1),
            initialize_timeout: Duration::from_secs(10),
            hover_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
        }
    }
}

struct ClientState {
    diagnosed_uris: Arc<Mutex<HashSet<Url>>>,
    diagnostics_notify: Arc<Notify>,
}
struct Stop;

/// Generic LSP client parameterized by server configuration.
///
/// Manages the server process lifecycle, handles LSP protocol communication
/// via `async-lsp`, and provides methods for opening documents and querying
/// type information.
#[derive(Debug)]
pub struct LspClient<C: LspServerConfig> {
    config: C,
    options: LspClientOptions,
    server: ServerSocket,
    mainloop_handle: Option<tokio::task::JoinHandle<()>>,
    child: tokio::process::Child,
    opened_documents: HashSet<String>,
    diagnosed_uris: Arc<Mutex<HashSet<Url>>>,
    diagnostics_notify: Arc<Notify>,
}

impl<C: LspServerConfig> LspClient<C> {
    /// Create and initialize a new LSP client with default options.
    pub async fn new(config: C, workspace_root: impl AsRef<Path>) -> Result<Self, LspError> {
        Self::with_options(config, workspace_root, LspClientOptions::default()).await
    }

    /// Create and initialize a new LSP client with custom options.
    pub async fn with_options(
        config: C,
        workspace_root: impl AsRef<Path>,
        options: LspClientOptions,
    ) -> Result<Self, LspError> {
        let binary_path = which::which(config.binary_name())
            .map_err(|_| LspError::ServerNotFound(config.binary_name().to_string()))?;

        let mut child = tokio::process::Command::new(binary_path)
            .args(config.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LspError::StartupFailed(format!("Failed to spawn process: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::StartupFailed("Failed to get stdout handle".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::StartupFailed("Failed to get stdin handle".into()))?;

        let diagnosed_uris = Arc::new(Mutex::new(HashSet::<Url>::new()));
        let diagnostics_notify = Arc::new(Notify::new());
        let handler_uris = Arc::clone(&diagnosed_uris);
        let handler_notify = Arc::clone(&diagnostics_notify);

        let (mainloop, mut server) = MainLoop::new_client(|_server| {
            let mut router = Router::new(ClientState {
                diagnosed_uris: handler_uris,
                diagnostics_notify: handler_notify,
            });
            router
                .notification::<PublishDiagnostics>(|state, params| {
                    state
                        .diagnosed_uris
                        .lock()
                        .expect("diagnosed_uris mutex poisoned")
                        .insert(params.uri);
                    state.diagnostics_notify.notify_waiters();
                    ControlFlow::Continue(())
                })
                .notification::<lsp_types::notification::Progress>(|_, _| ControlFlow::Continue(()))
                .unhandled_notification(|_, method| {
                    log::debug!("Unhandled notification from server: {method:?}");
                    ControlFlow::Continue(())
                })
                .event(|_, _: Stop| ControlFlow::Break(Ok(())));
            ServiceBuilder::new()
                .layer(TracingLayer::default())
                .layer(CatchUnwindLayer::default())
                .layer(ConcurrencyLayer::default())
                .service(router)
        });

        let stdout = stdout.compat();
        let stdin = stdin.compat_write();

        let mainloop_handle = tokio::spawn(async move {
            if let Err(e) = mainloop.run_buffered(stdout, stdin).await {
                log::error!("LSP main loop error: {e}");
            }
        });

        let workspace_root_str = workspace_root
            .as_ref()
            .to_str()
            .ok_or_else(|| LspError::StartupFailed("Invalid workspace path (non-UTF8)".into()))?;
        let workspace_uri = Url::from_file_path(workspace_root_str)
            .map_err(|()| LspError::StartupFailed("Invalid workspace path".into()))?;

        #[allow(deprecated)]
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(workspace_uri.clone()),
            capabilities: ClientCapabilities::default(),
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                uri: workspace_uri,
                name: workspace_root
                    .as_ref()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            }]),
            ..Default::default()
        };

        timeout(options.initialize_timeout, server.initialize(init_params))
            .await
            .map_err(|_| LspError::Timeout(options.initialize_timeout))?
            .map_err(|e| LspError::InitializeFailed(format!("{e}")))?;

        server
            .initialized(InitializedParams {})
            .map_err(|e| LspError::InitializeFailed(format!("Failed to send initialized: {e}")))?;

        Ok(Self {
            config,
            options,
            server,
            mainloop_handle: Some(mainloop_handle),
            child,
            opened_documents: HashSet::new(),
            diagnosed_uris,
            diagnostics_notify,
        })
    }

    /// Open a document for analysis.
    ///
    /// Sends a `textDocument/didOpen` notification and waits for the configured
    /// delay to allow the server to analyze the document.
    pub async fn open_document(
        &mut self,
        file_path: impl AsRef<Path>,
        content: &str,
    ) -> Result<(), LspError> {
        let uri = Url::from_file_path(file_path.as_ref()).map_err(|()| {
            LspError::ParseFailed(format!("Invalid file path: {:?}", file_path.as_ref()))
        })?;

        let uri_string = uri.to_string();
        if self.opened_documents.contains(&uri_string) {
            return Ok(());
        }

        self.server
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: self.config.language_id().to_string(),
                    version: 1,
                    text: content.to_string(),
                },
            })
            .map_err(|e| LspError::SendFailed(std::io::Error::other(format!("{e}"))))?;

        self.opened_documents.insert(uri_string);

        let deadline = tokio::time::sleep(self.options.open_document_timeout);
        tokio::pin!(deadline);
        loop {
            let notified = self.diagnostics_notify.notified();
            tokio::pin!(notified);

            if self
                .diagnosed_uris
                .lock()
                .expect("diagnosed_uris mutex poisoned")
                .contains(&uri)
            {
                break;
            }
            tokio::select! {
                () = &mut notified => {},
                () = &mut deadline => break,
            }
        }

        Ok(())
    }

    /// Query hover information at a specific position.
    ///
    /// Returns the extracted type information string if available.
    pub async fn hover(
        &mut self,
        uri: &Url,
        line: u32,
        column: u32,
    ) -> Result<Option<String>, LspError> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(line, column),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let response = timeout(self.options.hover_timeout, self.server.hover(params))
            .await
            .map_err(|_| LspError::Timeout(self.options.hover_timeout))?
            .map_err(|e| LspError::ServerError(format!("{e}")))?;

        Ok(response.and_then(|hover| extract_type_from_hover(&hover)))
    }

    /// Get document symbols (functions, methods, classes) for a file.
    pub async fn document_symbols(
        &mut self,
        uri: &Url,
    ) -> Result<Option<lsp_types::DocumentSymbolResponse>, LspError> {
        let params = lsp_types::DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response = timeout(
            self.options.hover_timeout,
            self.server.document_symbol(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.hover_timeout))?
        .map_err(|e| LspError::ServerError(format!("{e}")))?;

        Ok(response)
    }

    /// Prepare call hierarchy at a given position.
    ///
    /// Returns the call hierarchy items at the position, typically one item
    /// representing the function/method at that location.
    pub async fn prepare_call_hierarchy(
        &mut self,
        uri: &Url,
        line: u32,
        column: u32,
    ) -> Result<Option<Vec<lsp_types::CallHierarchyItem>>, LspError> {
        let params = lsp_types::CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(line, column),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let response = timeout(
            self.options.hover_timeout,
            self.server.prepare_call_hierarchy(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.hover_timeout))?
        .map_err(|e| LspError::ServerError(format!("{e}")))?;

        Ok(response)
    }

    /// Get outgoing calls from a call hierarchy item.
    ///
    /// Returns all functions/methods called from the given item.
    pub async fn outgoing_calls(
        &mut self,
        item: lsp_types::CallHierarchyItem,
    ) -> Result<Option<Vec<lsp_types::CallHierarchyOutgoingCall>>, LspError> {
        let params = lsp_types::CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response = timeout(
            self.options.hover_timeout,
            self.server.outgoing_calls(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.hover_timeout))?
        .map_err(|e| LspError::ServerError(format!("{e}")))?;

        Ok(response)
    }

    /// Shutdown the LSP server gracefully.
    ///
    /// This method:
    /// 1. Sends a shutdown request with a configurable timeout
    /// 2. Sends an exit notification
    /// 3. Waits for the process to exit
    pub async fn shutdown(mut self) -> Result<(), LspError> {
        timeout(self.options.shutdown_timeout, self.server.shutdown(()))
            .await
            .map_err(|_| LspError::Timeout(self.options.shutdown_timeout))?
            .map_err(|e| LspError::ServerError(format!("Shutdown failed: {e}")))?;

        self.server
            .exit(())
            .map_err(|e| LspError::SendFailed(std::io::Error::other(format!("{e}"))))?;

        let _ = self.server.emit(Stop);

        if let Some(handle) = self.mainloop_handle.take() {
            let _ = handle.await;
        }

        let _ = self.child.wait().await;

        Ok(())
    }
}

impl<C: LspServerConfig> Drop for LspClient<C> {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Extract a human-readable type string from an LSP Hover response.
#[must_use]
pub fn extract_type_from_hover(hover: &lsp_types::Hover) -> Option<String> {
    use lsp_types::{HoverContents, MarkedString, MarkupContent};

    match &hover.contents {
        HoverContents::Scalar(marked) => match marked {
            MarkedString::String(s) => non_empty(s),
            MarkedString::LanguageString(ls) => non_empty(&ls.value),
        },
        HoverContents::Markup(MarkupContent { value, .. }) => non_empty(value),
        HoverContents::Array(items) => {
            let values: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    MarkedString::String(s) => non_empty(s),
                    MarkedString::LanguageString(ls) => non_empty(&ls.value),
                })
                .collect();
            if values.is_empty() {
                None
            } else {
                Some(values.join("\n"))
            }
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Convenience type alias for the ty Python type checker client.
pub type TyLspClient = LspClient<TyConfig>;

impl TyLspClient {
    /// Create a new ty LSP client with default options.
    pub async fn create(workspace_root: impl AsRef<Path>) -> Result<Self, LspError> {
        Self::new(TyConfig, workspace_root).await
    }

    /// Create a new ty LSP client with custom options.
    pub async fn create_with_options(
        workspace_root: impl AsRef<Path>,
        options: LspClientOptions,
    ) -> Result<Self, LspError> {
        Self::with_options(TyConfig, workspace_root, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        Hover, HoverContents, LanguageString, MarkedString, MarkupContent, MarkupKind,
    };
    use rstest::rstest;

    #[rstest]
    #[case(
        Hover {
            contents: HoverContents::Scalar(MarkedString::String("str".into())),
            range: None,
        },
        Some("str".into())
    )]
    #[case(
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "```python\ns3_client: S3Client\n```".into(),
            }),
            range: None,
        },
        Some("```python\ns3_client: S3Client\n```".into())
    )]
    #[case(
        Hover {
            contents: HoverContents::Scalar(MarkedString::String(String::new())),
            range: None,
        },
        None
    )]
    #[case(
        Hover {
            contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                language: "python".into(),
                value: "int".into(),
            })),
            range: None,
        },
        Some("int".into())
    )]
    #[case(
        Hover {
            contents: HoverContents::Array(vec![
                MarkedString::String("Type: str".into()),
                MarkedString::LanguageString(LanguageString {
                    language: "python".into(),
                    value: "extra".into(),
                }),
            ]),
            range: None,
        },
        Some("Type: str\nextra".into())
    )]
    fn test_extract_type_from_hover(#[case] hover: Hover, #[case] expected: Option<String>) {
        assert_eq!(extract_type_from_hover(&hover), expected);
    }
}
