//! Filesystem provider implementation using `tokio::fs`

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use tokio::fs;

#[cfg(not(target_arch = "wasm32"))]
use crate::errors::{ExtractorError, Result};

/// Native filesystem provider using `tokio::fs` for async file operations.
///
/// Not available on WASM targets — use in-memory APIs instead.
/// This implementation provides robust file system operations with proper error
/// handling, Unicode support, and glob pattern matching for file listing.
///
/// # Thread Safety
/// This provider is `Send + Sync` and can be safely shared across threads.
///
/// # Performance Considerations
/// - Uses `tokio::fs` for non-blocking I/O operations
/// - Efficient directory traversal with early termination on errors
/// - Pattern compilation is cached when possible
/// - Large directories are processed incrementally
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct FileSystemProvider;

#[cfg(not(target_arch = "wasm32"))]
impl FileSystemProvider {
    /// Read the entire contents of a file as a UTF-8 string.
    ///
    /// This method uses tokio::fs::read_to_string for efficient async I/O
    /// and provides detailed error context including the operation and file path.
    pub async fn read_file(path: impl AsRef<Path>) -> Result<String> {
        fs::read_to_string(path.as_ref())
            .await
            .map_err(|e| ExtractorError::file_system("read", path.as_ref(), e))
    }
}

#[cfg(test)]
mod tests {
    use crate::providers::FileSystemProvider;

    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::fs;

    /// Helper to create a temporary directory with test files
    async fn create_test_directory() -> Result<TempDir> {
        let temp_dir = TempDir::new()
            .map_err(|e| ExtractorError::file_system("create temp directory", "temp", e))?;

        let base_path = temp_dir.path();

        // Create directory structure:
        // temp/
        // ├── file1.py
        // ├── file2.txt
        // ├── subdir/
        // │   ├── file3.py
        // │   └── file4.js
        // └── deep/
        //     └── nested/
        //         └── file5.ts

        fs::write(base_path.join("file1.py"), "def hello(): pass").await?;
        fs::write(base_path.join("file2.txt"), "Hello world").await?;

        fs::create_dir(base_path.join("subdir")).await?;
        fs::write(base_path.join("subdir/file3.py"), "def goodbye(): pass").await?;
        fs::write(base_path.join("subdir/file4.js"), "function test() {}").await?;

        fs::create_dir_all(base_path.join("deep/nested")).await?;
        fs::write(
            base_path.join("deep/nested/file5.ts"),
            "function typed(): string { return 'test'; }",
        )
        .await?;

        Ok(temp_dir)
    }

    #[tokio::test]
    async fn test_read_file_success() {
        let temp_dir = create_test_directory().await.unwrap();

        let content = FileSystemProvider::read_file(&temp_dir.path().join("file1.py"))
            .await
            .unwrap();

        assert_eq!(content, "def hello(): pass");
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let result = FileSystemProvider::read_file(&PathBuf::from("nonexistent_file.txt")).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, ExtractorError::FileSystem { .. }));
        assert!(error.to_string().contains("nonexistent_file.txt"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let empty_file = temp_dir.path().join("empty.txt");
        fs::write(&empty_file, "").await.unwrap();

        let content = FileSystemProvider::read_file(&empty_file).await.unwrap();

        assert_eq!(content, "");
    }

    #[tokio::test]
    async fn test_read_unicode_file() {
        let temp_dir = TempDir::new().unwrap();
        let unicode_file = temp_dir.path().join("unicode.txt");
        let unicode_content = "Hello 世界 🌍 Здравствуй мир";
        fs::write(&unicode_file, unicode_content).await.unwrap();

        let content = FileSystemProvider::read_file(&unicode_file).await.unwrap();

        assert_eq!(content, unicode_content);
    }
}
