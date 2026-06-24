//! Integration tests for the gopls LSP client.
//!
//! These tests verify end-to-end behavior with a real gopls server.
//! Tests require gopls and the Go toolchain to be installed.
//! Tests run sequentially using #[serial] to avoid LSP server conflicts.
//!
//! Run with: `cargo test --features integ-test --test gopls_integration_tests -- --ignored`

use iam_policy_autopilot_policy_generation::lsp::{
    gopls::GoplsClient,
    test_utils::{find_position, go},
    LspClientOptions, LspError,
};
use lsp_types::{DocumentSymbolResponse, SymbolKind, Url};
use rstest::rstest;
use serial_test::serial;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::fs;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct GoTestWorkspace {
    _temp_dir: TempDir,
    root_path: PathBuf,
}

impl GoTestWorkspace {
    async fn new() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        fs::write(root_path.join("go.mod"), go::fixtures::GO_MOD)
            .await
            .unwrap();
        Self {
            _temp_dir: temp_dir,
            root_path,
        }
    }

    async fn write_file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.root_path.join(name);
        fs::write(&path, content).await.unwrap();
        path
    }

    fn uri(&self, name: &str) -> Url {
        let path = self.root_path.join(name);
        Url::from_file_path(&path).unwrap()
    }
}

/// Open a workspace with a single Go file and return the connected client.
async fn open_workspace(fixture: &str) -> (GoTestWorkspace, GoplsClient) {
    let workspace = GoTestWorkspace::new().await;
    let file_path = workspace.write_file("main.go", fixture).await;

    let options = LspClientOptions {
        initialize_timeout: Duration::from_secs(30),
        open_document_timeout: Duration::from_secs(10),
        hover_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
    };

    let mut client = GoplsClient::create_with_options(&workspace.root_path, options)
        .await
        .unwrap();

    client.open_document(&file_path, fixture).await.unwrap();

    (workspace, client)
}

/// Get outgoing call names from a function.
///
/// `needle` is searched in `source` to find the line. If it starts with "func ",
/// the column is advanced past the prefix to land on the function name identifier.
async fn outgoing_call_names(
    client: &mut GoplsClient,
    uri: &Url,
    source: &str,
    needle: &str,
) -> Vec<String> {
    let (line, col) = find_position(source, needle);
    let col = if needle.starts_with("func ") {
        col + 5
    } else {
        col
    };

    let items = client
        .prepare_call_hierarchy(uri, line, col)
        .await
        .unwrap()
        .expect("Expected call hierarchy items");

    let item = items.into_iter().next().unwrap();
    client
        .outgoing_calls(item)
        .await
        .unwrap()
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.to.name)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests: document symbols
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
#[ignore]
async fn test_document_symbols_lists_all_functions() {
    if !go::is_ready() {
        panic!("gopls integration tests require Go + gopls");
    }

    let (workspace, mut client) = open_workspace(go::fixtures::SIMPLE_CALL_CHAIN).await;

    let symbols = client
        .document_symbols(&workspace.uri("main.go"))
        .await
        .unwrap()
        .expect("Expected symbols");

    let names: Vec<String> = match symbols {
        DocumentSymbolResponse::Flat(infos) => infos.into_iter().map(|s| s.name).collect(),
        DocumentSymbolResponse::Nested(syms) => syms.into_iter().map(|s| s.name).collect(),
    };

    for expected in ["main", "helper", "deepHelper", "unrelated"] {
        assert!(
            names.contains(&expected.to_string()),
            "Missing symbol '{expected}', found: {names:?}"
        );
    }

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests: prepare call hierarchy
// ---------------------------------------------------------------------------

#[rstest]
#[case("func helper", "helper", SymbolKind::FUNCTION)]
#[case("func main", "main", SymbolKind::FUNCTION)]
#[case("func deepHelper", "deepHelper", SymbolKind::FUNCTION)]
#[tokio::test]
#[serial]
#[ignore]
async fn test_prepare_call_hierarchy_resolves_function(
    #[case] needle: &str,
    #[case] expected_name: &str,
    #[case] expected_kind: SymbolKind,
) {
    if !go::is_ready() {
        panic!("gopls integration tests require Go + gopls");
    }

    let (workspace, mut client) = open_workspace(go::fixtures::SIMPLE_CALL_CHAIN).await;

    let (line, col) = find_position(go::fixtures::SIMPLE_CALL_CHAIN, needle);
    // Advance past "func " to land on the name; for method receivers advance past the
    // full needle since it already points at the name.
    let col = if needle.starts_with("func ") {
        col + 5
    } else {
        col
    };

    let items = client
        .prepare_call_hierarchy(&workspace.uri("main.go"), line, col)
        .await
        .unwrap()
        .expect("Expected call hierarchy items");

    assert!(!items.is_empty());
    assert_eq!(items[0].name, expected_name);
    assert_eq!(items[0].kind, expected_kind);

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests: outgoing calls
// ---------------------------------------------------------------------------

#[rstest]
#[case(go::fixtures::SIMPLE_CALL_CHAIN, "func main", &["helper"])]
#[case(go::fixtures::SIMPLE_CALL_CHAIN, "func helper", &["deepHelper"])]
#[case(go::fixtures::SIMPLE_CALL_CHAIN, "func deepHelper", &[])]
#[tokio::test]
#[serial]
#[ignore]
async fn test_outgoing_calls(
    #[case] fixture: &str,
    #[case] needle: &str,
    #[case] expected_callees: &[&str],
) {
    if !go::is_ready() {
        panic!("gopls integration tests require Go + gopls");
    }

    let (workspace, mut client) = open_workspace(fixture).await;

    let callee_names =
        outgoing_call_names(&mut client, &workspace.uri("main.go"), fixture, needle).await;

    for expected in expected_callees {
        assert!(
            callee_names.iter().any(|n| n == expected),
            "Expected '{expected}' in outgoing calls, got: {callee_names:?}"
        );
    }

    if expected_callees.is_empty() {
        assert!(
            callee_names.is_empty(),
            "Expected no outgoing calls, got: {callee_names:?}"
        );
    }

    client.shutdown().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_struct_method_outgoing_calls() {
    if !go::is_ready() {
        panic!("gopls integration tests require Go + gopls");
    }

    let (workspace, mut client) = open_workspace(go::fixtures::STRUCT_METHODS).await;

    let callee_names = outgoing_call_names(
        &mut client,
        &workspace.uri("main.go"),
        go::fixtures::STRUCT_METHODS,
        "HandleRequest",
    )
    .await;

    assert!(
        callee_names.contains(&"fetchData".to_string()),
        "HandleRequest should call fetchData, got: {callee_names:?}"
    );
    assert!(
        callee_names.contains(&"format".to_string()),
        "HandleRequest should call format, got: {callee_names:?}"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests: error handling
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn test_server_not_found_when_gopls_missing() {
    let workspace = GoTestWorkspace::new().await;

    let result = temp_env::async_with_vars([("PATH", Some(""))], async {
        GoplsClient::create(&workspace.root_path).await
    })
    .await;

    assert!(matches!(result, Err(LspError::ServerNotFound(_))));
}
