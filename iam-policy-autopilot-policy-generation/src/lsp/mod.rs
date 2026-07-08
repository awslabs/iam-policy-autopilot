//! LSP (Language Server Protocol) client for type information extraction.
//!
//! This module provides a generic, async LSP client built on [`async_lsp`] that can
//! communicate with any language server. It currently supports the
//! [ty](https://github.com/astral-sh/ty) Python type checker, and is designed to be
//! extended to other servers (e.g., gopls for Go) via the [`LspServerConfig`] trait.

mod error;
#[doc(hidden)]
pub mod gopls;
mod ty;

#[cfg(any(test, feature = "integ-test"))]
pub mod test_utils;

pub use error::LspError;
pub use ty::TyLspClient;

use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    InitializedParams, Position, ProgressParamsValue, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Url, WorkDoneProgress, WorkDoneProgressParams,
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

/// Options for configuring `LspClient` behavior.
#[derive(Debug)]
pub struct LspClientOptions {
    /// Time to wait after opening a document for the server to analyze it.
    pub open_document_timeout: Duration,
    /// Timeout for the initialize handshake.
    pub initialize_timeout: Duration,
    /// Timeout for individual server requests (hover, document symbols, call
    /// hierarchy).
    pub request_timeout: Duration,
    /// Timeout for shutdown.
    pub shutdown_timeout: Duration,
    /// Maximum time to wait for the server to finish all work-done progress
    /// tokens (e.g., workspace indexing). Used by [`LspClient::wait_for_idle`].
    pub idle_timeout: Duration,
    /// How long [`LspClient::wait_for_idle`] waits for the *first* progress token
    /// to appear before concluding the server has no background work to do.
    /// Prevents returning before asynchronously-reported indexing has begun.
    pub progress_startup_grace: Duration,
    /// Client capabilities to advertise during initialization.
    pub capabilities: Option<ClientCapabilities>,
}

impl Default for LspClientOptions {
    fn default() -> Self {
        Self {
            open_document_timeout: Duration::from_secs(1),
            initialize_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            idle_timeout: Duration::from_mins(2),
            progress_startup_grace: Duration::from_secs(2),
            capabilities: None,
        }
    }
}

struct ClientState {
    diagnosed_uris: Arc<Mutex<HashSet<Url>>>,
    diagnostics_notify: Arc<Notify>,
    active_progress: Arc<AtomicUsize>,
    progress_notify: Arc<Notify>,
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
    active_progress: Arc<AtomicUsize>,
    progress_notify: Arc<Notify>,
}

/// Wait until `is_ready()` returns `true`, or until `timeout_duration` elapses.
///
/// Returns `true` if the predicate became satisfied, `false` on timeout.
///
/// `notify` must be signalled via [`Notify::notify_waiters`] whenever the state
/// observed by `is_ready` changes. This helper encodes the check-then-wait
/// pattern in the one order that is free of lost wakeups: the `Notified` future
/// is *registered* (via [`Notified::enable`]) **before** each predicate check,
/// so a `notify_waiters` that races in between the check and the `.await` still
/// wakes us. `notify_waiters` stores no permit, so a waiter that is not yet
/// registered when it fires misses the signal entirely — checking the predicate
/// first (then registering) would reintroduce exactly that bug. Callers pass a
/// predicate instead of ordering the check and the await themselves so they
/// cannot get this wrong.
async fn wait_until(
    notify: &Notify,
    timeout_duration: Duration,
    mut is_ready: impl FnMut() -> bool,
) -> bool {
    let deadline = tokio::time::sleep(timeout_duration);
    tokio::pin!(deadline);
    let notified = notify.notified();
    tokio::pin!(notified);
    loop {
        // Register interest BEFORE checking, so a notification that fires
        // between the check and the await is captured rather than lost.
        notified.as_mut().enable();
        if is_ready() {
            return true;
        }
        tokio::select! {
            () = &mut notified => {
                // A consumed `Notified` stays ready forever; re-arm for the
                // next iteration so the next `enable()` observes fresh signals.
                notified.set(notify.notified());
            }
            () = &mut deadline => return false,
        }
    }
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
        let active_progress = Arc::new(AtomicUsize::new(0));
        let progress_notify = Arc::new(Notify::new());
        let handler_uris = Arc::clone(&diagnosed_uris);
        let handler_notify = Arc::clone(&diagnostics_notify);
        let handler_progress = Arc::clone(&active_progress);
        let handler_progress_notify = Arc::clone(&progress_notify);

        let (mainloop, mut server) = MainLoop::new_client(|_server| {
            let mut router = Router::new(ClientState {
                diagnosed_uris: handler_uris,
                diagnostics_notify: handler_notify,
                active_progress: handler_progress,
                progress_notify: handler_progress_notify,
            });
            router
                .request::<lsp_types::request::WorkDoneProgressCreate, _>(|state, params| {
                    log::debug!("workDoneProgress/create: token={:?}", params.token);
                    state.active_progress.fetch_add(1, Ordering::SeqCst);
                    // Wake `wait_for_idle`'s startup phase: it blocks until at
                    // least one progress token exists, so it must observe the
                    // 0 → 1 transition, not just the eventual 1 → 0 one.
                    state.progress_notify.notify_waiters();
                    std::future::ready(Ok(()))
                })
                .notification::<PublishDiagnostics>(|state, params| {
                    state
                        .diagnosed_uris
                        .lock()
                        .expect("diagnosed_uris mutex poisoned")
                        .insert(params.uri);
                    state.diagnostics_notify.notify_waiters();
                    ControlFlow::Continue(())
                })
                .notification::<lsp_types::notification::Progress>(|state, params| {
                    let ProgressParamsValue::WorkDone(progress) = params.value;
                    match progress {
                        WorkDoneProgress::Begin(begin) => {
                            log::info!(
                                "LSP progress begin: {}{}",
                                begin.title,
                                begin
                                    .message
                                    .as_ref()
                                    .map(|m| format!(" — {m}"))
                                    .unwrap_or_default()
                            );
                        }
                        WorkDoneProgress::Report(report) => {
                            log::debug!(
                                "LSP progress: {}",
                                report.message.as_deref().unwrap_or("")
                            );
                        }
                        WorkDoneProgress::End(_) => {
                            state.active_progress.fetch_sub(1, Ordering::SeqCst);
                            state.progress_notify.notify_waiters();
                        }
                    }
                    ControlFlow::Continue(())
                })
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
                // An EOF on the server's stream is expected during shutdown
                // (we kill the process), so this is not necessarily an error.
                // Genuine operational failures surface as `LspError` to callers.
                log::debug!("LSP main loop exited: {e}");
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
            capabilities: options.capabilities.clone().unwrap_or_default(),
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
            active_progress,
            progress_notify,
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
        let uri = file_url(file_path.as_ref())?;

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

        // Best-effort: wait until the server has published diagnostics for this
        // document. A timeout here is not fatal — we proceed regardless.
        let diagnosed_uris = Arc::clone(&self.diagnosed_uris);
        wait_until(
            &self.diagnostics_notify,
            self.options.open_document_timeout,
            || {
                diagnosed_uris
                    .lock()
                    .expect("diagnosed_uris mutex poisoned")
                    .contains(&uri)
            },
        )
        .await;

        Ok(())
    }

    /// Wait until the server has finished all active work-done progress tokens.
    ///
    /// Servers like gopls send `window/workDoneProgress/create` + `$/progress`
    /// notifications while indexing the workspace. The returned future blocks
    /// until all progress tokens have received their `end` notification, or the
    /// configured `idle_timeout` expires.
    ///
    /// Progress arrives asynchronously after documents are opened, so a naive
    /// "return as soon as no tokens are active" check would race the server and
    /// return before indexing even begins. To avoid that, this first waits up to
    /// `progress_startup_grace` for the first token to appear; if none does, the
    /// server has no background work for this input and we return immediately.
    /// Once work has started, we wait up to `idle_timeout` for it to drain.
    ///
    /// This is a non-`async` fn returning an owned future on purpose: an
    /// `async fn(&self)` would capture `&self` for the whole body, and since
    /// `LspClient<C>` is only `Sync` when `C: Sync`, that future would not be
    /// `Send`. By cloning the `Arc`s up front, the returned future owns its
    /// state and stays `Send` regardless of `C`.
    pub fn wait_for_idle(&self) -> impl std::future::Future<Output = Result<(), LspError>> {
        let idle_timeout = self.options.idle_timeout;
        let startup_grace = self.options.progress_startup_grace;
        let active_progress = Arc::clone(&self.active_progress);
        let progress_notify = Arc::clone(&self.progress_notify);

        async move {
            // Phase 1: give indexing a chance to start. The create handler bumps
            // the counter and notifies, so we observe the 0 → 1 transition.
            let started = wait_until(&progress_notify, startup_grace, || {
                active_progress.load(Ordering::SeqCst) > 0
            })
            .await;

            if !started {
                // No progress token appeared within the grace window: the server
                // does no background work for this input (or already finished).
                return Ok(());
            }

            // Phase 2: work is underway — wait for every token to reach `end`.
            if wait_until(&progress_notify, idle_timeout, || {
                active_progress.load(Ordering::SeqCst) == 0
            })
            .await
            {
                Ok(())
            } else {
                Err(LspError::Timeout(idle_timeout))
            }
        }
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

        let response = timeout(self.options.request_timeout, self.server.hover(params))
            .await
            .map_err(|_| LspError::Timeout(self.options.request_timeout))?
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
            self.options.request_timeout,
            self.server.document_symbol(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.request_timeout))?
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
            self.options.request_timeout,
            self.server.prepare_call_hierarchy(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.request_timeout))?
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
            self.options.request_timeout,
            self.server.outgoing_calls(params),
        )
        .await
        .map_err(|_| LspError::Timeout(self.options.request_timeout))?
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
        let shutdown_result =
            timeout(self.options.shutdown_timeout, self.server.shutdown(())).await;

        match shutdown_result {
            Ok(Ok(())) => {
                let _ = self.server.exit(());
            }
            Ok(Err(e)) => {
                log::warn!("LSP shutdown request failed: {e}, killing process");
            }
            Err(_) => {
                log::warn!("LSP shutdown timed out, killing process");
            }
        }

        let _ = self.server.emit(Stop);

        if let Some(handle) = self.mainloop_handle.take() {
            let _ = handle.await;
        }

        let _ = self.child.start_kill();
        let _ = self.child.wait().await;

        Ok(())
    }
}

impl<C: LspServerConfig> Drop for LspClient<C> {
    fn drop(&mut self) {
        // Abort the main loop before the `ServerSocket` channel is dropped.
        // Otherwise the still-running loop polls its receiver, observes the
        // closed sender, and panics with "Sender is alive" (async-lsp).
        // This matters when the client is dropped without `shutdown()` being
        // called, e.g. when an error propagates out of the caller via `?`.
        if let Some(handle) = self.mainloop_handle.take() {
            handle.abort();
        }
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

/// Convert a file path to a `file://` URL.
pub fn file_url(path: &Path) -> Result<Url, LspError> {
    Url::from_file_path(path).map_err(|()| LspError::InvalidPath(path.to_path_buf()))
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

    use std::sync::atomic::AtomicBool;

    #[tokio::test]
    async fn wait_until_wakes_on_notification_and_times_out_otherwise() {
        let notify = Arc::new(Notify::new());

        // Times out when the predicate never holds (the timeout branch).
        assert!(!wait_until(&notify, Duration::from_millis(20), || false).await);

        // Wakes via a notification that races in *after* the wait begins — the
        // case the check-then-register ordering exists to handle. The 1s ceiling
        // is never reached on success; it only bounds how long a regressed
        // (lost-wakeup) helper would hang before this assert fails.
        let flag = Arc::new(AtomicBool::new(false));
        let bg_notify = Arc::clone(&notify);
        let bg_flag = Arc::clone(&flag);
        tokio::spawn(async move {
            bg_flag.store(true, Ordering::SeqCst);
            bg_notify.notify_waiters();
        });
        assert!(
            wait_until(&notify, Duration::from_secs(1), || flag
                .load(Ordering::SeqCst))
            .await,
            "wait_until should wake on notification, not time out"
        );
    }
}
