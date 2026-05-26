//! Test utilities and fixtures for LSP client testing.

/// Sample Python code fixtures for testing.
pub mod fixtures {
    /// Simple Python code with boto3 import and S3 client creation.
    pub const SIMPLE_BOTO3: &str = r"import boto3

s3_client = boto3.client('s3')
response = s3_client.list_buckets()
";

    /// Python code with type annotations.
    pub const WITH_TYPE_ANNOTATIONS: &str = r"import boto3
from mypy_boto3_s3 import S3Client

def get_s3_client() -> S3Client:
    return boto3.client('s3')

client: S3Client = get_s3_client()
";

    /// Python code with multiple AWS service clients.
    pub const MULTIPLE_SERVICES: &str = r"import boto3

s3 = boto3.client('s3')
dynamodb = boto3.client('dynamodb')
lambda_client = boto3.client('lambda')
";

    /// Python code with resource API usage.
    pub const RESOURCE_API: &str = r"import boto3

s3 = boto3.resource('s3')
bucket = s3.Bucket('my-bucket')
bucket.upload_file('local.txt', 'remote.txt')
";

    /// Empty Python file.
    pub const EMPTY: &str = "";

    /// Python code with syntax error (for error handling tests).
    pub const SYNTAX_ERROR: &str = r"import boto3

def broken_function(
    # Missing closing parenthesis
";
}

/// Find the (line, column) position of `needle` in `source`.
///
/// Returns the zero-based line and column of the first occurrence.
/// Panics if `needle` is not found.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn find_position(source: &str, needle: &str) -> (u32, u32) {
    for (line_idx, line) in source.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return (line_idx as u32, col as u32);
        }
    }
    panic!("'{needle}' not found in fixture");
}

/// Check if ty is available in PATH.
#[must_use]
pub fn is_ty_available() -> bool {
    which::which("ty").is_ok()
}

/// Check if boto3-stubs are installed.
#[must_use]
pub fn is_boto3_stubs_available() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import mypy_boto3_s3"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Check if both ty and boto3-stubs are available.
#[must_use]
pub fn is_lsp_ready() -> bool {
    is_ty_available() && is_boto3_stubs_available()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_position_first_line() {
        assert_eq!(find_position(fixtures::SIMPLE_BOTO3, "import"), (0, 0));
    }

    #[test]
    fn test_find_position_nested() {
        assert_eq!(find_position(fixtures::SIMPLE_BOTO3, "s3_client"), (2, 0));
    }

    #[test]
    fn test_find_position_mid_line() {
        assert_eq!(
            find_position(fixtures::SIMPLE_BOTO3, "list_buckets"),
            (3, 21)
        );
    }

    #[test]
    #[should_panic(expected = "not found in fixture")]
    fn test_find_position_missing() {
        find_position(fixtures::SIMPLE_BOTO3, "nonexistent_symbol");
    }

    #[test]
    fn test_fixtures_are_valid_strings() {
        assert!(!fixtures::SIMPLE_BOTO3.is_empty());
        assert!(!fixtures::WITH_TYPE_ANNOTATIONS.is_empty());
        assert!(!fixtures::MULTIPLE_SERVICES.is_empty());
        assert!(!fixtures::RESOURCE_API.is_empty());
        assert!(fixtures::EMPTY.is_empty());
        assert!(!fixtures::SYNTAX_ERROR.is_empty());
    }
}
