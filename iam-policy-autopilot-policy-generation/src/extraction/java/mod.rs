//! Java extraction module — entry point for AWS SDK for Java v2 method call extraction.
//!
//! This module provides a **separate entry point** for Java that does not use the existing
//! [`Extractor`] trait (used by Python/Go/JS/TS). Instead it uses the Java-specific
//! [`JavaLanguageExtractorSet`] and [`JavaMatcher`].
//!
//! # Architecture
//!
//! ```text
//! Vec<SourceFile>
//!      ↓
//! JavaLanguageExtractorSet   (SdkExtractor impls → ExtractionResult)
//!      ↓
//! JavaMatcher                (ExtractionResult → Vec<SdkMethodCall>)
//!      ↓
//! Vec<SdkMethodCall>
//! ```
//!
//! # Entry Point
//!
//! ```ignore
//! use iam_policy_autopilot_policy_generation::extraction::java::extract_java_sdk_calls;
//! use iam_policy_autopilot_policy_generation::extraction::sdk_model::ServiceDiscovery;
//! use iam_policy_autopilot_policy_generation::{Language, SourceFile};
//! use std::path::PathBuf;
//!
//! let service_index = ServiceDiscovery::load_service_index(Language::Java).await?;
//! let source = SourceFile::with_language(
//!     PathBuf::from("Example.java"),
//!     std::fs::read_to_string("Example.java")?,
//!     Language::Java,
//! );
//! let calls = extract_java_sdk_calls(vec![source], &service_index).await?;
//! ```
//!
//! [`Extractor`]: crate::extraction::extractor::Extractor

pub(crate) mod extractor;
pub(crate) mod extractors;
pub(crate) mod matcher;
pub(crate) mod matchers;
pub(crate) mod types;

#[cfg(test)]
pub(crate) mod test_macros;

use crate::errors::{ExtractorError, Result};
use crate::extraction::java::extractor::JavaLanguageExtractorSet;
use crate::extraction::java::matcher::JavaMatcher;
use crate::extraction::java::types::ExtractionResult;
use crate::extraction::{SdkMethodCall, ServiceModelIndex, SourceFile};

// ================================================================================================
// Error type
// ================================================================================================

/// Errors that can occur during Java SDK method call extraction.
#[derive(Debug, thiserror::Error)]
pub(crate) enum JavaExtractionError {
    /// The source file could not be parsed by ast-grep.
    #[error("Failed to parse Java source '{path}': {message}")]
    ParseError {
        /// The path of the file that failed to parse
        path: String,
        /// The underlying parse error message
        message: String,
    },

    /// A source file was passed that is not a Java file.
    #[error("Source file is not Java: {path}")]
    NotJavaFile {
        /// The path of the non-Java file
        path: String,
    },
}

impl From<JavaExtractionError> for ExtractorError {
    fn from(e: JavaExtractionError) -> Self {
        match e {
            JavaExtractionError::NotJavaFile { path } => Self::method_extraction(
                "java",
                std::path::PathBuf::from(&path),
                format!("Source file is not Java: {path}"),
            ),
            JavaExtractionError::ParseError { path, message } => {
                Self::method_extraction("java", std::path::PathBuf::from(&path), message)
            }
        }
    }
}

// ================================================================================================
// Entry point
// ================================================================================================

/// Extract AWS SDK method calls from one or more Java source files.
///
/// This is the primary entry point for Java extraction. It does **not** use the
/// existing [`Extractor`] trait (used by Python/Go/JS/TS). Instead it uses the
/// Java-specific [`JavaLanguageExtractorSet`] and [`JavaMatcher`].
///
/// # Arguments
/// * `source_files` - One or more Java [`SourceFile`] values to analyze. All files must
///   have [`Language::Java`] set.
/// * `service_index` - Pre-loaded SDK service index (build with
///   [`ServiceDiscovery::load_service_index`] using [`Language::Java`]).
///
/// # Returns
/// All matched SDK method calls across all input files, as a flat [`Vec<SdkMethodCall>`].
///
/// # Errors
/// - [`ExtractorError::Validation`] if `source_files` is empty.
/// - [`ExtractorError::MethodExtraction`] if any file is not Java or ast-grep fails to parse a file.
///
/// [`Extractor`]: crate::extraction::extractor::Extractor
/// [`Language::Java`]: crate::Language::Java
/// [`ServiceDiscovery::load_service_index`]: crate::extraction::sdk_model::ServiceDiscovery::load_service_index
pub(crate) async fn extract_java_sdk_calls(
    source_files: Vec<SourceFile>,
    service_index: &ServiceModelIndex,
) -> Result<Vec<SdkMethodCall>> {
    if source_files.is_empty() {
        return Err(ExtractorError::validation("No source files provided"));
    }

    // Phase 1 — parallel AST extraction (CPU-bound).
    //
    // Each file is processed on a blocking thread via `spawn_blocking`.  The
    // `JavaLanguageExtractorSet` is stateless (all extractors are pure functions
    // over the AST), so it is cheap to construct per task.  `SourceFile` is
    // `Send + 'static` (owned `String` content + `PathBuf`), so it can be moved
    // into the closure without cloning the service index.
    let mut join_set: tokio::task::JoinSet<Result<ExtractionResult>> = tokio::task::JoinSet::new();

    for source_file in source_files {
        join_set.spawn_blocking(move || {
            log::debug!(
                "Java extraction: processing file '{}'",
                source_file.path.display()
            );

            let extractor_set = JavaLanguageExtractorSet::default_aws_v2();
            let extraction_result = extractor_set.extract_from_file(&source_file)?;

            log::debug!(
                "Java extraction: found {} imports, {} calls, {} waiters, {} paginators",
                extraction_result.imports.len(),
                extraction_result.calls.len(),
                extraction_result.waiters.len(),
                extraction_result.paginators.len(),
            );

            Ok(extraction_result)
        });
    }

    // Collect and merge results into a single ExtractionResult, propagating the first error encountered.
    //
    // Merging is safe because ExtractionResult carries per-item Location values (including
    // file_path), so the matcher can still build its per-file import indexes correctly
    // even when all files are combined into one value.
    //
    // NOTE: `join_next()` returns tasks in *completion* order, not submission order, so
    // the merged result — and therefore the matcher output — would be non-deterministic
    // when multiple files are processed in parallel.  We sort the final call list below
    // to guarantee a stable, reproducible output regardless of scheduling.
    let mut extraction_result = ExtractionResult::default();
    while let Some(join_result) = join_set.join_next().await {
        // `JoinError` means the blocking task panicked — treat as a parse error.
        let file_result = join_result.map_err(|e| {
            ExtractorError::method_extraction(
                "java",
                std::path::PathBuf::from("unknown"),
                format!("Extraction task panicked: {e}"),
            )
        })??;
        extraction_result.extend(file_result);
    }

    // Phase 2 — sequential matching (cheap HashMap lookups, borrows service_index).
    //
    // The matcher builds per-file import indexes from the Location embedded in each
    // item, so passing the merged result produces the same filtering as processing files
    // individually.
    let matcher = JavaMatcher::new(
        service_index,
        &crate::extraction::java::extractor::JAVA_UTILITIES_MODEL,
    );
    let mut all_calls = matcher.match_calls(&extraction_result);

    // Sort by (name, location) to produce a deterministic output order regardless
    // of which blocking task finished first.
    all_calls.sort_by(|a, b| {
        fn key(c: &SdkMethodCall) -> impl Ord + '_ {
            (c.name.as_str(), c.metadata.as_ref().map(|m| &m.location))
        }
        key(a).cmp(&key(b))
    });

    log::debug!(
        "Java extraction: matched {} SDK method calls across all files",
        all_calls.len(),
    );

    Ok(all_calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::sdk_model::{SdkServiceDefinition, ServiceMethodRef, ServiceModelIndex};
    use crate::{Language, SdkMethodCall, SourceFile};
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── error cases ───────────────────────────────────────────────────────────
    //
    // These two cases exercise error paths that cannot be expressed as descriptor
    // files (empty input and non-Java file), so they remain as inline tests.

    #[tokio::test]
    async fn test_extract_java_sdk_calls_empty_input() {
        let index = ServiceModelIndex {
            services: HashMap::new(),
            method_lookup: HashMap::new(),
            waiter_lookup: HashMap::new(),
        };
        let result = extract_java_sdk_calls(vec![], &index).await;
        assert!(
            matches!(result, Err(ExtractorError::Validation { .. })),
            "empty input should return Validation error"
        );
    }

    #[tokio::test]
    async fn test_extract_java_sdk_calls_not_java_file() {
        let index = ServiceModelIndex {
            services: HashMap::new(),
            method_lookup: HashMap::new(),
            waiter_lookup: HashMap::new(),
        };
        let source = SourceFile::with_language(
            PathBuf::from("test.py"),
            "s3_client.put_object()".to_string(),
            Language::Python,
        );
        let result = extract_java_sdk_calls(vec![source], &index).await;
        assert!(
            matches!(result, Err(ExtractorError::MethodExtraction { .. })),
            "Python file should return MethodExtraction error"
        );
    }

    // ── file-driven pipeline tests ────────────────────────────────────────────
    //
    // Each descriptor JSON in tests/java/entry_point/ specifies one or more Java
    // source files, a service index, and the expected SDK calls (full SdkMethodCall
    // including Metadata).  The test calls `extract_java_sdk_calls` — the real async
    // entry point — so the full extraction + merge + matching pipeline is exercised
    // end-to-end.

    #[rstest::rstest]
    #[tokio::test]
    async fn test_entry_point(#[files("tests/java/entry_point/*.json")] descriptor_file: PathBuf) {
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Descriptor {
            source_files: Vec<String>,
            service_index_file: String,
            expected_sdk_calls: Vec<SdkMethodCall>,
        }

        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct ServiceIndexJson {
            #[serde(default)]
            services: HashMap<String, SdkServiceDefinition>,
            #[serde(default)]
            method_lookup: HashMap<String, Vec<ServiceMethodRef>>,
            #[serde(default)]
            waiter_lookup: HashMap<String, Vec<ServiceMethodRef>>,
        }

        let test_name = descriptor_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let descriptor_dir = descriptor_file
            .parent()
            .expect("descriptor file must have a parent directory");

        let descriptor_json = std::fs::read_to_string(&descriptor_file).unwrap_or_else(|e| {
            panic!("[{test_name}] Failed to read descriptor {descriptor_file:?}: {e}")
        });
        let descriptor: Descriptor = serde_json::from_str(&descriptor_json).unwrap_or_else(|e| {
            panic!("[{test_name}] Failed to parse descriptor {descriptor_file:?}: {e}")
        });

        let index_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(&descriptor.service_index_file);
        let index_json = std::fs::read_to_string(&index_path).unwrap_or_else(|e| {
            panic!("[{test_name}] Failed to read service index {index_path:?}: {e}")
        });
        let index_data: ServiceIndexJson = serde_json::from_str(&index_json).unwrap_or_else(|e| {
            panic!("[{test_name}] Failed to parse service index {index_path:?}: {e}")
        });

        let service_index = ServiceModelIndex {
            services: index_data.services,
            method_lookup: index_data.method_lookup,
            waiter_lookup: index_data.waiter_lookup,
        };

        let source_files: Vec<SourceFile> = descriptor
            .source_files
            .iter()
            .map(|name| {
                let java_path = descriptor_dir.join(name);
                let source_code = std::fs::read_to_string(&java_path).unwrap_or_else(|e| {
                    panic!("[{test_name}] Failed to read Java file {java_path:?}: {e}")
                });
                SourceFile::with_language(
                    java_path
                        .file_name()
                        .expect("java file must have a name")
                        .into(),
                    source_code,
                    Language::Java,
                )
            })
            .collect();

        let actual_calls = extract_java_sdk_calls(source_files, &service_index)
            .await
            .unwrap_or_else(|e| panic!("[{test_name}] extract_java_sdk_calls failed: {e}"));

        assert_eq!(
            actual_calls, descriptor.expected_sdk_calls,
            "[{test_name}] SdkMethodCall mismatch",
        );
    }
}
