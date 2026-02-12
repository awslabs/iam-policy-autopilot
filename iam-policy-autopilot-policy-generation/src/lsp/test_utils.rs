//! Test utilities and fixtures for LSP client testing.
//!
//! This module provides helper functions and fixtures for testing the LSP client.

/// Sample Python code fixtures for testing.
pub mod fixtures {
    /// Simple Python code with boto3 import and S3 client creation.
    pub const SIMPLE_BOTO3: &str = r#"import boto3

s3_client = boto3.client('s3')
response = s3_client.list_buckets()
"#;

    /// Python code with type annotations.
    pub const WITH_TYPE_ANNOTATIONS: &str = r#"import boto3
from mypy_boto3_s3 import S3Client

def get_s3_client() -> S3Client:
    return boto3.client('s3')

client: S3Client = get_s3_client()
"#;

    /// Python code with multiple AWS service clients.
    pub const MULTIPLE_SERVICES: &str = r#"import boto3

s3 = boto3.client('s3')
dynamodb = boto3.client('dynamodb')
lambda_client = boto3.client('lambda')
"#;

    /// Python code with resource API usage.
    pub const RESOURCE_API: &str = r#"import boto3

s3 = boto3.resource('s3')
bucket = s3.Bucket('my-bucket')
bucket.upload_file('local.txt', 'remote.txt')
"#;

    /// Empty Python file.
    pub const EMPTY: &str = "";

    /// Python code with syntax error (for error handling tests).
    pub const SYNTAX_ERROR: &str = r#"import boto3

def broken_function(
    # Missing closing parenthesis
"#;
}

/// Helper function to check if ty is available in PATH.
///
/// This can be used to conditionally skip tests that require ty.
pub fn is_ty_available() -> bool {
    which::which("ty").is_ok()
}

/// Helper function to check if boto3-stubs are installed.
///
/// This checks if the mypy_boto3_s3 package is available, which indicates
/// that boto3-stubs are installed. Without stubs, ty won't provide AWS-specific
/// type information.
pub fn is_boto3_stubs_available() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import mypy_boto3_s3"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Helper function to check if both ty and boto3-stubs are available.
///
/// For the LSP client to be useful for AWS type information, both ty and
/// boto3-stubs must be installed.
pub fn is_lsp_ready() -> bool {
    is_ty_available() && is_boto3_stubs_available()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ty_available() {
        // This test just verifies the function doesn't panic
        let _ = is_ty_available();
    }

    #[test]
    fn test_is_boto3_stubs_available() {
        // This test verifies the function doesn't panic
        let result = is_boto3_stubs_available();
        eprintln!("boto3-stubs available: {}", result);
    }

    #[test]
    fn test_is_lsp_ready() {
        // This test verifies the function doesn't panic
        let result = is_lsp_ready();
        eprintln!("LSP ready (ty + boto3-stubs): {}", result);
    }

    #[test]
    fn test_fixtures_are_valid_strings() {
        // Verify all fixtures are valid UTF-8 strings
        assert!(!fixtures::SIMPLE_BOTO3.is_empty());
        assert!(!fixtures::WITH_TYPE_ANNOTATIONS.is_empty());
        assert!(!fixtures::MULTIPLE_SERVICES.is_empty());
        assert!(!fixtures::RESOURCE_API.is_empty());
        assert!(fixtures::EMPTY.is_empty());
        assert!(!fixtures::SYNTAX_ERROR.is_empty());
    }
}
