//! Integration tests for the LSP client.
//!
//! These tests verify end-to-end behavior with a real ty server.
//! Tests require ty and boto3-stubs to be installed.
//! Tests run sequentially using #[serial] to avoid LSP server conflicts.

use iam_policy_autopilot_policy_generation::lsp::{
    test_utils::{fixtures, is_lsp_ready},
    LspError, TyLspClient,
};
use serial_test::serial;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;

/// Test fixture containing a temporary workspace with Python files.
pub struct TestWorkspace {
    /// Temporary directory that will be cleaned up when dropped
    pub temp_dir: TempDir,
    /// Path to the workspace root
    pub root_path: PathBuf,
}

impl TestWorkspace {
    /// Create a new test workspace with a temporary directory.
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = TempDir::new()?;
        let root_path = temp_dir.path().to_path_buf();
        Ok(Self {
            temp_dir,
            root_path,
        })
    }

    /// Create a Python file in the workspace with the given content.
    ///
    /// # Arguments
    ///
    /// * `relative_path` - Path relative to workspace root (e.g., "test.py" or "src/app.py")
    /// * `content` - Python source code content
    pub async fn create_file(
        &self,
        relative_path: &str,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        let file_path = self.root_path.join(relative_path);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write the file
        fs::write(&file_path, content).await?;

        Ok(file_path)
    }

    /// Get the absolute path for a file in the workspace.
    pub fn file_path(&self, relative_path: &str) -> PathBuf {
        self.root_path.join(relative_path)
    }

    /// Get the file:// URI for a file in the workspace.
    pub fn file_uri(&self, relative_path: &str) -> String {
        use lsp_types::Url;
        let path = self.file_path(relative_path);
        Url::from_file_path(&path)
            .map(|url| url.to_string())
            .unwrap_or_else(|_| format!("file://{}", path.display()))
    }
}

/// Helper macro to skip tests if ty and boto3-stubs are not available
macro_rules! skip_if_no_lsp {
    () => {
        if !is_lsp_ready() {
            eprintln!("Skipping test: ty or boto3-stubs not available");
            eprintln!("  Install ty: pip install ty");
            eprintln!("  Install boto3-stubs: pip install 'boto3-stubs[essential|full|all]'");
            return;
        }
    };
}

#[tokio::test]
#[serial]
async fn test_full_lifecycle() {
    skip_if_no_lsp!();

    // Create a test workspace
    let workspace = TestWorkspace::new().expect("Failed to create workspace");

    // Create a Python file
    let file_path = workspace
        .create_file("test.py", fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to create file");

    eprintln!("Test file content:\n{}", fixtures::SIMPLE_BOTO3);

    // Create and initialize LSP client
    let mut client: TyLspClient = TyLspClient::new(&workspace.root_path)
        .await
        .expect("Failed to create LSP client");

    // Open the document
    let _: () = client
        .open_document(&file_path, fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to open document");

    let file_uri = workspace.file_uri("test.py");

    // Test 1: s3_client variable should return S3Client type
    eprintln!("\nTest 1: s3_client variable at line 2, char 0");
    let hover_s3_client: Option<String> = client
        .hover(&file_uri, 2, 0)
        .await
        .expect("Failed to query hover");
    eprintln!("Result: {:?}", hover_s3_client);

    assert!(
        hover_s3_client.is_some(),
        "Expected type information for s3_client variable"
    );
    let s3_client_type = hover_s3_client.unwrap();
    assert!(
        s3_client_type.contains("S3Client"),
        "Expected S3Client type, got: {}",
        s3_client_type
    );

    // Test 2: response variable should return ListBucketsOutputTypeDef
    eprintln!("\nTest 2: response variable at line 3, char 0");
    let hover_response: Option<String> = client
        .hover(&file_uri, 3, 0)
        .await
        .expect("Failed to query hover");
    eprintln!("Result: {:?}", hover_response);

    assert!(
        hover_response.is_some(),
        "Expected type information for response variable"
    );
    let response_type = hover_response.unwrap();
    assert!(
        response_type.contains("ListBucketsOutputTypeDef")
            || response_type.contains("ListBucketsOutput"),
        "Expected ListBucketsOutputTypeDef type, got: {}",
        response_type
    );

    // Test 3: list_buckets method should return method signature
    eprintln!("\nTest 3: list_buckets method at line 3, char 21");
    let hover_method: Option<String> = client
        .hover(&file_uri, 3, 21)
        .await
        .expect("Failed to query hover");
    eprintln!("Result: {:?}", hover_method);

    assert!(
        hover_method.is_some(),
        "Expected type information for list_buckets method"
    );
    let method_sig = hover_method.unwrap();
    assert!(
        method_sig.contains("list_buckets") && method_sig.contains("ListBucketsOutput"),
        "Expected list_buckets method signature, got: {}",
        method_sig
    );

    // Shutdown gracefully
    let _: () = client.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
#[serial]
async fn test_multiple_documents() {
    skip_if_no_lsp!();

    let workspace = TestWorkspace::new().expect("Failed to create workspace");

    // Create multiple Python files
    let file1 = workspace
        .create_file("file1.py", fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to create file1");

    let file2 = workspace
        .create_file("file2.py", fixtures::MULTIPLE_SERVICES)
        .await
        .expect("Failed to create file2");

    // Create LSP client
    let mut client: TyLspClient = TyLspClient::new(&workspace.root_path)
        .await
        .expect("Failed to create LSP client");

    // Open both documents
    let _: () = client
        .open_document(&file1, fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to open file1");

    let _: () = client
        .open_document(&file2, fixtures::MULTIPLE_SERVICES)
        .await
        .expect("Failed to open file2");

    // Query hover on both files at meaningful positions
    let uri1 = workspace.file_uri("file1.py");
    // Line 2, char 0: s3_client variable
    let hover1: Option<String> = client
        .hover(&uri1, 2, 0)
        .await
        .expect("Failed to hover file1");
    eprintln!("File1 s3_client type: {:?}", hover1);

    assert!(hover1.is_some(), "Expected type info for file1 s3_client");
    assert!(
        hover1.as_ref().unwrap().contains("S3Client"),
        "Expected S3Client type in file1, got: {:?}",
        hover1
    );

    let uri2 = workspace.file_uri("file2.py");
    // Line 2, char 0: s3 variable
    let hover2: Option<String> = client
        .hover(&uri2, 2, 0)
        .await
        .expect("Failed to hover file2");
    eprintln!("File2 s3 type: {:?}", hover2);

    assert!(hover2.is_some(), "Expected type info for file2 s3");
    assert!(
        hover2.as_ref().unwrap().contains("S3Client"),
        "Expected S3Client type in file2, got: {:?}",
        hover2
    );

    // Shutdown
    let _: () = client.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
#[serial]
async fn test_hover_on_empty_file() {
    skip_if_no_lsp!();

    let workspace = TestWorkspace::new().expect("Failed to create workspace");

    let file_path = workspace
        .create_file("empty.py", fixtures::EMPTY)
        .await
        .expect("Failed to create file");

    let mut client: TyLspClient = TyLspClient::new(&workspace.root_path)
        .await
        .expect("Failed to create LSP client");

    let _: () = client
        .open_document(&file_path, fixtures::EMPTY)
        .await
        .expect("Failed to open document");

    // Query hover on empty file should return None
    let file_uri = workspace.file_uri("empty.py");
    let hover_result: Option<String> = client
        .hover(&file_uri, 0, 0)
        .await
        .expect("Failed to query hover");

    assert!(
        hover_result.is_none(),
        "Expected None for hover on empty file"
    );

    let _: () = client.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
#[serial]
async fn test_hover_on_invalid_position() {
    skip_if_no_lsp!();

    let workspace = TestWorkspace::new().expect("Failed to create workspace");

    let file_path = workspace
        .create_file("test.py", fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to create file");

    let mut client: TyLspClient = TyLspClient::new(&workspace.root_path)
        .await
        .expect("Failed to create LSP client");

    let _: () = client
        .open_document(&file_path, fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to open document");

    // Query hover at an invalid position (line 1000)
    let file_uri = workspace.file_uri("test.py");
    let hover_result: Option<String> = client
        .hover(&file_uri, 1000, 0)
        .await
        .expect("Failed to query hover");

    // Should return None for invalid position
    assert!(
        hover_result.is_none(),
        "Expected None for hover on invalid position"
    );

    let _: () = client.shutdown().await.expect("Failed to shutdown");
}

#[tokio::test]
#[serial]
async fn test_ty_not_found_error() {
    // Temporarily modify PATH to exclude ty
    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", "");

    let workspace = TestWorkspace::new().expect("Failed to create workspace");
    let result: Result<TyLspClient, LspError> = TyLspClient::new(&workspace.root_path).await;

    // Restore original PATH
    std::env::set_var("PATH", &original_path);

    // Verify we get TyNotFound error
    assert!(result.is_err());
    match result {
        Err(LspError::TyNotFound) => {
            // Expected error
        }
        _ => panic!("Expected TyNotFound error, got: {:?}", result),
    }
}

#[tokio::test]
#[serial]
async fn test_open_same_document_twice() {
    skip_if_no_lsp!();

    let workspace = TestWorkspace::new().expect("Failed to create workspace");

    let file_path = workspace
        .create_file("test.py", fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to create file");

    let mut client: TyLspClient = TyLspClient::new(&workspace.root_path)
        .await
        .expect("Failed to create LSP client");

    // Open the document twice - should not error
    let _: () = client
        .open_document(&file_path, fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to open document first time");

    let _: () = client
        .open_document(&file_path, fixtures::SIMPLE_BOTO3)
        .await
        .expect("Failed to open document second time");

    let _: () = client.shutdown().await.expect("Failed to shutdown");
}
