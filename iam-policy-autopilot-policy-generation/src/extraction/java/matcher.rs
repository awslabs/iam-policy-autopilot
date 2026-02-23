//! [`JavaMatcher`] — thin orchestrator that delegates to focused sub-matchers.
//!
//! This is the **matcher stage**: it takes the Java-specific intermediate representation
//! produced by the extractors and converts it to the language-agnostic [`SdkMethodCall`]
//! type consumed by the rest of the pipeline.
//!
//! The orchestrator builds a **per-file** import index so that each call is filtered using
//! only the imports present in the same source file.  In Java every file must declare its
//! own imports, so cross-file import merging would incorrectly widen the candidate set.
//!
//! The orchestrator then delegates to:
//! - [`matchers::service_call`] — direct method calls × botocore `method_lookup`
//! - [`matchers::waiter`]       — waiter calls × `waiter_lookup`
//! - [`matchers::paginator`]    — paginator calls × `method_lookup`
//! - [`matchers::utility`]      — utility calls × `java-sdk-v2-utilities.json`

use std::collections::HashMap;
use std::path::PathBuf;

use crate::extraction::java::matchers::paginator::match_paginators;
use crate::extraction::java::matchers::service_call::match_service_calls;
use crate::extraction::java::matchers::utility::{match_utilities, JavaUtilitiesModel};
use crate::extraction::java::matchers::waiter::match_waiters;
use crate::extraction::java::types::{ExtractionResult, Import, UtilityImport};
use crate::extraction::{SdkMethodCall, ServiceModelIndex};

/// Maps an [`ExtractionResult`] to a flat list of [`SdkMethodCall`]s.
///
/// The output of this stage is [`SdkMethodCall`] — the same type produced by all other
/// language extractors — so it can be fed directly into the enrichment and policy-generation
/// pipeline.
pub(crate) struct JavaMatcher<'a> {
    service_index: &'a ServiceModelIndex,
    utility_model: &'a JavaUtilitiesModel,
}

impl<'a> JavaMatcher<'a> {
    /// Create a new matcher backed by the given service index and utilities model.
    ///
    /// The caller is responsible for supplying the model (typically the process-wide
    /// [`JAVA_UTILITIES_MODEL`] static) so the embedded JSON is parsed only once.
    ///
    /// [`JAVA_UTILITIES_MODEL`]: crate::extraction::java::extractor::JAVA_UTILITIES_MODEL
    pub(crate) fn new(
        service_index: &'a ServiceModelIndex,
        utility_model: &'a JavaUtilitiesModel,
    ) -> Self {
        Self {
            service_index,
            utility_model,
        }
    }

    /// Convert an [`ExtractionResult`] into validated [`SdkMethodCall`]s.
    ///
    /// Builds a per-file import index first, then delegates to the four focused
    /// sub-matchers in order: service calls, waiters, paginators, utilities.
    pub(crate) fn match_calls(&self, result: &ExtractionResult) -> Vec<SdkMethodCall> {
        // Build per-file full-import index:
        //   file_path → list of all Import records in that file
        //
        // In Java every file must declare its own imports, so we must not share imports
        // across files when filtering candidates for a given call.
        //
        // The service-name set needed for Tier 2/3 import filtering is derived on-demand
        // inside each sub-matcher from this map, so we only need one index here.
        let mut imports_by_file: HashMap<PathBuf, Vec<&Import>> = HashMap::new();
        for imp in &result.imports {
            imports_by_file
                .entry(imp.location.file_path.clone())
                .or_default()
                .push(imp);
        }

        // Build per-file utility-import index:
        //   file_path → list of utility imports in that file
        let mut utility_imports_by_file: HashMap<PathBuf, Vec<&UtilityImport>> = HashMap::new();
        for ui in &result.utility_imports {
            utility_imports_by_file
                .entry(ui.location.file_path.clone())
                .or_default()
                .push(ui);
        }

        let mut output = Vec::new();

        output.extend(match_service_calls(
            result,
            self.service_index,
            &imports_by_file,
        ));
        output.extend(match_waiters(result, self.service_index, &imports_by_file));
        output.extend(match_paginators(
            result,
            self.service_index,
            &imports_by_file,
        ));
        output.extend(match_utilities(
            result,
            &self.utility_model,
            self.service_index,
            &utility_imports_by_file,
        ));

        output
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::java::matchers::apply_import_filter;
    use crate::java_matcher_test;
    use rstest::rstest;
    use std::collections::HashSet;

    // ── import-filter unit tests (pure, no file I/O) ──────────────────────────

    /// Parameterized test for `apply_import_filter`.
    #[rstest]
    #[case(vec!["s3", "s3control"], vec!["s3"], vec!["s3"], "should narrow to s3 when only s3 is imported")]
    #[case(vec!["s3", "dynamodb"], vec![], vec!["s3", "dynamodb"], "empty import set should pass all services through")]
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

    java_matcher_test!(
        "tests/java/matchers/orchestrator/*.json",
        test_orchestrator_matching
    );
}
