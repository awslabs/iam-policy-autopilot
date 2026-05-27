//! Integration tests for the LSP client.
//!
//! These tests verify end-to-end behavior with a real ty server.
//! Tests require ty and boto3-stubs to be installed.
//! Tests run sequentially using #[serial] to avoid LSP server conflicts.
//!
//! Run with: `cargo test --features integ-test -- --ignored`

use iam_policy_autopilot_policy_generation::lsp::{
    test_utils::{find_position, python},
    LspError, TyLspClient,
};
use rstest::rstest;
use serial_test::serial;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;

struct TestWorkspace {
    _temp_dir: TempDir,
    root_path: PathBuf,
}

impl TestWorkspace {
    fn new() -> std::io::Result<Self> {
        let temp_dir = TempDir::new()?;
        let root_path = temp_dir.path().to_path_buf();

        // ty resolves third-party imports by looking for a .venv in the project root.
        // Since test workspaces are temp directories, we write a pyproject.toml pointing
        // ty to the active Python environment so it can find boto3-stubs.
        let python_prefix = std::process::Command::new("python3")
            .args(["-c", "import sys; print(sys.prefix)"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        let python_prefix = python_prefix.trim();

        std::fs::write(
            root_path.join("pyproject.toml"),
            format!("[tool.ty.environment]\npython = \"{python_prefix}\"\n"),
        )?;

        Ok(Self {
            _temp_dir: temp_dir,
            root_path,
        })
    }

    async fn create_file(&self, relative_path: &str, content: &str) -> std::io::Result<PathBuf> {
        let file_path = self.root_path.join(relative_path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content).await?;
        Ok(file_path)
    }

    fn file_uri(&self, relative_path: &str) -> String {
        let path = self.root_path.join(relative_path);
        lsp_types::Url::from_file_path(&path)
            .map(|url| url.to_string())
            .unwrap_or_else(|_| format!("file://{}", path.display()))
    }
}

#[rstest]
#[case("s3_client", "S3Client")]
#[case("response", "ListBuckets")]
#[case("list_buckets", "list_buckets")]
#[tokio::test]
#[serial]
#[ignore]
async fn test_hover_returns_expected_type(#[case] needle: &str, #[case] expected: &str) {
    if !python::is_ready() {
        panic!("LSP integration tests require ty + boto3-stubs");
    }

    let workspace = TestWorkspace::new().unwrap();
    let file_path = workspace
        .create_file("test.py", python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    let mut client = TyLspClient::create(&workspace.root_path).await.unwrap();
    client
        .open_document(&file_path, python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    let (line, col) = find_position(python::fixtures::SIMPLE_BOTO3, needle);
    let hover = client
        .hover(&workspace.file_uri("test.py"), line, col)
        .await
        .unwrap();

    assert!(
        hover.as_ref().is_some_and(|h| h.contains(expected)),
        "Expected hover at '{needle}' to contain '{expected}', got: {hover:?}"
    );

    client.shutdown().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_multiple_documents() {
    if !python::is_ready() {
        panic!("LSP integration tests require ty + boto3-stubs");
    }

    let workspace = TestWorkspace::new().unwrap();
    let file1 = workspace
        .create_file("file1.py", python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();
    let file2 = workspace
        .create_file("file2.py", python::fixtures::MULTIPLE_SERVICES)
        .await
        .unwrap();

    let mut client = TyLspClient::create(&workspace.root_path).await.unwrap();
    client
        .open_document(&file1, python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();
    client
        .open_document(&file2, python::fixtures::MULTIPLE_SERVICES)
        .await
        .unwrap();

    let (line, col) = find_position(python::fixtures::SIMPLE_BOTO3, "s3_client");
    let hover1 = client
        .hover(&workspace.file_uri("file1.py"), line, col)
        .await
        .unwrap();
    assert!(
        hover1.as_ref().is_some_and(|h| h.contains("S3Client")),
        "Expected S3Client in file1, got: {hover1:?}"
    );

    let (line, col) = find_position(python::fixtures::MULTIPLE_SERVICES, "s3 =");
    let hover2 = client
        .hover(&workspace.file_uri("file2.py"), line, col)
        .await
        .unwrap();
    assert!(
        hover2.as_ref().is_some_and(|h| h.contains("S3Client")),
        "Expected S3Client in file2, got: {hover2:?}"
    );

    client.shutdown().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_hover_on_empty_file() {
    if !python::is_ready() {
        panic!("LSP integration tests require ty + boto3-stubs");
    }

    let workspace = TestWorkspace::new().unwrap();
    let file_path = workspace
        .create_file("empty.py", python::fixtures::EMPTY)
        .await
        .unwrap();

    let mut client = TyLspClient::create(&workspace.root_path).await.unwrap();
    client
        .open_document(&file_path, python::fixtures::EMPTY)
        .await
        .unwrap();

    let hover = client
        .hover(&workspace.file_uri("empty.py"), 0, 0)
        .await
        .unwrap();
    assert!(hover.is_none(), "Expected None for hover on empty file");

    client.shutdown().await.unwrap();
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_hover_on_invalid_position() {
    if !python::is_ready() {
        panic!("LSP integration tests require ty + boto3-stubs");
    }

    let workspace = TestWorkspace::new().unwrap();
    let file_path = workspace
        .create_file("test.py", python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    let mut client = TyLspClient::create(&workspace.root_path).await.unwrap();
    client
        .open_document(&file_path, python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    let hover = client
        .hover(&workspace.file_uri("test.py"), 1000, 0)
        .await
        .unwrap();
    assert!(hover.is_none(), "Expected None for invalid position");

    client.shutdown().await.unwrap();
}

#[tokio::test]
#[serial]
async fn test_server_not_found_error() {
    let workspace = TestWorkspace::new().unwrap();

    let result = temp_env::async_with_vars([("PATH", Some(""))], async {
        TyLspClient::create(&workspace.root_path).await
    })
    .await;

    assert!(matches!(result, Err(LspError::ServerNotFound(_))));
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_open_same_document_twice() {
    if !python::is_ready() {
        panic!("LSP integration tests require ty + boto3-stubs");
    }

    let workspace = TestWorkspace::new().unwrap();
    let file_path = workspace
        .create_file("test.py", python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    let mut client = TyLspClient::create(&workspace.root_path).await.unwrap();
    client
        .open_document(&file_path, python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();
    client
        .open_document(&file_path, python::fixtures::SIMPLE_BOTO3)
        .await
        .unwrap();

    client.shutdown().await.unwrap();
}
