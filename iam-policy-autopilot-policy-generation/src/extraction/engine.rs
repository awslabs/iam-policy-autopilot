//! Extraction Engine - Core business logic for extracting method definitions and SDK method calls.
//!
//! This module contains the [`Engine`] which orchestrates all providers to perform
//! method extraction workflows from source code. It serves as the main entry point for the
//! extraction process, coordinating file system operations, JSON parsing, and tree-sitter
//! source code analysis.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

use crate::errors::{ExtractorError, Result};
use crate::extraction::extractor::Extractor;
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::{self, ExtractedMethods, ExtractionMetadata, SourceFile};
use crate::Language;

/// Core business logic for extracting method definitions and SDK method calls from source code.
#[non_exhaustive]
pub struct Engine;

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Create a new SDK method extractor with the specified providers.
    pub fn new() -> Self {
        Self
    }

    /// Extract SDK method calls from loaded source files with validation against AWS SDK service definitions.
    ///
    /// This method analyzes loaded source files to extract AWS SDK method calls,
    /// validates them against actual AWS SDK operations, and filters out non-SDK method calls.
    ///
    /// # Arguments
    ///
    /// * `source_files` - References to loaded source files to analyze
    /// * `language` - Programming language identifier (e.g., "python", "typescript", "javascript")
    ///
    /// # Returns
    ///
    /// Complete extraction results including only validated AWS SDK method calls and metadata.
    pub async fn extract_sdk_method_calls(
        &self,
        language: Language,
        source_files: Vec<SourceFile>,
    ) -> Result<ExtractedMethods> {
        let start_time = Instant::now();

        // Validate that source files are provided
        if source_files.is_empty() {
            return Err(ExtractorError::validation(
                "No source files provided for SDK method call extraction".to_string(),
            ));
        }

        // Load SDK service index for validation
        log::debug!("Loading AWS SDK service definitions for validation (language: {language})...");
        let sdk_load_start = Instant::now();
        let service_index = ServiceDiscovery::load_service_index(language).await?;
        let sdk_load_duration = sdk_load_start.elapsed();

        log::debug!(
            "Loaded AWS SDK definitions in {:.2}ms: {} services, {} method mappings",
            sdk_load_duration.as_secs_f64() * 1000.0,
            service_index.services.len(),
            service_index.method_lookup.len()
        );

        #[allow(unreachable_patterns)]
        let extractor: Arc<dyn Extractor + Send + Sync> = match language {
            Language::Python => Arc::new(extraction::python::extractor::PythonExtractor::new()),
            Language::Go => Arc::new(extraction::go::extractor::GoExtractor::new()),
            Language::JavaScript => {
                Arc::new(extraction::javascript::extractor::JavaScriptExtractor::new())
            }
            Language::TypeScript => {
                Arc::new(extraction::typescript::extractor::TypeScriptExtractor::new())
            }
            _ => return Err(ExtractorError::unsupported_language_override(language)),
        };

        // Initialize metadata with loaded files
        let mut metadata = ExtractionMetadata::new(source_files.clone(), Vec::new());

        // Extract SDK method calls from all source files concurrently
        let mut all_extraction_results = Vec::new();
        let mut join_set = JoinSet::new();

        for source_file in source_files {
            let extractor = extractor.clone();
            join_set.spawn(async move { extractor.parse(&source_file).await });
        }

        // Collect results from concurrent tasks
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(extraction_result) => {
                    all_extraction_results.push(extraction_result);
                }
                Err(e) => {
                    // Task join error - this is more serious
                    return Err(ExtractorError::method_extraction(
                        "unsupported",
                        PathBuf::from("unknown"),
                        format!("Task execution failed: {e}"),
                    ));
                }
            }
        }

        extractor.filter_map(&mut all_extraction_results, &service_index);

        // Disambiguate and validate method calls against SDK definitions
        extractor.disambiguate(&mut all_extraction_results, &service_index);

        let method_calls = all_extraction_results
            .into_iter()
            .flat_map(|r| r.method_calls())
            .collect::<Vec<_>>();

        // Update metadata with final method count
        metadata.update_method_count(method_calls.len());

        let total_duration = start_time.elapsed();
        log::debug!(
            "SDK method call extraction completed in {:.2}ms: {} validated SDK methods found",
            total_duration.as_secs_f64() * 1000.0,
            method_calls.len()
        );

        // Create final results
        Ok(ExtractedMethods {
            methods: method_calls,
            metadata,
        })
    }

    /// Detect and validate language consistency across multiple source files.
    ///
    /// This method detects the programming language from file extensions and ensures
    /// all files have the same detected language.
    pub fn detect_and_validate_language(&self, source_files: &[&Path]) -> Result<Language> {
        if source_files.is_empty() {
            return Err(ExtractorError::validation(
                "No source files provided for language detection".to_string(),
            ));
        }

        let mut detected_languages = std::collections::HashSet::new();
        let mut file_languages = Vec::new();

        // Detect language for each file
        for file_path in source_files {
            if let Some(language) = SourceFile::detect_language(file_path) {
                detected_languages.insert(language);
                file_languages.push((file_path, language));
            } else {
                return Err(ExtractorError::validation(format!(
                    "Unable to detect language for file: {}",
                    file_path.display()
                )));
            }
        }

        // Check if all files have the same language
        if detected_languages.len() > 1 {
            let mut error_msg = "Mixed programming languages detected:\n".to_string();
            for (file_path, language) in file_languages {
                error_msg.push_str(&format!("  {} -> {}\n", file_path.display(), language));
            }
            error_msg.push_str("All source files must be in the same programming language, or use --language to override.");

            return Err(ExtractorError::validation(error_msg));
        }

        // Return the single detected language
        // We know there's exactly one element due to the validation above
        Ok(detected_languages
            .into_iter()
            .next()
            .expect("Should have detected exactly one language"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_language_detection() {
        let test_cases = vec![
            ("test.py", "python"),
            ("test.ts", "typescript"),
            ("test.js", "javascript"),
            ("test.go", "go"),
            ("test.unsupported", "unsupported"),
            ("no_extension", "unsupported"),
        ];

        for (filename, expected) in test_cases {
            let path = PathBuf::from(filename);
            let detected = SourceFile::detect_language(&path)
                .map(|lang| lang.to_string())
                .unwrap_or_else(|| "unsupported".to_string());
            assert_eq!(detected, expected, "Failed for {filename}");
        }
    }

    /// Test that the extractor can be created with real providers and process a simple file.
    #[tokio::test]
    async fn test_extractor_with_real_providers() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a temporary Python file
        let python_source = r#"
import boto3

def example_function():
    """An example function."""
    s3_client = boto3.client('s3')
    return s3_client.list_buckets()

def helper_function():
    """A helper function."""
    return "helper"
"#;

        // Create temporary file
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(python_source.as_bytes()).unwrap();
        let temp_path = temp_file.path().to_path_buf();

        // Create extractor with base path for real service definitions
        let extractor = Engine::new();

        // Create a SourceFile from the temporary file
        let source_file = SourceFile::with_language(
            temp_path.clone(),
            python_source.to_string(),
            Language::Python,
        );

        // Test that we can call the extraction method without errors
        let result = extractor
            .extract_sdk_method_calls(Language::Python, vec![source_file])
            .await;

        // The result should be Ok, even if no methods are found
        assert!(
            result.is_ok(),
            "Extraction should not fail with real providers"
        );

        let results = result.unwrap();
        // Metadata should be populated
        assert_eq!(results.metadata.source_files.len(), 1);
        assert_eq!(results.metadata.source_files[0].language, Language::Python);
    }

    /// Test that the extractor handles empty source files list appropriately.
    #[tokio::test]
    async fn test_empty_source_files_handling() {
        let extractor = Engine::new();

        // Test with empty source files list
        let result = extractor
            .extract_sdk_method_calls(Language::Python, vec![])
            .await;

        // Should return an error for empty source files list
        assert!(result.is_err());
    }
}
