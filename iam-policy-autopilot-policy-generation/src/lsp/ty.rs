//! [ty](https://github.com/astral-sh/ty) Python type checker client.
//!
//! Concrete [`LspServerConfig`] and a [`LspClient`] alias specialized for ty,
//! layered on the generic client in the parent module.

use std::path::Path;

use super::{LspClient, LspClientOptions, LspError, LspServerConfig};

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
