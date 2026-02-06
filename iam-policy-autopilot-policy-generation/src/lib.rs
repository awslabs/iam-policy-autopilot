//! IAM Policy Autopilot Core Library
//!
//! This library provides core functionality for AWS IAM permission analysis
//! and SDK method extraction

#![deny(missing_docs)]
#![deny(unsafe_code)]
#![warn(clippy::all)]
#![allow(clippy::module_name_repetitions)]

// Re-export the errors module for public use
pub(crate) mod errors;

// Re-export the enrichment module for public use
pub(crate) mod enrichment;

// Re-export the providers module for public use
pub(crate) mod providers;

// Service configuration
pub(crate) mod service_configuration;

// Embedded AWS service data
pub mod embedded_data;

// Re-export the extraction module for public use
pub mod extraction;
// Re-export the policy_generation module for public use
pub mod policy_generation;

// Export api for public use
pub mod api;

use std::fmt::Display;
use std::path::PathBuf;

pub use enrichment::{Engine as EnrichmentEngine, Explanation};
pub use extraction::{Engine as ExtractionEngine, ExtractedMethods, SdkMethodCall, SourceFile};
pub use policy_generation::{
    Effect, Engine as PolicyGenerationEngine, IamPolicy, PolicyType, PolicyWithMetadata, Statement,
};

// Re-export commonly used types for convenience
pub(crate) use extraction::ServiceModelIndex;

pub use providers::FileSystemProvider;
pub use providers::JsonProvider;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::errors::ExtractorError;

/// Language that is analyzed
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum Language {
    Python,
    Go,
    JavaScript,
    TypeScript,
}

impl Language {
    fn sdk_type(&self) -> SdkType {
        match self {
            Self::Python => SdkType::Boto3,
            _ => SdkType::Other,
        }
    }
}

/// SdkType used, for Boto3 we look up the method name in the SDF
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum SdkType {
    Boto3,
    Other,
}

impl Language {
    /// Attempts to parse a language from a string representation.
    ///
    /// # Arguments
    ///
    /// * `language` - A string slice representing the language name
    ///
    /// # Returns
    ///
    /// * `Ok(Language)` - If the string matches a supported language
    /// * `Err(ExtractorError::UnsupportedLanguageOverride)` - If the string doesn't match any supported language
    ///
    /// # Examples
    ///
    /// ```
    /// use iam_policy_autopilot_policy_generation::Language;
    ///
    /// assert_eq!(Language::try_from_str("python").unwrap(), Language::Python);
    /// assert_eq!(Language::try_from_str("go").unwrap(), Language::Go);
    /// assert!(Language::try_from_str("unsupported").is_err());
    /// ```
    pub fn try_from_str(s: &str) -> Result<Self, ExtractorError> {
        match s {
            "python" | "py" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            "javascript" | "js" => Ok(Self::JavaScript),
            "typescript" | "ts" => Ok(Self::TypeScript),
            _ => Err(ExtractorError::UnsupportedLanguage {
                language: s.to_string(),
            }),
        }
    }
}

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let language_str = match self {
            Self::Python => "python",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
        };
        write!(f, "{language_str}")
    }
}

impl From<Language> for String {
    fn from(value: Language) -> Self {
        match value {
            Language::Python => "python",
            Language::Go => "go",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
        }
        .to_string()
    }
}

/// Represents a location in a source file
///
/// This struct stores file path and position information and serializes
/// to the GNU coding standard (https://www.gnu.org/prep/standards/html_node/Errors.html)
/// format: `filename:startLine.startCol-endLine.endCol`
#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(
    description = "File location in GNU coding standard format: filename:startLine.startCol-endLine.endCol"
)]
pub struct Location {
    /// File path
    pub file_path: PathBuf,
    /// Starting position (line, column) - both 1-based
    pub start_position: (usize, usize),
    /// Ending position (line, column) - both 1-based
    pub end_position: (usize, usize),
}

impl Location {
    /// Create a new Location
    #[must_use]
    pub fn new(
        file_path: PathBuf,
        start_position: (usize, usize),
        end_position: (usize, usize),
    ) -> Self {
        Self {
            file_path,
            start_position,
            end_position,
        }
    }

    /// Create a new Location from an AST node
    #[must_use]
    pub fn from_node<T>(
        file_path: PathBuf,
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<T>>,
    ) -> Self
    where
        T: ast_grep_language::LanguageExt,
    {
        let start = node.start_pos();
        let end = node.end_pos();
        Self {
            file_path,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        }
    }

    /// Line where the finding starts
    #[must_use]
    pub fn start_line(&self) -> usize {
        self.start_position.0
    }

    /// Column where the finding starts
    #[must_use]
    pub fn start_col(&self) -> usize {
        self.start_position.1
    }

    /// Line where the finding ends
    #[must_use]
    pub fn end_line(&self) -> usize {
        self.end_position.0
    }

    /// Column where the finding ends
    #[must_use]
    pub fn end_col(&self) -> usize {
        self.end_position.1
    }

    /// Format as GNU coding standard: `filename:startLine.startCol-endLine.endCol`
    #[must_use]
    pub fn to_gnu_format(&self) -> String {
        let path_str = self.file_path.display();
        let (start_line, start_col) = self.start_position;
        let (end_line, end_col) = self.end_position;

        format!("{path_str}:{start_line}.{start_col}-{end_line}.{end_col}")
    }
}

impl Serialize for Location {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_gnu_format())
    }
}

impl<'de> Deserialize<'de> for Location {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_gnu_format(&s).map_err(serde::de::Error::custom)
    }
}

impl Location {
    /// Parse a Location from GNU coding standard format: `filename:startLine.startCol-endLine.endCol`
    pub fn from_gnu_format(s: &str) -> Result<Self, String> {
        // Find the colon that separates filename from position
        let colon_pos = s.rfind(':').ok_or("Missing colon separator")?;
        let (file_path_str, position_str) = s.split_at(colon_pos);
        let position_str = &position_str[1..]; // Remove the colon

        // Parse the position part: startLine.startCol-endLine.endCol
        let dash_pos = position_str.find('-').ok_or("Missing dash separator")?;
        let (start_str, end_str) = position_str.split_at(dash_pos);
        let end_str = &end_str[1..]; // Remove the dash

        // Parse start position: startLine.startCol
        let start_dot_pos = start_str.find('.').ok_or("Missing dot in start position")?;
        let (start_line_str, start_col_str) = start_str.split_at(start_dot_pos);
        let start_col_str = &start_col_str[1..]; // Remove the dot

        // Parse end position: endLine.endCol
        let end_dot_pos = end_str.find('.').ok_or("Missing dot in end position")?;
        let (end_line_str, end_col_str) = end_str.split_at(end_dot_pos);
        let end_col_str = &end_col_str[1..]; // Remove the dot

        // Convert strings to numbers
        let start_line = start_line_str
            .parse::<usize>()
            .map_err(|_| format!("Invalid start line: {start_line_str}"))?;
        let start_col = start_col_str
            .parse::<usize>()
            .map_err(|_| format!("Invalid start column: {start_col_str}"))?;
        let end_line = end_line_str
            .parse::<usize>()
            .map_err(|_| format!("Invalid end line: {end_line_str}"))?;
        let end_col = end_col_str
            .parse::<usize>()
            .map_err(|_| format!("Invalid end column: {end_col_str}"))?;

        Ok(Self {
            file_path: PathBuf::from(file_path_str),
            start_position: (start_line, start_col),
            end_position: (end_line, end_col),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_display() {
        assert_eq!(Language::Python.to_string(), "python");
        assert_eq!(Language::Go.to_string(), "go");
        assert_eq!(Language::JavaScript.to_string(), "javascript");
        assert_eq!(Language::TypeScript.to_string(), "typescript");
    }

    #[test]
    fn test_language_display_formatting() {
        assert_eq!(format!("{}", Language::Python), "python");
        assert_eq!(format!("{}", Language::Go), "go");
        assert_eq!(format!("{}", Language::JavaScript), "javascript");
        assert_eq!(format!("{}", Language::TypeScript), "typescript");
    }

    #[test]
    fn test_language_try_from_str() {
        // Test valid language strings
        assert_eq!(Language::try_from_str("python").unwrap(), Language::Python);
        assert_eq!(Language::try_from_str("go").unwrap(), Language::Go);
        assert_eq!(
            Language::try_from_str("javascript").unwrap(),
            Language::JavaScript
        );
        assert_eq!(
            Language::try_from_str("typescript").unwrap(),
            Language::TypeScript
        );

        // Test invalid language string returns error
        assert!(Language::try_from_str("unsupported").is_err());
        assert!(Language::try_from_str("java").is_err());
        assert!(Language::try_from_str("").is_err());
    }

    #[test]
    fn test_location_gnu_format() {
        let location = Location::new(PathBuf::from("src/main.rs"), (10, 5), (15, 20));

        let gnu_format = location.to_gnu_format();
        assert_eq!(gnu_format, "src/main.rs:10.5-15.20");
    }

    #[test]
    fn test_location_from_gnu_format() {
        let gnu_str = "src/main.rs:10.5-15.20";
        let location = Location::from_gnu_format(gnu_str).unwrap();

        assert_eq!(location.file_path, PathBuf::from("src/main.rs"));
        assert_eq!(location.start_position, (10, 5));
        assert_eq!(location.end_position, (15, 20));
    }

    #[test]
    fn test_location_from_gnu_format_with_complex_path() {
        let gnu_str = "/home/user/project/src/lib.rs:1.1-100.50";
        let location = Location::from_gnu_format(gnu_str).unwrap();

        assert_eq!(
            location.file_path,
            PathBuf::from("/home/user/project/src/lib.rs")
        );
        assert_eq!(location.start_position, (1, 1));
        assert_eq!(location.end_position, (100, 50));
    }

    #[test]
    fn test_location_serialize_deserialize_roundtrip() {
        let original = Location::new(PathBuf::from("test/file.py"), (42, 13), (45, 7));

        // Serialize to JSON
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"test/file.py:42.13-45.7\"");

        // Deserialize back
        let deserialized: Location = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_location_from_gnu_format_invalid_formats() {
        // Missing colon
        assert!(Location::from_gnu_format("src/main.rs10.5-15.20").is_err());

        // Missing dash
        assert!(Location::from_gnu_format("src/main.rs:10.515.20").is_err());

        // Missing dots
        assert!(Location::from_gnu_format("src/main.rs:105-1520").is_err());

        // Invalid numbers
        assert!(Location::from_gnu_format("src/main.rs:abc.5-15.20").is_err());
        assert!(Location::from_gnu_format("src/main.rs:10.xyz-15.20").is_err());

        // Empty string
        assert!(Location::from_gnu_format("").is_err());
    }
}
