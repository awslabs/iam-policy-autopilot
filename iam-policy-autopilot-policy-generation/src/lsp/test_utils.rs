//! Test utilities and fixtures for LSP client testing.

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

/// Language-specific readiness checks for LSP integration tests.
pub mod python {
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
    pub fn is_ready() -> bool {
        is_ty_available() && is_boto3_stubs_available()
    }

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
}

/// Language-specific readiness checks for Go/gopls integration tests.
pub mod go {
    /// Check if gopls is available in PATH.
    #[must_use]
    pub fn is_gopls_available() -> bool {
        which::which("gopls").is_ok()
    }

    /// Check if the Go toolchain is available.
    #[must_use]
    pub fn is_go_available() -> bool {
        which::which("go").is_ok()
    }

    /// Check if both Go and gopls are available.
    #[must_use]
    pub fn is_ready() -> bool {
        is_go_available() && is_gopls_available()
    }

    /// Sample Go code fixtures for testing.
    pub mod fixtures {
        /// Go module file for test workspace.
        pub const GO_MOD: &str = "module example.com/testmod\n\ngo 1.21\n";

        /// Simple Go file with functions calling each other.
        pub const SIMPLE_CALL_CHAIN: &str = r#"package main

import "fmt"

func main() {
	result := helper()
	fmt.Println(result)
}

func helper() string {
	return deepHelper()
}

func deepHelper() string {
	return "hello"
}

func unrelated() string {
	return "not called from main"
}
"#;

        /// Go file with a struct method that calls other functions.
        pub const STRUCT_METHODS: &str = r#"package main

import "fmt"

type Server struct {
	name string
}

func NewServer(name string) *Server {
	return &Server{name: name}
}

func (s *Server) HandleRequest() string {
	data := s.fetchData()
	return s.format(data)
}

func (s *Server) fetchData() string {
	return "raw data"
}

func (s *Server) format(data string) string {
	return fmt.Sprintf("[%s] %s", s.name, data)
}
"#;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_position_first_line() {
        assert_eq!(
            find_position(python::fixtures::SIMPLE_BOTO3, "import"),
            (0, 0)
        );
    }

    #[test]
    fn test_find_position_nested() {
        assert_eq!(
            find_position(python::fixtures::SIMPLE_BOTO3, "s3_client"),
            (2, 0)
        );
    }

    #[test]
    fn test_find_position_mid_line() {
        assert_eq!(
            find_position(python::fixtures::SIMPLE_BOTO3, "list_buckets"),
            (3, 21)
        );
    }

    #[test]
    #[should_panic(expected = "not found in fixture")]
    fn test_find_position_missing() {
        let _ = find_position(python::fixtures::SIMPLE_BOTO3, "nonexistent_symbol");
    }

    #[test]
    fn test_fixtures_are_valid_strings() {
        assert!(!python::fixtures::SIMPLE_BOTO3.is_empty());
        assert!(!python::fixtures::WITH_TYPE_ANNOTATIONS.is_empty());
        assert!(!python::fixtures::MULTIPLE_SERVICES.is_empty());
        assert!(!python::fixtures::RESOURCE_API.is_empty());
        assert!(python::fixtures::EMPTY.is_empty());
        assert!(!python::fixtures::SYNTAX_ERROR.is_empty());
    }
}
