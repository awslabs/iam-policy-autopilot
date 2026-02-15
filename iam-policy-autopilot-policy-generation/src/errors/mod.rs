//! Error handling module

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for operations that can fail with `ExtractorError`
pub(crate) type Result<T> = std::result::Result<T, ExtractorError>;

/// Comprehensive error type for the SDK method extraction system.
///
/// This enum covers all possible error conditions that can occur during
/// SDK method extraction, parsing, and processing operations.
#[derive(Error, Debug)]
pub enum ExtractorError {
    /// File system operation errors with detailed context
    #[error("File system error during {operation} on path '{path}': {source}")]
    FileSystem {
        /// The operation that failed (e.g., "read", "write", "create directory")
        operation: String,
        /// The file path involved in the operation
        path: PathBuf,
        /// The underlying I/O error
        #[source]
        source: std::io::Error,
    },

    /// JSON parsing and serialization errors with context
    #[error("JSON parsing error in {context}: {source}")]
    JsonParsing {
        /// Context where the JSON error occurred (e.g., "SDK configuration", "method metadata")
        context: String,
        /// The underlying JSON error
        #[source]
        source: serde_json::Error,
    },

    /// Unsupported programming language errors
    #[error("Unsupported language for file '{path}' with extension '{extension}'. Supported languages: Python, TypeScript, JavaScript, Go, Java")]
    UnsupportedFileLanguage {
        /// Path to the file with unsupported language
        path: PathBuf,
        /// File extension that couldn't be mapped to a supported language
        extension: String,
    },

    /// Unsupported programming language errors
    #[error("Unsupported language. Supported languages: Python, TypeScript, JavaScript, Go, Java")]
    UnsupportedLanguage {
        /// Unsupported language
        language: String,
    },

    /// Configuration validation and setup errors
    #[error("Configuration error: {message}")]
    Configuration {
        /// Detailed error message about the configuration issue
        message: String,
        /// Optional underlying error that caused the configuration failure
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Input validation errors for user-provided data
    #[error("Validation error: {message}")]
    Validation {
        /// Detailed validation error message
        message: String,
        /// Optional field name that failed validation
        field: Option<String>,
    },

    /// SDK-specific processing errors
    #[error("SDK processing error for '{sdk_name}': {message}")]
    SdkProcessing {
        /// Name of the SDK being processed
        sdk_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Method extraction errors with context
    #[error("Method extraction failed for '{language}' in file '{path}': {message}")]
    MethodExtraction {
        /// Programming language being processed
        language: String,
        /// File path where extraction failed
        path: PathBuf,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// OperationAction map loading errors for operation action maps
    #[error("OperationFas map not found for service '{service_name}' at path '{path}'")]
    OperationFasMapNotFound {
        /// Service name that was requested
        service_name: String,
        /// File path that was attempted
        path: String,
    },

    /// OperationAction map parsing errors for malformed JSON files
    #[error("Failed to parse OperationFas map for service '{service_name}': {message}")]
    OperationFasMapParseError {
        /// Service name being parsed
        service_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// OperationAction map loading errors for operation action maps
    #[error("OperationAction map not found for service '{service_name}' at path '{path}'")]
    OperationActionMapNotFound {
        /// Service name that was requested
        service_name: String,
        /// File path that was attempted
        path: String,
    },

    /// OperationAction map parsing errors for malformed JSON files
    #[error("Failed to parse OperationAction map for service '{service_name}': {message}")]
    OperationActionMapParseError {
        /// Service name being parsed
        service_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Service Reference loading errors for Service Definition Files
    #[error("Service Reference not found for service '{service_name}' at path '{path}'")]
    ServiceReferenceNotFound {
        /// Service name that was requested
        service_name: String,
        /// File path that was attempted
        path: String,
    },

    /// Service Reference parsing errors for malformed JSON files
    #[error("Failed to parse Service Reference for service '{service_name}': {message}")]
    ServiceReferenceParseError {
        /// Service name being parsed
        service_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Resource matching errors for resource matcher functionality
    #[error("Resource matching error for service '{service_name}': {message}")]
    ResourceMatchError {
        /// Service name involved in resource matching
        service_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// General enrichment errors for method call enrichment
    #[error("Enrichment error for service '{service_name}': {message}")]
    EnrichmentError {
        /// Service being enriched
        service_name: String,
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Policy generation errors for IAM policy creation
    #[error("Policy generation error: {message}")]
    PolicyGeneration {
        /// Detailed error message
        message: String,
        /// Optional underlying error
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Invalid service hints with suggestions
    #[error("Provided service hints do not exist:\n{suggestions}")]
    InvalidServiceHints {
        /// Formatted list of invalid services with suggestions
        suggestions: String,
    },
}

impl ExtractorError {
    /// Create a file system error with operation context
    pub(crate) fn file_system(
        operation: impl Into<String>,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::FileSystem {
            operation: operation.into(),
            path: path.into(),
            source,
        }
    }

    /// Create an unsupported language error
    pub(crate) fn unsupported_language_override(language: impl Into<String>) -> Self {
        Self::UnsupportedLanguage {
            language: language.into(),
        }
    }

    /// Create a validation error
    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: None,
        }
    }

    /// Create an SDK processing error with source
    pub(crate) fn sdk_processing_with_source(
        sdk_name: impl Into<String>,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::SdkProcessing {
            sdk_name: sdk_name.into(),
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a method extraction error
    pub(crate) fn method_extraction(
        language: impl Into<String>,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self::MethodExtraction {
            language: language.into(),
            path: path.into(),
            message: message.into(),
            source: None,
        }
    }

    /// Create an Service Reference parse error with source
    pub(crate) fn service_reference_parse_error_with_source(
        service_name: impl Into<String>,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::ServiceReferenceParseError {
            service_name: service_name.into(),
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create an enrichment error
    pub(crate) fn enrichment_error(
        service_name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::EnrichmentError {
            service_name: service_name.into(),
            message: message.into(),
            source: None,
        }
    }

    /// Create a policy generation error
    pub(crate) fn policy_generation(message: impl Into<String>) -> Self {
        Self::PolicyGeneration {
            message: message.into(),
            source: None,
        }
    }

    /// Create an invalid service hints error
    pub(crate) fn invalid_service_hints(suggestions: impl Into<String>) -> Self {
        Self::InvalidServiceHints {
            suggestions: suggestions.into(),
        }
    }
}

/// Convert common standard library errors to `ExtractorError`
impl From<std::io::Error> for ExtractorError {
    fn from(error: std::io::Error) -> Self {
        Self::FileSystem {
            operation: "unknown operation".to_string(),
            path: PathBuf::from("unknown path"),
            source: error,
        }
    }
}

impl From<serde_json::Error> for ExtractorError {
    fn from(error: serde_json::Error) -> Self {
        Self::JsonParsing {
            context: "unknown context".to_string(),
            source: error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_system_error_creation() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        let error = ExtractorError::file_system("read", "/path/to/file", io_error);

        assert!(matches!(error, ExtractorError::FileSystem { .. }));
        assert!(error.to_string().contains("read"));
        assert!(error.to_string().contains("/path/to/file"));
    }
}
