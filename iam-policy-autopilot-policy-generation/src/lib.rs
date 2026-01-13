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

// AWS utilities
pub mod aws;

/// Fetch resources from AWS accounts
pub mod context_fetcher;

// Re-export the extraction module for public use
pub mod extraction;
// Re-export the policy_generation module for public use
pub mod policy_generation;

// Export api for public use
pub mod api;

use std::fmt::Display;

pub use enrichment::Engine as EnrichmentEngine;
pub use extraction::{Engine as ExtractionEngine, ExtractedMethods, SdkMethodCall, SourceFile};
pub use policy_generation::{
    Effect, Engine as PolicyGenerationEngine, IamPolicy, MethodActionMapping, PolicyType,
    PolicyWithMetadata, Statement,
};

// Re-export commonly used types for convenience
pub(crate) use extraction::ServiceModelIndex;

pub use providers::FileSystemProvider;
pub use providers::JsonProvider;

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
            "python" | "py" => Ok(Language::Python),
            "go" => Ok(Language::Go),
            "javascript" | "js" => Ok(Language::JavaScript),
            "typescript" | "ts" => Ok(Language::TypeScript),
            _ => Err(ExtractorError::UnsupportedLanguage {
                language: s.to_string(),
            }),
        }
    }
}

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let language_str = match self {
            Language::Python => "python",
            Language::Go => "go",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
        };
        write!(f, "{}", language_str)
    }
}

impl From<Language> for String {
    fn from(value: Language) -> String {
        match value {
            Language::Python => "python",
            Language::Go => "go",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
        }
        .to_string()
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
}
