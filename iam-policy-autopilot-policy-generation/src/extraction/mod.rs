//! Extraction module - Complete SDK method extraction functionality.
//!
//! This module provides comprehensive functionality for extracting AWS SDK method calls
//! from source code, including service discovery, SDK model parsing, extraction orchestration,
//! and all core data structures for representing source files, parsed methods, and extraction results.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) mod engine;
pub(crate) mod extractor;
pub(crate) mod go;
pub(crate) mod javascript;
pub(crate) mod python;
pub(crate) mod sdk_model;
pub(crate) mod service_hints;
pub(crate) mod shared;
pub(crate) mod typescript;
pub(crate) mod waiter_model;

// Re-export main types for convenience
pub use engine::Engine;
pub(crate) use sdk_model::ServiceModelIndex;
pub(crate) use service_hints::ServiceHintsProcessor;

// Re-export all core and output types for convenience
pub use self::{core::*, output::*};

/// Core data structures for source file parsing and method extraction
pub mod core {
    use std::sync::Arc;

    use schemars::JsonSchema;

    use crate::{Language, Location};

    use super::{Deserialize, Path, PathBuf, Serialize};

    #[derive(Clone)]
    pub(crate) struct AstWithSourceFile<T: ast_grep_language::LanguageExt> {
        pub(crate) ast: Arc<ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<T>>>,
        pub(crate) source_file: Arc<SourceFile>,
    }

    impl<T: ast_grep_language::LanguageExt> AstWithSourceFile<T> {
        pub(crate) fn new(
            ast: ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<T>>,
            source_file: SourceFile,
        ) -> Self {
            Self {
                ast: Arc::new(ast),
                source_file: Arc::new(source_file),
            }
        }
    }

    /// Represents a source file being analyzed
    ///
    /// Contains the file path, content, and detected programming language.
    /// This is the primary input structure for the extraction process.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "PascalCase")]
    #[non_exhaustive]
    pub struct SourceFile {
        /// File system path to the source file
        pub(crate) path: PathBuf,
        /// Complete content of the source file
        #[serde(skip)]
        pub content: String,
        /// Programming language identifier (e.g., "python", "typescript", "go")
        pub language: Language,
        /// File size in bytes
        #[serde(skip)]
        pub(crate) size: usize,
    }

    impl SourceFile {
        /// Create a `SourceFile` with explicit language specification.
        ///
        /// # Arguments
        /// * `path` - The file path
        /// * `content` - The file content
        /// * `language` - The programming language
        #[must_use]
        pub fn with_language(path: PathBuf, content: String, language: Language) -> Self {
            let size = content.len();

            Self {
                path,
                content,
                language,
                size,
            }
        }

        /// Detect programming language from file extension.
        pub(crate) fn detect_language(path: &Path) -> Option<Language> {
            let ext = path.extension()?.to_str()?.to_lowercase();
            Language::try_from_str(&ext).ok()
        }
    }

    /// Metadata for a parsed method call
    ///
    /// Contains detailed information about a method call including parameters,
    /// position information, and parsing context. This is optional metadata
    /// that can be omitted when only basic method identification is needed.
    #[derive(Debug, Clone, Hash, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
    #[serde(rename_all = "PascalCase")]
    pub struct SdkMethodCallMetadata {
        /// List of method parameters with their metadata
        pub(crate) parameters: Vec<Parameter>,
        /// Return type annotation if available
        pub(crate) return_type: Option<String>,

        /// The matched expression
        pub(crate) expr: String,

        // Position information
        pub(crate) location: Location,

        // SDK method call context
        /// Receiver variable name (e.g., "`s3_client`", "ec2")
        pub(crate) receiver: Option<String>,
    }

    impl SdkMethodCallMetadata {
        /// Returns whether this method call uses dictionary unpacking
        /// If true, parameter validation should be skipped
        pub(crate) fn has_dictionary_unpacking(&self) -> bool {
            self.parameters
                .iter()
                .any(|p| matches!(p, Parameter::DictionarySplat { .. }))
        }
    }

    /// Represents a parsed method from source code
    ///
    /// Contains the essential method identification information with optional
    /// detailed metadata. This is the core output of the parsing process.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "PascalCase")]
    #[non_exhaustive]
    pub struct SdkMethodCall {
        /// Method name as it appears in source code
        pub name: String,
        /// Matched AWS service names (e.g., ["s3"], ["ec2", "lambda"])
        /// Methods can exist in multiple services, hence Vec<String>
        pub possible_services: Vec<String>,
        /// Optional detailed metadata about the method call
        #[serde(default)]
        pub metadata: Option<SdkMethodCallMetadata>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct SimpleView<'a> {
        name: &'a str,
        possible_services: &'a Vec<String>,
    }

    impl SdkMethodCall {
        /// Serialize a list
        pub fn serialize_list(
            calls: &[SdkMethodCall],
            include_metadata: bool,
            pretty: bool,
        ) -> serde_json::Result<String> {
            if include_metadata {
                if pretty {
                    serde_json::to_string_pretty(calls)
                } else {
                    serde_json::to_string(calls)
                }
            } else {
                let simple: Vec<SimpleView> = calls
                    .iter()
                    .map(|call| SimpleView {
                        name: &call.name,
                        possible_services: &call.possible_services,
                    })
                    .collect();
                if pretty {
                    serde_json::to_string_pretty(&simple)
                } else {
                    serde_json::to_string(&simple)
                }
            }
        }
    }

    /// Parameter value that distinguishes between resolved literals and unresolved expressions
    #[derive(Debug, Clone, Hash, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
    pub(crate) enum ParameterValue {
        /// Resolved string literal with quotes stripped (e.g., "my-bucket", "42", "true")
        Resolved(String),
        /// Unresolved identifier/expression (e.g., bucket_name, get_bucket(), config["key"])
        Unresolved(String),
    }

    impl ParameterValue {
        /// Get the raw string value regardless of resolved/unresolved status
        pub fn as_string(&self) -> &str {
            match self {
                ParameterValue::Resolved(s) | ParameterValue::Unresolved(s) => s,
            }
        }
    }

    /// Represents a method parameter with type-safe variants
    ///
    /// This enum eliminates the conceptual confusion of the previous struct design
    /// where unpacked parameters had a "name" field containing expressions.
    /// Each variant contains only the fields that make sense for that parameter type.
    ///
    /// TODO: Refactor enum variant fields into separate structs to enable Default trait
    /// implementation and improve ergonomics. See: https://github.com/awslabs/iam-policy-autopilot/issues/61
    #[derive(Debug, Clone, Hash, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
    pub(crate) enum Parameter {
        /// Positional argument (e.g., first, second argument in call)
        Positional {
            /// Actual value in method call with resolution status
            value: ParameterValue,
            /// Positional index in method call (0-based)
            position: usize,
            /// Type annotation if available (e.g., "str", "int")
            type_annotation: Option<String>,
            /// For Go struct literals: extracted top-level field names (not nested)
            #[serde(skip_serializing_if = "Option::is_none")]
            struct_fields: Option<Vec<String>>,
        },
        /// Named keyword argument (e.g., Bucket='my-bucket')
        Keyword {
            /// Parameter name
            name: String,
            /// Actual value in method call with resolution status
            value: ParameterValue,
            /// Positional index in method call (0-based)
            position: usize,
            /// Type annotation if available (e.g., "str", "int", "Optional[str]")
            type_annotation: Option<String>,
        },
        /// Unpacked dictionary argument (e.g., **kwargs, **params)
        DictionarySplat {
            /// The unpacking expression (e.g., "**params", "**kwargs")
            expression: String,
            /// Positional index in method call (0-based)
            position: usize,
        },
    }
}

/// Output data structures for extraction results and metadata
pub mod output {
    use super::{Deserialize, SdkMethodCall, Serialize, SourceFile};

    /// Complete extraction results
    ///
    /// Contains all extracted methods for a service along with metadata
    /// about the extraction process. This is the primary output structure
    /// of the extraction system.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "PascalCase")]
    #[non_exhaustive]
    pub struct ExtractedMethods {
        /// List of all extracted methods
        pub methods: Vec<SdkMethodCall>,
        /// Metadata about the extraction process
        /// INVARIANT: all source_files must have the same language
        pub metadata: ExtractionMetadata,
    }

    /// Metadata about the extraction process
    ///
    /// Provides information about when the extraction was performed,
    /// what files were processed, and any warnings or issues encountered.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "PascalCase")]
    #[non_exhaustive]
    pub struct ExtractionMetadata {
        /// ISO 8601 timestamp of when extraction was performed
        pub extraction_time: String,
        /// List of source files that were processed
        pub source_files: Vec<SourceFile>,
        /// Total number of methods extracted
        pub total_methods: usize,
        /// List of warnings or non-fatal issues encountered
        pub warnings: Vec<String>,
    }

    impl ExtractionMetadata {
        /// Create new extraction metadata with current timestamp
        #[must_use]
        pub(crate) fn new(source_files: Vec<SourceFile>, warnings: Vec<String>) -> Self {
            let extraction_time = Self::current_timestamp();
            let total_methods = 0; // Will be updated when methods are added

            Self {
                extraction_time,
                source_files,
                total_methods,
                warnings,
            }
        }

        /// Update the total methods count
        pub(crate) fn update_method_count(&mut self, count: usize) {
            self.total_methods = count;
        }

        /// Get current timestamp as ISO 8601 string
        fn current_timestamp() -> String {
            use std::time::{SystemTime, UNIX_EPOCH};

            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| {
                    let secs = duration.as_secs();
                    // Simple timestamp format - will be replaced with proper ISO 8601 when chrono is available
                    format!("{secs}")
                })
                .unwrap_or_else(|_| "0".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Language, Location};

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_source_file_creation() {
        let source_file = SourceFile {
            path: PathBuf::from("test.py"),
            content: "def test(): pass".to_string(),
            language: Language::Python,
            size: "def test(): pass".len(),
        };

        assert_eq!(source_file.path, PathBuf::from("test.py"));
        assert_eq!(source_file.language, Language::Python);
    }

    #[test]
    fn test_location_construction() {
        let location = Location::new(PathBuf::new(), (10, 1), (10, 25));
        assert_eq!(location.start_position, (10, 1));
        assert_eq!(location.end_position, (10, 25));
    }

    #[test]
    fn test_extraction_metadata_creation() {
        let metadata = ExtractionMetadata::new(vec![], vec![]);
        assert!(!metadata.extraction_time.is_empty());
        assert_eq!(metadata.total_methods, 0);
        assert_eq!(metadata.warnings.len(), 0);
    }

    #[test]
    fn test_source_file_serialization() {
        let source_file = SourceFile {
            path: PathBuf::from("test.py"),
            content: "def test(): pass".to_string(),
            language: Language::Python,
            size: 16,
        };

        let json = serde_json::to_string(&source_file).unwrap();

        // Verify PascalCase field names
        assert!(json.contains("\"Path\""));
        assert!(json.contains("\"Language\""));
    }

    #[test]
    fn test_sdk_method_call_serialization() {
        let method = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        let json = serde_json::to_string(&method).unwrap();

        // Verify PascalCase field names
        assert!(json.contains("\"Name\""));
        assert!(json.contains("\"PossibleServices\""));
    }

    #[test]
    fn test_sdk_method_call_metadata_serialization() {
        let metadata = SdkMethodCallMetadata {
            parameters: vec![],
            return_type: Some("Dict[str, Any]".to_string()),
            expr: "s3_client.foo_bar".to_string(),
            location: Location::new(PathBuf::new(), (10, 5), (10, 30)),
            receiver: Some("s3_client".to_string()),
        };

        let json = serde_json::to_string(&metadata).unwrap();

        // Verify PascalCase field names
        assert!(json.contains("\"Parameters\""));
        assert!(json.contains("\"ReturnType\""));
        assert!(json.contains("\"Location\""));
        assert!(json.contains("\"Receiver\""));
    }
}

#[test]
fn test_extracted_methods_serialization() {
    let extracted = ExtractedMethods {
        methods: vec![],
        metadata: ExtractionMetadata::new(vec![], vec![]),
    };

    let json = serde_json::to_string(&extracted).unwrap();

    // Verify PascalCase field names
    assert!(json.contains("\"Methods\""));
    assert!(json.contains("\"Metadata\""));
}

#[test]
fn test_extraction_metadata_serialization() {
    let metadata = ExtractionMetadata::new(vec![], vec!["warning1".to_string()]);

    let json = serde_json::to_string(&metadata).unwrap();

    // Verify PascalCase field names
    assert!(json.contains("\"ExtractionTime\""));
    assert!(json.contains("\"SourceFiles\""));
    assert!(json.contains("\"TotalMethods\""));
    assert!(json.contains("\"Warnings\""));
}
