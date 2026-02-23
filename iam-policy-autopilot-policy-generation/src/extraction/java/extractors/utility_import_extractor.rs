//! Utility-import classification for the Java extractor.
//!
//! Utility classes live under different package roots that are not derivable by a simple rule.
//! This module provides [`classify_utility_import`], which maps a fully-qualified import path
//! to a `(utility_name, class_name)` pair by consulting the embedded
//! `java-sdk-v2-utilities.json` model.
//!
//! # How it works
//!
//! The model is loaded once at startup via a [`std::sync::LazyLock`]. For each feature entry
//! in the model, the `Import` field is a package prefix (e.g.
//! `"software.amazon.awssdk.transfer.s3"`). An import path matches a feature entry when the
//! import path starts with that prefix. The `utility_name` returned is the service key from
//! the model (e.g. `"s3"`, `"dynamodb"`).
//!
//! # Integration
//!
//! [`classify_utility_import`] is called by [`JavaImportExtractor::process`] for every import
//! declaration matched by the `$IMPORT_MARKER` rule. If the import path matches a known utility
//! package, the result is pushed to [`ExtractionResult::utility_imports`] instead of
//! [`ExtractionResult::imports`].
//!
//! [`JavaImportExtractor::process`]: super::import_extractor::JavaImportExtractor

use std::sync::LazyLock;

use crate::extraction::java::extractor::JAVA_UTILITIES_MODEL;

// ── Model-driven import prefix table ─────────────────────────────────────────

/// A single entry in the import-prefix lookup table derived from the model.
struct ImportEntry {
    /// Package prefix, e.g. `"software.amazon.awssdk.transfer.s3"`
    prefix: String,
    /// Service name from the model, e.g. `"s3"`
    service_name: String,
}

/// Global import-prefix table, built once from the shared [`JAVA_UTILITIES_MODEL`].
///
/// Derives unique `(prefix, service_name)` pairs from the already-loaded model so
/// the embedded JSON is never parsed a second time.
static IMPORT_TABLE: LazyLock<Vec<ImportEntry>> = LazyLock::new(|| {
    // Collect unique (prefix, service_name) pairs — multiple features may share the same prefix
    let mut seen = std::collections::HashSet::new();
    let mut table = Vec::new();

    for (service_name, features) in &JAVA_UTILITIES_MODEL.services {
        for (_feature_name, feature) in features {
            let key = (feature.import.clone(), service_name.clone());
            if seen.insert(key) {
                table.push(ImportEntry {
                    prefix: feature.import.clone(),
                    service_name: service_name.clone(),
                });
            }
        }
    }

    table
});

// ── Public API ────────────────────────────────────────────────────────────────

/// Classify a utility import path into `(utility_name, class_name)`.
///
/// `utility_name` is the service key from `java-sdk-v2-utilities.json`
/// (e.g. `"s3"`, `"dynamodb"`).
///
/// Returns `None` if:
/// - the path does not match any known utility package prefix, or
/// - the import is a wildcard (`*`).
pub(crate) fn classify_utility_import(import_path: &str) -> Option<(String, String)> {
    // Extract the simple class name (last dotted segment)
    let class_name = import_path.split('.').last()?.to_string();

    // Wildcard imports (ending in '*') are not useful for class-name resolution
    if class_name == "*" {
        return None;
    }

    // Find the first matching prefix in the table
    let entry = IMPORT_TABLE
        .iter()
        .find(|e| import_path.starts_with(e.prefix.as_str()))?;

    Some((entry.service_name.clone(), class_name))
}

#[cfg(test)]
mod tests {
    use super::classify_utility_import;
    use crate::java_extractor_test;
    use rstest::rstest;

    // ── classify_utility_import unit tests ───────────────────────────────────
    // These tests rely on the embedded java-sdk-v2-utilities.json model.
    // The utility_name returned is the service key from the model (e.g. "s3", "dynamodb").

    #[rstest]
    #[case(
        "software.amazon.awssdk.transfer.s3.S3TransferManager",
        Some(("s3", "S3TransferManager"))
    )]
    #[case(
        "software.amazon.awssdk.services.s3.presigner.S3Presigner",
        Some(("s3", "S3Presigner"))
    )]
    #[case(
        "software.amazon.awssdk.enhanced.dynamodb.DynamoDbEnhancedClient",
        Some(("dynamodb", "DynamoDbEnhancedClient"))
    )]
    #[case(
        "software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable",
        Some(("dynamodb", "DynamoDbTable"))
    )]
    #[case(
        "software.amazon.awssdk.services.cloudfront.utils.CloudFrontUtilities",
        None
    )]
    #[case("software.amazon.awssdk.services.s3.S3Client", None)]
    #[case("java.util.List", None)]
    #[case("software.amazon.awssdk.transfer.s3.*", None)]
    fn test_classify_utility_import(
        #[case] import_path: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        let result = classify_utility_import(import_path);
        let expected = expected.map(|(u, c)| (u.to_string(), c.to_string()));
        assert_eq!(result, expected, "failed for '{import_path}'");
    }

    // ── file-driven extractor integration tests ───────────────────────────────
    // These tests verify that JavaImportExtractor (which calls classify_utility_import
    // internally) correctly populates ExtractionResult::utility_imports.

    java_extractor_test!(
        "tests/java/extractors/utility_imports/*.java",
        crate::extraction::java::types::UtilityImport,
        utility_imports
    );
}
