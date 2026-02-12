//! Error types for LSP operations.

use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during LSP operations.
#[derive(Debug, Error)]
pub enum LspError {
    /// The ty command was not found in PATH.
    #[error("ty command not found in PATH")]
    TyNotFound,

    /// Failed to start the ty server process.
    #[error("Failed to start ty server: {0}")]
    StartupFailed(String),

    /// Failed to initialize the ty server.
    #[error("Failed to initialize ty server: {0}")]
    InitializeFailed(String),

    /// An LSP operation timed out.
    #[error("LSP operation timed out after {0:?}")]
    Timeout(Duration),

    /// Failed to send a message to the LSP server.
    #[error("Failed to send message: {0}")]
    SendFailed(#[from] std::io::Error),

    /// Failed to parse an LSP response.
    #[error("Failed to parse LSP response: {0}")]
    ParseFailed(String),

    /// The LSP server returned an error.
    #[error("LSP server returned error: {0}")]
    ServerError(String),
}
