use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// Errors that can occur during LSP operations.
#[derive(Debug, Error)]
pub enum LspError {
    /// The language server binary was not found in PATH.
    #[error("{0} not found in PATH")]
    ServerNotFound(String),

    /// Failed to start the language server process.
    #[error("Failed to start language server: {0}")]
    StartupFailed(String),

    /// Failed to initialize the language server.
    #[error("Failed to initialize language server: {0}")]
    InitializeFailed(String),

    /// An LSP operation timed out.
    #[error("LSP operation timed out after {0:?}")]
    Timeout(Duration),

    /// Failed to send a message to the language server.
    #[error("Failed to send message: {0}")]
    SendFailed(#[from] std::io::Error),

    /// Failed to parse an LSP response.
    #[error("Failed to parse LSP response: {0}")]
    ParseFailed(String),

    /// Failed to convert a file path to a URI.
    #[error("Failed to convert a file path to a URI: {0}")]
    InvalidPath(PathBuf),

    /// The language server returned an error.
    #[error("Language server error: {0}")]
    ServerError(String),
}
