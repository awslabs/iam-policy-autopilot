use std::path::Path;

use super::{LspClient, LspClientOptions, LspError, LspServerConfig};

/// Configuration for the gopls Go language server.
pub struct GoplsConfig;

impl LspServerConfig for GoplsConfig {
    fn binary_name(&self) -> &'static str {
        "gopls"
    }

    fn args(&self) -> &[&str] {
        &["serve"]
    }

    fn language_id(&self) -> &'static str {
        "go"
    }
}

/// Convenience type alias for a gopls LSP client.
pub type GoplsClient = LspClient<GoplsConfig>;

impl GoplsClient {
    /// Create a new gopls LSP client with default options.
    pub async fn create(workspace_root: impl AsRef<Path>) -> Result<Self, LspError> {
        Self::new(GoplsConfig, workspace_root).await
    }

    /// Create a new gopls LSP client with custom options.
    pub async fn create_with_options(
        workspace_root: impl AsRef<Path>,
        options: LspClientOptions,
    ) -> Result<Self, LspError> {
        Self::with_options(GoplsConfig, workspace_root, options).await
    }
}
