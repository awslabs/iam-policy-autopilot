# Plan 1: gopls LSP Integration

## Goal

Add gopls support to the existing `src/lsp/` module — server config, client alias, and the LSP
methods needed for call graph construction. No feature flag needed here; this extends the LSP
infrastructure that already exists (same pattern as `TyConfig`/`TyLspClient`).

## Scope

- `src/lsp/gopls.rs` — `GoplsConfig`, `GoplsClient` type alias, convenience constructor
- LSP method extensions on `LspClient<C>` — `document_symbols`, `prepare_call_hierarchy`, `outgoing_calls`
- Integration tests against a real gopls instance

## Files to Create/Modify

### New: `src/lsp/gopls.rs`

```rust
use std::path::Path;
use super::{LspClient, LspError, LspServerConfig};

/// Configuration for the gopls Go language server.
pub struct GoplsConfig;

impl LspServerConfig for GoplsConfig {
    fn binary_name(&self) -> &'static str { "gopls" }
    fn args(&self) -> &[&str] { &["serve"] }
    fn language_id(&self) -> &'static str { "go" }
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
        options: super::LspClientOptions,
    ) -> Result<Self, LspError> {
        Self::with_options(GoplsConfig, workspace_root, options).await
    }
}
```

### Modify: `src/lsp/mod.rs`

Add `pub mod gopls;` and new LSP methods:

```rust
use lsp_types::{
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse,
};

impl<C: LspServerConfig> LspClient<C> {
    /// Get document symbols (functions, methods, classes) for a file.
    pub async fn document_symbols(
        &mut self,
        file_uri: &str,
    ) -> Result<Option<DocumentSymbolResponse>, LspError> {
        // Build DocumentSymbolParams, send request, parse response
    }

    /// Prepare call hierarchy at a given position.
    ///
    /// Returns the call hierarchy items at the position, typically one item
    /// representing the function/method at that location.
    pub async fn prepare_call_hierarchy(
        &mut self,
        file_uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<Vec<CallHierarchyItem>>, LspError> {
        // Build CallHierarchyPrepareParams, send request
    }

    /// Get outgoing calls from a call hierarchy item.
    ///
    /// Returns all functions/methods called from the given item.
    pub async fn outgoing_calls(
        &mut self,
        item: CallHierarchyItem,
    ) -> Result<Vec<CallHierarchyOutgoingCall>, LspError> {
        // Build CallHierarchyOutgoingCallsParams, send request
    }
}
```

## gopls Specifics

- **Requires a Go module**: gopls won't index bare `.go` files. The workspace must contain `go.mod`.
- **Initialization time**: gopls indexes the full module graph on startup. Use longer timeouts:
  - `initialize_timeout`: 30s
  - `open_document_timeout`: 10s
- **`documentSymbol`**: Returns all functions/methods/types in a file with their ranges.
- **`prepareCallHierarchy`**: Given a position on a function name, returns a `CallHierarchyItem`.
- **`outgoingCalls`**: Given a `CallHierarchyItem`, returns all calls from that function with target items.
- **Binary location**: May not be in PATH (e.g., `~/go/bin/gopls`). Consider accepting a path override
  in `GoplsConfig` or via environment variable.

## Testing

Integration tests behind `#[cfg(feature = "integ-test")]`:

1. **Test fixture**: Small Go module with known functions and call relationships
2. **`test_gopls_document_symbols`**: Verify function discovery + ranges
3. **`test_gopls_prepare_call_hierarchy`**: Verify item resolution at function position
4. **`test_gopls_outgoing_calls`**: Verify outgoing call edges

### Test file

New file: `tests/gopls_integration_tests.rs` (same pattern as `tests/lsp_integration_tests.rs`)

Tests use `#[ignore]` + `#[serial]` and a `GoTestWorkspace` helper that creates a temp directory
with `go.mod` and source files.

### Readiness check

```rust
pub mod go {
    pub fn is_ready() -> bool {
        which::which("gopls").is_ok()
    }
}
```

Requirements:
- `gopls` binary in PATH
- Test fixture Go module with `go.mod` and source files

## CI Setup

Update `.github/workflows/pr-checks.yml` to run gopls integration tests alongside the existing
Python LSP tests. The `lsp-integration` job needs Go + gopls installed:

```yaml
  lsp-integration:
    name: LSP Integration Tests
    needs: [changes]
    if: needs.changes.outputs.rust == 'true'
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v4
        with:
          submodules: recursive

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      # Python LSP (ty) dependencies
      - name: Install Python LSP test dependencies
        run: |
          python3 -m venv .lsp-venv
          .lsp-venv/bin/pip install 'ty>=0.0.39,<0.1' boto3 'boto3-stubs[essential]'
          echo "${{ github.workspace }}/.lsp-venv/bin" >> $GITHUB_PATH

      # Go LSP (gopls) dependencies
      - uses: actions/setup-go@v5
        with:
          go-version: 'stable'

      - name: Install gopls
        run: go install golang.org/x/tools/gopls@latest

      # Run all LSP integration tests
      - name: Run Python LSP integration tests
        run: cargo test -p iam-policy-autopilot-policy-generation --features integ-test --test lsp_integration_tests -- --ignored

      - name: Run Go LSP integration tests
        run: cargo test -p iam-policy-autopilot-policy-generation --features integ-test --test gopls_integration_tests -- --ignored
```

Key points:
- `actions/setup-go@v5` installs Go and adds `$GOPATH/bin` to PATH
- `go install golang.org/x/tools/gopls@latest` installs gopls to `$GOPATH/bin`
- Separate test commands for Python and Go LSP tests (clear failure attribution)

## Dependencies

- `lsp-types` 0.95 (already in Cargo.lock) — has `CallHierarchyItem`, `CallHierarchyOutgoingCall`, etc.
- `async-lsp` 0.2 (already a dependency) — has `prepare_call_hierarchy`, `outgoing_calls`, `document_symbol` on `LanguageServer`

## Out of Scope

- `CallGraph` type and `CallGraphBuilder` trait (Plan 2)
- `model_generation` module (Plan 3)
- Feature flags (added in Plan 2)
