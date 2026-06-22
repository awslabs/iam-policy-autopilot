//! Java extraction module — entry point for AWS SDK for Java v2 method call extraction.
//!
//! This module provides the [`JavaLanguageExtractor`] which implements the framework's
//! [`LanguageExtractor`] trait.
//!
//! # Architecture
//!
//! ```text
//! Vec<SourceFile>
//!      ↓
//! JavaLanguageExtractor::extract()   [provided by LanguageExtractor framework]
//!      ↓
//! JavaLanguageExtractor::match_calls()
//!      ↓
//! Vec<SdkMethodCall>
//! ```
//!
//! [`LanguageExtractor`]: crate::extraction::framework::LanguageExtractor

pub(crate) mod extractors;
pub(crate) mod matchers;
pub(crate) mod types;

#[cfg(test)]
pub(crate) mod test_macros;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use ast_grep_language::Java;

use crate::embedded_data::JavaSdkData;
use crate::extraction::framework::{
    LanguageExtractor, LanguageExtractorSet, SdkExtractor, UtilitiesModel, UtilityMethod,
    UtilityOperation,
};
use crate::extraction::java::matchers::paginator::match_paginators;
use crate::extraction::java::matchers::service_call::match_service_calls;
use crate::extraction::java::matchers::utility::match_utilities;
use crate::extraction::java::matchers::waiter::match_waiters;
use crate::extraction::java::types::{ExtractionResult, Import, UtilityImport};
use crate::extraction::{SdkMethodCall, ServiceModelIndex};

#[cfg(test)]
use crate::errors::ExtractorError;

// ================================================================================================
// Utilities model — loaded once for the entire process lifetime
// ================================================================================================

/// The `java-sdk-v2-utilities.json` model, loaded exactly once and normalised into the
/// shared [`UtilitiesModel`] type from the framework.
///
/// Both the import-classification table (used during AST extraction) and
/// [`JavaLanguageExtractor::match_calls`] (used during matching) share this single instance,
/// so the embedded JSON is parsed only once regardless of how many files are processed.
///
/// # Panics
///
/// Panics on first access if the embedded JSON is missing or malformed.  Both
/// conditions indicate a corrupt binary and are unrecoverable.
pub(crate) static JAVA_UTILITIES_MODEL: LazyLock<UtilitiesModel> = LazyLock::new(|| {
    // The java-sdk-v2-utilities.json has a different schema from the framework's
    // UtilitiesModel. We normalise it here at load time.
    //
    // Java JSON schema:
    // {
    //   "Services": {
    //     "<service>": {
    //       "<feature_name>": {
    //         "MethodName": "...",
    //         "ReceiverClass": "...",
    //         "Import": "...",
    //         "Operations": [{ "Service": "...", "Name": "..." }]
    //       }
    //     }
    //   }
    // }
    //
    // Framework UtilitiesModel schema:
    // {
    //   services: HashMap<service_name, HashMap<method_name, UtilityMethod>>
    // }
    //
    // Normalisation: use MethodName as the method key, map Operations to UtilityOperation.

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct JavaUtilityOperationRaw {
        service: String,
        name: String,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct JavaUtilityFeatureRaw {
        method_name: String,
        receiver_class: String,
        import: String,
        operations: Vec<JavaUtilityOperationRaw>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct JavaUtilitiesModelRaw {
        services: std::collections::HashMap<
            String,
            std::collections::HashMap<String, JavaUtilityFeatureRaw>,
        >,
    }

    let data = JavaSdkData::get_utilities_model()
        .expect("java-sdk-v2-utilities.json must be present in embedded data");
    let raw: JavaUtilitiesModelRaw =
        serde_json::from_slice(&data).expect("java-sdk-v2-utilities.json must be valid JSON");

    // Normalise: service_name → method_name → UtilityMethod
    let services = raw
        .services
        .into_iter()
        .map(|(service_name, features)| {
            let methods = features
                .into_values()
                .map(|feature| {
                    let method = UtilityMethod {
                        operations: feature
                            .operations
                            .into_iter()
                            .map(|op| UtilityOperation {
                                service: op.service,
                                name: op.name,
                            })
                            .collect(),
                        receiver_class: Some(feature.receiver_class),
                        import_prefix: Some(feature.import),
                    };
                    (feature.method_name, method)
                })
                .collect();
            (service_name, methods)
        })
        .collect();

    UtilitiesModel { services }
});

// ================================================================================================
// JavaLanguageExtractor — implements the framework LanguageExtractor trait
// ================================================================================================

/// The Java language extractor.
///
/// Implements [`LanguageExtractor`] for AWS SDK for Java v2. Stateless — all runtime
/// data (`ServiceModelIndex`, `UtilitiesModel`) is passed to [`match_calls`] by the engine.
///
/// [`LanguageExtractor`]: crate::extraction::framework::LanguageExtractor
/// [`match_calls`]: JavaLanguageExtractor::match_calls
pub(crate) struct JavaLanguageExtractor;

impl LanguageExtractor for JavaLanguageExtractor {
    type Language = Java;
    type ExtractionResult = ExtractionResult;

    fn extractor_set(&self) -> LanguageExtractorSet<Java, ExtractionResult> {
        use crate::extraction::java::extractors::import_extractor::JavaImportExtractor;
        use crate::extraction::java::extractors::method_extractor::JavaMethodCallExtractor;
        use crate::extraction::java::extractors::paginator_extractor::JavaPaginatorExtractor;
        use crate::extraction::java::extractors::waiter_extractor::JavaWaiterCallExtractor;

        LanguageExtractorSet::new(
            Java,
            vec![
                Box::new(JavaImportExtractor)
                    as Box<dyn SdkExtractor<Java, ExtractionResult = ExtractionResult>>,
                Box::new(JavaPaginatorExtractor),
                Box::new(JavaWaiterCallExtractor),
                Box::new(JavaMethodCallExtractor),
            ],
        )
        .expect("default_aws_v2 extractor labels must be unique")
    }

    fn utilities_model(&self) -> Option<&'static UtilitiesModel> {
        Some(&JAVA_UTILITIES_MODEL)
    }

    /// Phase 2 — convert the [`ExtractionResult`] IR into validated [`SdkMethodCall`]s.
    ///
    /// Builds a per-file import index (Java requires per-file import scoping), then
    /// delegates to the four focused sub-matchers in order:
    /// service calls → waiters → paginators → utilities.
    fn match_calls(
        &self,
        ir: &ExtractionResult,
        service_index: &ServiceModelIndex,
        utilities_model: Option<&UtilitiesModel>,
    ) -> Vec<SdkMethodCall> {
        // Build per-file full-import index:
        //   file_path → list of all Import records in that file
        //
        // In Java every file must declare its own imports, so we must not share imports
        // across files when filtering candidates for a given call.
        let mut imports_by_file: HashMap<PathBuf, Vec<&Import>> = HashMap::new();
        for imp in &ir.imports {
            imports_by_file
                .entry(imp.location.file_path.clone())
                .or_default()
                .push(imp);
        }

        // Build per-file utility-import index:
        //   file_path → list of utility imports in that file
        let mut utility_imports_by_file: HashMap<PathBuf, Vec<&UtilityImport>> = HashMap::new();
        for ui in &ir.utility_imports {
            utility_imports_by_file
                .entry(ui.location.file_path.clone())
                .or_default()
                .push(ui);
        }

        let mut output = Vec::new();

        output.extend(match_service_calls(ir, service_index, &imports_by_file));
        output.extend(match_waiters(ir, service_index, &imports_by_file));
        output.extend(match_paginators(ir, service_index, &imports_by_file));

        // Only run utility matching if a utilities model is available.
        if let Some(model) = utilities_model {
            output.extend(match_utilities(
                ir,
                model,
                service_index,
                &utility_imports_by_file,
            ));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::framework::{extract, LanguageExtractor};
    use crate::extraction::java::matchers::apply_import_filter;
    use crate::extraction::sdk_model::{SdkServiceDefinition, ServiceMethodRef, ServiceModelIndex};
    use crate::{Language, SdkMethodCall, SourceFile};
    use rstest::rstest;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    // ── import-filter unit tests (pure, no file I/O) ──────────────────────────

    /// Parameterized test for `apply_import_filter`.
    #[rstest]
    #[case(vec!["s3", "s3control"], vec!["s3"], vec!["s3"], "should narrow to s3 when only s3 is imported")]
    #[case(vec!["s3", "dynamodb"], vec![], vec![], "empty imports filter everything out")]
    #[case(vec!["s3"], vec!["dynamodb"], vec![], "no match returns empty — caller must not emit SdkMethodCall")]
    #[case(vec!["s3", "dynamodb", "sqs"], vec!["dynamodb", "sqs"], vec!["dynamodb", "sqs"], "should narrow to dynamodb and sqs")]
    fn test_apply_import_filter(
        #[case] services: Vec<&str>,
        #[case] imported: Vec<&str>,
        #[case] expected: Vec<&str>,
        #[case] msg: &str,
    ) {
        let services: Vec<String> = services.into_iter().map(str::to_string).collect();
        let imported: HashSet<String> = imported.into_iter().map(str::to_string).collect();
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();

        let filtered = apply_import_filter(services, &imported);
        assert_eq!(filtered, expected, "{msg}");
    }

    // ── file-driven orchestrator integration tests ────────────────────────────

    crate::java_matcher_test!(
        "tests/java/matchers/orchestrator/*.json",
        test_orchestrator_matching
    );

    // ── error cases ───────────────────────────────────────────────────────────
    //
    // These cases exercise error paths that cannot be expressed as descriptor
    // files, so they remain as inline tests.

    #[tokio::test]
    async fn test_extract_empty_input() {
        // Empty input is valid — the framework returns Ok with an empty IR.
        let extractor = JavaLanguageExtractor;
        let result = extract(&extractor, vec![]).await;
        assert!(result.is_ok(), "empty input should succeed with empty IR");
    }

    #[tokio::test]
    async fn test_extract_not_java_file() {
        // Passing a non-Java file to the Java extractor is a caller error.
        // The framework detects the language mismatch and returns MethodExtraction.
        let source = SourceFile::with_language(
            PathBuf::from("test.py"),
            "s3_client.put_object()".to_string(),
            Language::Python,
        );
        let extractor = JavaLanguageExtractor;
        let result = extract(&extractor, vec![source]).await;
        assert!(
            matches!(result, Err(ExtractorError::MethodExtraction { .. })),
            "Python file passed to Java extractor should return MethodExtraction error"
        );
    }

    // ── file-driven pipeline tests ────────────────────────────────────────────
    //
    // Each descriptor JSON in tests/java/entry_point/ specifies one or more Java
    // source files, a service index, and the expected SDK calls (full SdkMethodCall
    // including Metadata).  The test exercises the full extraction + merge + matching
    // pipeline end-to-end via JavaLanguageExtractor.

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

        let extractor = JavaLanguageExtractor;
        let ir = extract(&extractor, source_files)
            .await
            .unwrap_or_else(|e| panic!("[{test_name}] extract failed: {e}"));
        let utilities_model = extractor.utilities_model();
        let actual_calls = extractor.match_calls(&ir, &service_index, utilities_model);

        assert_eq!(
            actual_calls, descriptor.expected_sdk_calls,
            "[{test_name}] SdkMethodCall mismatch",
        );
    }

    // ── extractor infrastructure tests ───────────────────────────────────────

    #[test]
    fn test_combined_rule_builds_without_error() {
        let extractor = JavaLanguageExtractor;
        let yaml = extractor.extractor_set().build_combined_rule();
        assert!(yaml.contains("any:"), "combined rule should contain any:");
        assert!(
            yaml.contains("Java_combined"),
            "combined rule should have id"
        );
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn test_extract_imports_and_calls_single_pass(
        #[files("tests/java/extractors/extractor/imports_and_calls_single_pass.java")]
        java_file: PathBuf,
    ) {
        let source_code = std::fs::read_to_string(&java_file)
            .unwrap_or_else(|e| panic!("Failed to read {java_file:?}: {e}"));
        let source = SourceFile::with_language(
            java_file.file_name().expect("must have file name").into(),
            source_code,
            Language::Java,
        );
        let extractor = JavaLanguageExtractor;
        let result = extract(&extractor, vec![source])
            .await
            .expect("should succeed");
        assert!(!result.imports.is_empty(), "should find imports");
        assert!(!result.calls.is_empty(), "should find method calls");
    }
}
