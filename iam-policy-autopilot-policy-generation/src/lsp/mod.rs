//! LSP (Language Server Protocol) client for Python type information.
//!
//! This module provides a minimal, standalone LSP client for communicating with
//! the [ty](https://github.com/astral-sh/ty) Python type checker. The client uses
//! JSON-RPC 2.0 over stdin/stdout to query type information via hover requests.
//!
//! # Overview
//!
//! The LSP client automatically detects and uses ty when available on the system,
//! requiring no configuration. It manages the ty server process lifecycle, handles

#![allow(dead_code)]
//! document synchronization, and provides async APIs for querying type information.
//!
//! ## Key Features
//!
//! - **Automatic ty detection**: Checks PATH for ty availability
//! - **Process management**: Spawns, initializes, and cleanly shuts down ty server
//! - **Document handling**: Opens Python files for analysis with proper synchronization
//! - **Type queries**: Retrieves type information at specific code positions via hover
//! - **Async-first**: Built on tokio for efficient async I/O operations
//! - **Error handling**: Comprehensive error types with timeout protection
//!
//! # Usage
//!
//! ## Basic Example
//!
//! ```no_run
//! use iam_policy_autopilot_policy_generation::lsp::TyLspClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create and initialize a ty LSP client
//! let mut client = TyLspClient::new("/path/to/workspace").await?;
//!
//! // Open a Python file for analysis
//! let content = "import boto3\ns3 = boto3.client('s3')";
//! client.open_document("/path/to/file.py", content).await?;
//!
//! // Query type information at a specific position (line 1, character 5)
//! if let Some(type_info) = client.hover("file:///path/to/file.py", 1, 5).await? {
//!     println!("Type: {}", type_info);
//! }
//!
//! // Shutdown gracefully
//! client.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Multiple Documents
//!
//! ```no_run
//! use iam_policy_autopilot_policy_generation::lsp::TyLspClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = TyLspClient::new("/path/to/workspace").await?;
//!
//! // Open multiple Python files in the same workspace
//! client.open_document("/path/to/workspace/app.py", "import boto3").await?;
//! client.open_document("/path/to/workspace/utils.py", "def helper(): pass").await?;
//!
//! // Query type information from any opened document
//! let type1 = client.hover("file:///path/to/workspace/app.py", 0, 7).await?;
//! let type2 = client.hover("file:///path/to/workspace/utils.py", 0, 4).await?;
//!
//! client.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Error Handling
//!
//! ```no_run
//! use iam_policy_autopilot_policy_generation::lsp::{TyLspClient, LspError};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! match TyLspClient::new("/path/to/workspace").await {
//!     Ok(mut client) => {
//!         // Use the client
//!         client.shutdown().await?;
//!     }
//!     Err(LspError::TyNotFound) => {
//!         eprintln!("ty is not installed or not in PATH");
//!     }
//!     Err(LspError::Timeout(duration)) => {
//!         eprintln!("ty server initialization timed out after {:?}", duration);
//!     }
//!     Err(e) => {
//!         eprintln!("Failed to start ty server: {}", e);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Public API
//!
//! ## Types
//!
//! - [`TyLspClient`]: Main client struct for managing ty server and querying type information
//! - [`LspError`]: Error type for all LSP operations
//!
//! # Error Conditions
//!
//! The LSP client can fail in several ways:
//!
//! ## Startup Errors
//!
//! - **[`LspError::TyNotFound`]**: ty command not found in PATH
//!   - Occurs when ty is not installed or not accessible
//!   - Check: `which ty` or `ty --version`
//!
//! - **[`LspError::StartupFailed`]**: Failed to spawn or initialize ty server
//!   - Process spawn failed (permissions, missing dependencies)
//!   - Failed to get stdin/stdout handles
//!   - Invalid workspace path (non-UTF-8 characters)
//!
//! - **[`LspError::InitializeFailed`]**: Initialize request failed
//!   - ty server rejected initialization
//!   - ty server returned error response
//!   - Communication failure during initialization
//!
//! ## Communication Errors
//!
//! - **[`LspError::Timeout`]**: Operation exceeded timeout duration
//!   - Initialize: 10 seconds
//!   - Hover: 5 seconds
//!   - Shutdown: 2 seconds
//!   - May indicate ty server is unresponsive or overloaded
//!
//! - **[`LspError::SendFailed`]**: Failed to send message to ty server
//!   - Broken pipe (ty server crashed)
//!   - I/O error writing to stdin
//!
//! - **[`LspError::ParseFailed`]**: Failed to parse LSP response
//!   - Malformed JSON from ty server
//!   - Invalid Content-Length header
//!   - Unexpected response format
//!
//! ## Server Errors
//!
//! - **[`LspError::ServerError`]**: ty server returned error response
//!   - Invalid request parameters
//!   - Document not found
//!   - Internal ty server error
//!
//! # Protocol Details
//!
//! ## LSP Communication Flow
//!
//! 1. **Initialize**: Client sends initialize request with workspace root
//! 2. **Initialized**: Client sends initialized notification after receiving response
//! 3. **Document Open**: Client sends textDocument/didOpen for each file
//! 4. **Analysis Wait**: Client waits 1 second for ty to analyze document
//! 5. **Hover Query**: Client sends textDocument/hover requests for type information
//! 6. **Shutdown**: Client sends shutdown request and exit notification
//!
//! ## Message Format
//!
//! All LSP messages use JSON-RPC 2.0 with Content-Length header:
//!
//! ```text
//! Content-Length: <byte_count>\r\n
//! \r\n
//! <JSON payload>
//! ```
//!
//! ## Position Coordinates
//!
//! - **Line numbers**: Zero-based (first line is 0)
//! - **Character positions**: Zero-based (first character is 0)
//! - **Character encoding**: UTF-16 code units (LSP standard)
//!
//! # Requirements
//!
//! - **ty**: Must be installed and available in PATH
//!   - Install: `pip install ty` or `cargo install ty`
//!   - Verify: `ty --version`
//!
//! - **Tokio runtime**: Async operations require tokio runtime
//!   - Client methods are async and must be awaited
//!   - Use `tokio::runtime::Runtime` or `#[tokio::main]`
//!
//! # Implementation Notes
//!
//! ## Document Lifecycle
//!
//! - Documents must be opened before querying hover information
//! - The client tracks opened documents to avoid duplicate opens
//! - Documents remain open until client shutdown
//! - ty analyzes documents asynchronously (1 second wait after open)
//!
//! ## Process Management
//!
//! - ty server process is spawned with stdin/stdout pipes
//! - Process is automatically killed when client is dropped
//! - Graceful shutdown sends shutdown request and exit notification
//! - Drop implementation ensures process cleanup even on panic
//!
//! ## Timeouts
//!
//! All operations have timeouts to prevent hanging:
//! - Initialize: 10 seconds (ty needs time to start and analyze workspace)
//! - Hover: 5 seconds (type queries should be fast)
//! - Shutdown: 2 seconds (graceful shutdown should be quick)
//!
//! ## Thread Safety
//!
//! - `TyLspClient` is not `Send` or `Sync` due to process handles
//! - Use within a single async task or protect with `Arc<Mutex<>>`
//! - Request IDs use `AtomicU64` for thread-safe incrementing

mod error;
mod protocol;

// Test utilities are available in both unit tests and integration tests
#[cfg(any(test, feature = "integ-test"))]
pub mod test_utils;

pub use error::LspError;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout};

/// LSP client for the ty Python type checker.
///
/// This client manages a ty server process and provides async methods for
/// opening documents and querying type information via hover requests.
#[allow(dead_code)]
#[derive(Debug)]
pub struct TyLspClient {
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    request_id: AtomicU64,
    workspace_root: String,
    opened_documents: HashSet<String>,
}

impl TyLspClient {
    /// Create and initialize a new ty LSP client.
    ///
    /// This method:
    /// 1. Checks if ty is available in PATH
    /// 2. Spawns the ty server process
    /// 3. Sends an initialize request
    /// 4. Waits for the initialize response
    /// 5. Sends an initialized notification
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - ty is not found in PATH
    /// - The ty server fails to start
    /// - The initialize request times out or fails
    pub async fn new(workspace_root: impl AsRef<Path>) -> Result<Self, LspError> {
        use std::process::Stdio;
        use tokio::process::Command;
        use tokio::time::{timeout, Duration};

        // Check if ty is in PATH
        let ty_path = which::which("ty").map_err(|_| {
            log::warn!("ty command not found in PATH");
            LspError::TyNotFound
        })?;

        log::debug!("Found ty at: {ty_path:?}");
        log::debug!(
            "Starting ty server with workspace: {}",
            workspace_root.as_ref().display()
        );

        // Spawn ty server process with stdin/stdout pipes
        let mut process = Command::new(ty_path)
            .arg("server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LspError::StartupFailed(format!("Failed to spawn process: {e}")))?;

        log::debug!("ty server process spawned with PID: {:?}", process.id());

        // Get stdin and stdout handles
        let mut stdin = process
            .stdin
            .take()
            .ok_or_else(|| LspError::StartupFailed("Failed to get stdin handle".into()))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| LspError::StartupFailed("Failed to get stdout handle".into()))?;
        let mut stdout = BufReader::new(stdout);

        // Send initialize request with 10s timeout
        let workspace_root_str = workspace_root
            .as_ref()
            .to_str()
            .ok_or_else(|| LspError::StartupFailed("Invalid workspace path".into()))?;

        let init_request = protocol::create_initialize_request(1, workspace_root_str);

        log::debug!("Sending initialize request");

        let (stdin, stdout) = timeout(Duration::from_secs(10), async {
            // Send initialize request
            protocol::send_message(&mut stdin, &init_request)
                .await
                .map_err(|e| LspError::InitializeFailed(format!("Failed to send request: {e}")))?;

            // Wait for initialize response
            let response = protocol::read_message(&mut stdout, Duration::from_secs(10))
                .await
                .map_err(|e| LspError::InitializeFailed(format!("Failed to read response: {e}")))?;

            log::debug!("Received initialize response: {response:?}");

            // Check for error in response
            if let Some(error) = response.get("error") {
                return Err(LspError::InitializeFailed(format!(
                    "Server returned error: {error}"
                )));
            }

            // Verify we got a result
            if response.get("result").is_none() {
                return Err(LspError::InitializeFailed(
                    "No result in initialize response".into(),
                ));
            }

            // Send initialized notification
            let initialized_notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });

            log::debug!("Sending initialized notification");

            protocol::send_message(&mut stdin, &initialized_notification)
                .await
                .map_err(|e| {
                    LspError::InitializeFailed(format!("Failed to send notification: {e}"))
                })?;

            Ok::<_, LspError>((stdin, stdout))
        })
        .await
        .map_err(|_| LspError::Timeout(Duration::from_secs(10)))??;

        log::info!("ty LSP client initialized successfully");

        Ok(Self {
            process,
            stdin,
            stdout,
            request_id: AtomicU64::new(2), // Start at 2 since we used 1 for initialize
            workspace_root: workspace_root_str.to_string(),
            opened_documents: HashSet::new(),
        })
    }

    /// Open a document for analysis.
    ///
    /// Sends a textDocument/didOpen notification to the ty server and waits
    /// 1 second for ty to analyze the document.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification fails to send.
    pub async fn open_document(
        &mut self,
        file_path: impl AsRef<Path>,
        content: &str,
    ) -> Result<(), LspError> {
        use tokio::time::{sleep, Duration};

        // Convert file path to file:// URI
        let file_uri = lsp_types::Url::from_file_path(file_path.as_ref())
            .map(|url| url.to_string())
            .map_err(|()| {
                LspError::ParseFailed(format!("Invalid file path: {:?}", file_path.as_ref()))
            })?;

        log::debug!("Opening document: {file_uri}");

        // Check if document is already opened
        if self.opened_documents.contains(&file_uri) {
            log::debug!("Document already opened: {file_uri}");
            return Ok(());
        }

        // Create textDocument/didOpen notification
        let did_open_notification = protocol::create_did_open_notification(&file_uri, content);

        // Send the notification
        protocol::send_message(&mut self.stdin, &did_open_notification)
            .await
            .map_err(|e| {
                LspError::SendFailed(std::io::Error::other(format!(
                    "Failed to send didOpen notification: {e}"
                )))
            })?;

        // Track the opened document
        self.opened_documents.insert(file_uri.clone());

        log::debug!("Waiting 1 second for ty to analyze document");

        // Wait 1 second for ty to analyze the document
        sleep(Duration::from_secs(1)).await;

        log::debug!("Document opened successfully: {file_uri}");

        Ok(())
    }

    /// Query hover information at a specific position.
    ///
    /// Sends a textDocument/hover request and returns the type information
    /// if available.
    ///
    /// # Arguments
    ///
    /// * `file_uri` - The file URI (e.g., "file:///path/to/file.py")
    /// * `line` - Zero-based line number
    /// * `character` - Zero-based character position
    ///
    /// # Returns
    ///
    /// Returns `Some(String)` with type information if available, or `None`
    /// if the hover response is empty.
    ///
    /// # Errors
    ///
    /// Returns an error if the request times out or fails.
    pub async fn hover(
        &mut self,
        file_uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<String>, LspError> {
        use std::sync::atomic::Ordering;
        use tokio::time::{timeout, Duration};

        // Get next request ID
        let request_id = self.request_id.fetch_add(1, Ordering::SeqCst);

        log::debug!("Sending hover request for {file_uri}:{line}:{character} (id={request_id})");

        // Create hover request
        let hover_request = protocol::create_hover_request(request_id, file_uri, line, character);

        // Send request and wait for response with 5s timeout
        let start_time = std::time::Instant::now();
        let total_timeout = Duration::from_secs(5);

        let response = timeout(total_timeout, async {
            // Send the request
            protocol::send_message(&mut self.stdin, &hover_request)
                .await
                .map_err(|e| {
                    LspError::SendFailed(std::io::Error::other(format!(
                        "Failed to send hover request: {e}"
                    )))
                })?;

            // Wait for response with matching ID
            // LSP servers may send notifications or other responses, so we need to loop
            // Use shorter timeouts per read to avoid blocking too long on any single message
            loop {
                // Calculate remaining time for this operation
                let elapsed = start_time.elapsed();
                if elapsed >= total_timeout {
                    return Err(LspError::Timeout(total_timeout));
                }

                let remaining = total_timeout
                    .checked_sub(elapsed)
                    .expect("elapsed time should not exceed total timeout");
                // Use shorter timeout per read (min of 1s or remaining time)
                let read_timeout = std::cmp::min(Duration::from_secs(1), remaining);

                let message = protocol::read_message(&mut self.stdout, read_timeout).await?;

                // Check if this is a response (has "id" field)
                if let Some(response_id) = message.get("id") {
                    // Check if the ID matches our request
                    if response_id.as_u64() == Some(request_id) {
                        log::debug!("Found matching response for request {request_id}");
                        return Ok::<serde_json::Value, LspError>(message);
                    }
                    log::warn!(
                        "Received response with mismatched ID: expected {request_id}, got {response_id:?}"
                    );
                    // Continue reading - this might be a response to a different request
                    continue;
                }
                // This is a notification (no ID field) - log and skip it
                if let Some(method) = message.get("method") {
                    log::debug!(
                        "Received notification while waiting for response: {method}"
                    );
                } else {
                    log::debug!("Received message without ID or method: {message:?}");
                }
            }
        })
        .await
        .map_err(|_| LspError::Timeout(total_timeout))??;

        log::debug!("Received hover response: {response:?}");

        // Check for error in response
        if let Some(error) = response.get("error") {
            return Err(LspError::ServerError(format!(
                "Hover request failed: {error}"
            )));
        }

        // Extract type information from the result
        let result = response.get("result");

        // If result is null or missing, return None
        if result.is_none_or(serde_json::Value::is_null) {
            log::debug!("Hover response has no result");
            return Ok(None);
        }

        let result = result.expect("result was just checked");

        // Extract contents field
        let contents = result.get("contents");

        // If contents is null or missing, return None
        if contents.is_none_or(serde_json::Value::is_null) {
            log::debug!("Hover response has no contents");
            return Ok(None);
        }

        let contents = contents.expect("contents was just checked");

        // Extract type information from contents
        // Contents can be:
        // 1. A string
        // 2. An object with "value" field (markdown/plaintext)
        // 3. An array of strings or objects
        let type_info = if let Some(value_str) = contents.as_str() {
            // Case 1: contents is a string
            Some(value_str.to_string())
        } else if let Some(value_obj) = contents.as_object() {
            // Case 2: contents is an object with "value" field
            value_obj
                .get("value")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string)
        } else if let Some(value_array) = contents.as_array() {
            // Case 3: contents is an array - join all values
            let values: Vec<String> = value_array
                .iter()
                .filter_map(|item| {
                    if let Some(s) = item.as_str() {
                        Some(s.to_string())
                    } else if let Some(obj) = item.as_object() {
                        obj.get("value")
                            .and_then(|v| v.as_str())
                            .map(std::string::ToString::to_string)
                    } else {
                        None
                    }
                })
                .collect();

            if values.is_empty() {
                None
            } else {
                Some(values.join("\n"))
            }
        } else {
            None
        };

        if type_info.is_some() {
            log::debug!("Extracted type info: {type_info:?}");
        } else {
            log::debug!("No type info found in hover response");
        }

        Ok(type_info)
    }

    /// Shutdown the LSP server gracefully.
    ///
    /// Sends a shutdown request followed by an exit notification, then waits
    /// for the process to exit.
    ///
    /// # Errors
    ///
    /// Returns an error if the shutdown request times out or fails.
    /// Shutdown the LSP server gracefully.
    ///
    /// This method:
    /// 1. Sends a shutdown request with 2s timeout
    /// 2. Sends an exit notification
    /// 3. Waits for the process to exit
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The shutdown request times out
    /// - Communication with the server fails
    pub async fn shutdown(mut self) -> Result<(), LspError> {
        use std::time::Duration;
        use tokio::time::timeout;

        log::debug!("Shutting down ty LSP server");

        // Get the next request ID
        let request_id = self
            .request_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Send shutdown request
        let shutdown_request = protocol::create_shutdown_request(request_id);
        protocol::send_message(&mut self.stdin, &shutdown_request).await?;

        log::debug!("Sent shutdown request, waiting for response");

        // Wait for shutdown response with 2s timeout
        let _response = timeout(
            Duration::from_secs(2),
            protocol::read_message(&mut self.stdout, Duration::from_secs(2)),
        )
        .await
        .map_err(|_| LspError::Timeout(Duration::from_secs(2)))??;

        log::debug!("Received shutdown response, sending exit notification");

        // Send exit notification
        let exit_notification = protocol::create_exit_notification();
        protocol::send_message(&mut self.stdin, &exit_notification).await?;

        log::debug!("Sent exit notification, waiting for process to exit");

        // Wait for the process to exit
        let status = self.process.wait().await.map_err(LspError::SendFailed)?;

        log::debug!("ty LSP server exited with status: {status:?}");

        Ok(())
    }
}

impl Drop for TyLspClient {
    fn drop(&mut self) {
        log::debug!("Dropping TyLspClient, ensuring ty server process is terminated");

        // Ensure process is killed if not already shut down
        let _ = self.process.start_kill();

        // Wait briefly for process to exit (non-blocking check)
        // We use try_wait() in a loop since Drop is synchronous
        for _ in 0..10 {
            if let Ok(Some(status)) = self.process.try_wait() {
                // Process has exited
                log::debug!("ty server process exited with status: {status:?}");
                break;
            }
            // Brief sleep to give process time to exit
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_hover_response_parsing_string_contents() {
        // Test parsing hover response with string contents
        use serde_json::json;

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "contents": "str"
            }
        });

        // Extract type info using the same logic as hover()
        let result = response.get("result").unwrap();
        let contents = result.get("contents").unwrap();
        let type_info = contents.as_str().map(|s| s.to_string());

        assert_eq!(type_info, Some("str".to_string()));
    }

    #[test]
    fn test_hover_response_parsing_object_contents() {
        // Test parsing hover response with object contents (markdown format)
        use serde_json::json;

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "contents": {
                    "kind": "markdown",
                    "value": "```python\ns3_client: S3Client\n```"
                }
            }
        });

        // Extract type info using the same logic as hover()
        let result = response.get("result").unwrap();
        let contents = result.get("contents").unwrap();
        let type_info = contents
            .as_object()
            .and_then(|obj| obj.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        assert_eq!(
            type_info,
            Some("```python\ns3_client: S3Client\n```".to_string())
        );
    }

    #[test]
    fn test_hover_response_parsing_null_result() {
        // Test parsing hover response with null result
        use serde_json::json;

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": null
        });

        // Extract type info using the same logic as hover()
        let result = response.get("result");
        let is_null = result.is_none() || result.unwrap().is_null();

        assert!(is_null);
    }

    #[test]
    fn test_hover_response_parsing_null_contents() {
        // Test parsing hover response with null contents
        use serde_json::json;

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "contents": null
            }
        });

        // Extract type info using the same logic as hover()
        let result = response.get("result").unwrap();
        let contents = result.get("contents");
        let is_null = contents.is_none() || contents.unwrap().is_null();

        assert!(is_null);
    }

    #[test]
    fn test_hover_response_parsing_array_contents() {
        // Test parsing hover response with array contents
        use serde_json::json;

        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "contents": [
                    "Type: str",
                    {"kind": "markdown", "value": "Additional info"}
                ]
            }
        });

        // Extract type info using the same logic as hover()
        let result = response.get("result").unwrap();
        let contents = result.get("contents").unwrap();

        if let Some(array) = contents.as_array() {
            let values: Vec<String> = array
                .iter()
                .filter_map(|item| {
                    if let Some(s) = item.as_str() {
                        Some(s.to_string())
                    } else if let Some(obj) = item.as_object() {
                        obj.get("value")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect();

            let type_info = if values.is_empty() {
                None
            } else {
                Some(values.join("\n"))
            };

            assert_eq!(type_info, Some("Type: str\nAdditional info".to_string()));
        } else {
            panic!("Expected array contents");
        }
    }
}
