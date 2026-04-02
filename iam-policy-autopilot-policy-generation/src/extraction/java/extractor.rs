//! `SdkExtractor` trait and `JavaLanguageExtractorSet` for Java source extraction.
//!
//! Each [`SdkExtractor`] implementation handles one syntactic pattern (imports, method calls,
//! waiters, paginators). The [`JavaLanguageExtractorSet`] combines all patterns into
//! a **single ast-grep rule** so the AST is scanned only once, then routes each match to the
//! correct extractor via [`SdkExtractor::process`].
//!
//! # Single-pass design
//!
//! ```text
//! JavaLanguageExtractorSet::extract_from_file
//!   │
//!   ├─ Assembles combined rule:
//!   │    id: java_combined
//!   │    language: Java
//!   │    rule:
//!   │      any:
//!   │        - <extractor_1 rule body>
//!   │        - <extractor_2 rule body>
//!   │        - ...
//!   │
//!   ├─ root.find_all(combined_matcher)   ← single AST scan
//!   │
//!   └─ For each NodeMatch:
//!        for extractor in extractors:
//!          if env.get_match(extractor.discriminator_label()).is_some():
//!            extractor.process(node_match, source_file, result)
//!            break
//! ```

use std::sync::LazyLock;

use ast_grep_config::from_yaml_string;
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Java;

use crate::embedded_data::JavaSdkData;
use crate::extraction::java::matchers::utility::JavaUtilitiesModel;
use crate::extraction::java::types::ExtractionResult;
use crate::extraction::java::JavaExtractionError;
use crate::extraction::AstWithSourceFile;
use crate::SourceFile;

// ================================================================================================
// Shared utilities model — loaded once for the entire process lifetime
// ================================================================================================

/// The `java-sdk-v2-utilities.json` model, loaded exactly once.
///
/// Both the import-classification table (used during AST extraction) and the
/// [`JavaMatcher`] (used during matching) share this single instance,
/// so the embedded JSON is parsed only once regardless of how many files are processed.
///
/// # Panics
///
/// Panics on first access if the embedded JSON is missing or malformed.  Both
/// conditions indicate a corrupt binary and are unrecoverable.
///
/// [`JavaMatcher`]: super::matcher::JavaMatcher
pub(crate) static JAVA_UTILITIES_MODEL: LazyLock<JavaUtilitiesModel> = LazyLock::new(|| {
    let data = JavaSdkData::get_utilities_model()
        .expect("java-sdk-v2-utilities.json must be present in embedded data");
    serde_json::from_slice(&data).expect("java-sdk-v2-utilities.json must be valid JSON")
});

// ================================================================================================
// NodeMatch type alias
// ================================================================================================

/// Type alias for a matched node in a Java AST.
pub(crate) type JavaNodeMatch<'a> =
    ast_grep_core::NodeMatch<'a, ast_grep_core::tree_sitter::StrDoc<Java>>;

// ================================================================================================
// SdkExtractor trait
// ================================================================================================

/// Trait implemented by each Java-specific extractor.
///
/// Each implementation handles one syntactic pattern (imports, method calls, waiters, etc.).
///
/// # Single-pass scanning
///
/// The [`JavaLanguageExtractorSet`] combines all extractor rules into a single ast-grep rule
/// and scans the AST once. For each match, it calls [`SdkExtractor::process`] on the extractor
/// whose [`SdkExtractor::discriminator_label`] is present in the match environment.
///
/// # Rule format
///
/// [`SdkExtractor::rule_yaml`] must return the **rule body** — the content that goes under
/// `rule:` in the YAML, indented with two spaces. The `id:` and `language:` keys are added
/// by [`JavaLanguageExtractorSet`].
///
/// Each rule body **must** capture the [`SdkExtractor::discriminator_label`] metavariable
/// so the set can route matches to the correct extractor.
///
/// Example for an import extractor with discriminator `IMPORT_MARKER`:
/// ```yaml
///   kind: import_declaration
///   has:
///     pattern: $IMPORT_MARKER
/// ```
///
/// # Discriminator label
///
/// Each extractor must capture a **unique label** that no other extractor uses. This label
/// is used by [`JavaLanguageExtractorSet`] to route each match to the correct extractor.
///
/// The output of `SdkExtractor` is the Java-specific intermediate representation
/// ([`ExtractionResult`]), **not** [`SdkMethodCall`]. The conversion to
/// [`SdkMethodCall`] happens in the [`JavaMatcher`] (the matcher stage).
///
/// [`SdkMethodCall`]: crate::SdkMethodCall
/// [`JavaMatcher`]: super::matcher::JavaMatcher
pub(crate) trait SdkExtractor: Send + Sync {
    /// The rule body YAML — the content under `rule:`, indented with two spaces.
    ///
    /// Must capture [`SdkExtractor::discriminator_label`] so the set can route matches.
    fn rule_yaml(&self) -> &'static str;

    /// The unique metavariable label that this extractor captures and no other extractor uses.
    ///
    /// Used by [`JavaLanguageExtractorSet`] to identify which extractor matched a node.
    /// Must match the label used in [`SdkExtractor::rule_yaml`] (without the `$` prefix).
    fn discriminator_label(&self) -> &'static str;

    /// Process a single AST match and append findings to `result`.
    ///
    /// # Arguments
    /// * `node_match` - The matched AST node with its captured environment
    /// * `source_file` - Metadata for the source file being processed
    /// * `result` - Mutable accumulator for extraction findings
    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    );
}

// ================================================================================================
// JavaLanguageExtractorSet
// ================================================================================================

/// Runs all registered [`SdkExtractor`]s on a single Java source file using a **single AST scan**.
///
/// Use [`JavaLanguageExtractorSet::default_aws_v2`] to get the standard set of extractors
/// for AWS SDK for Java v2.
pub(crate) struct JavaLanguageExtractorSet {
    extractors: Vec<Box<dyn SdkExtractor>>,
}

impl JavaLanguageExtractorSet {
    /// Build the default extractor set for AWS SDK for Java v2.
    ///
    /// Includes extractors for:
    /// - Import declarations
    /// - Direct method calls (`receiver.method(args)`)
    /// - Waiter usages (init and call)
    /// - Paginator usages
    pub(crate) fn default_aws_v2() -> Self {
        use crate::extraction::java::extractors::import_extractor::JavaImportExtractor;
        use crate::extraction::java::extractors::method_extractor::JavaMethodCallExtractor;
        use crate::extraction::java::extractors::paginator_extractor::JavaPaginatorExtractor;
        use crate::extraction::java::extractors::waiter_extractor::JavaWaiterCallExtractor;

        Self {
            extractors: vec![
                // More specific patterns first to avoid shadowing by the broad method-call rule
                // Note: JavaImportExtractor also handles utility imports internally via
                // classify_utility_import, so no separate UtilityImportExtractor is needed.
                Box::new(JavaImportExtractor),
                Box::new(JavaPaginatorExtractor),
                Box::new(JavaWaiterCallExtractor),
                // Broad method-call extractor last
                Box::new(JavaMethodCallExtractor),
            ],
        }
    }

    /// Run all extractors on a single source file using a single AST scan.
    ///
    /// Combines all extractor rules into one `any:` rule, scans the AST once, then routes
    /// each match to the correct extractor via the discriminator label.
    ///
    /// # Errors
    /// Returns [`JavaExtractionError::NotJavaFile`] if the source file is not a Java file.
    /// Returns [`JavaExtractionError::ParseError`] if ast-grep fails to compile the combined rule.
    /// Both variants are converted to [`ExtractorError`] via the [`From`] impl in the parent module.
    pub(crate) fn extract_from_file(
        &self,
        source_file: &SourceFile,
    ) -> Result<ExtractionResult, JavaExtractionError> {
        use crate::Language;

        if source_file.language != Language::Java {
            return Err(JavaExtractionError::NotJavaFile {
                path: source_file.path.display().to_string(),
            });
        }

        // Build the combined rule YAML
        let combined_yaml = self.build_combined_rule();
        log::trace!("JavaLanguageExtractorSet combined rule:\n{combined_yaml}");

        let globals = ast_grep_config::GlobalRules::default();
        let configs = from_yaml_string::<Java>(&combined_yaml, &globals).map_err(|e| {
            JavaExtractionError::ParseError {
                path: source_file.path.display().to_string(),
                message: format!("Failed to compile combined rule: {e}"),
            }
        })?;

        let config = &configs[0];

        // Parse the source file into an AST
        let ast_grep = Java.ast_grep(&source_file.content);
        let ast = AstWithSourceFile::new(ast_grep, source_file.clone());

        let mut result = ExtractionResult::default();

        // Single AST scan — route each match to the correct extractor
        for node_match in ast.ast.root().find_all(&config.matcher) {
            let env = node_match.get_env();

            for extractor in &self.extractors {
                if env.get_match(extractor.discriminator_label()).is_some() {
                    extractor.process(&node_match, source_file, &mut result);
                    break;
                }
            }
        }

        Ok(result)
    }

    /// Build a combined `any:` rule YAML from all registered extractors.
    ///
    /// Each extractor provides a rule body (content under `rule:`). These are assembled as
    /// items under `rule:\n  any:`.
    fn build_combined_rule(&self) -> String {
        let mut yaml = String::from("id: java_combined\nlanguage: Java\nrule:\n  any:\n");

        for extractor in &self.extractors {
            // Each line of the rule body gets indented as an any: list item.
            // The first line gets "    - " prefix, subsequent lines get "      " prefix.
            let body = extractor.rule_yaml();
            let mut first = true;
            for line in body.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if first {
                    yaml.push_str("    - ");
                    yaml.push_str(line);
                    yaml.push('\n');
                    first = false;
                } else {
                    yaml.push_str("      ");
                    yaml.push_str(line);
                    yaml.push('\n');
                }
            }
        }

        yaml
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Language, SourceFile};
    use std::path::PathBuf;

    #[test]
    fn test_combined_rule_builds_without_error() {
        let set = JavaLanguageExtractorSet::default_aws_v2();
        let yaml = set.build_combined_rule();
        assert!(yaml.contains("any:"), "combined rule should contain any:");
        assert!(
            yaml.contains("java_combined"),
            "combined rule should have id"
        );
    }

    #[test]
    fn test_extract_from_non_java_file_errors() {
        let set = JavaLanguageExtractorSet::default_aws_v2();
        let source = SourceFile::with_language(
            PathBuf::from("test.py"),
            "s3.put_object()".to_string(),
            Language::Python,
        );
        assert!(matches!(
            set.extract_from_file(&source),
            Err(JavaExtractionError::NotJavaFile { .. })
        ));
    }

    #[rstest::rstest]
    fn test_extract_imports_and_calls_single_pass(
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
        let set = JavaLanguageExtractorSet::default_aws_v2();
        let result = set.extract_from_file(&source).expect("should succeed");
        assert!(!result.imports.is_empty(), "should find imports");
        assert!(!result.calls.is_empty(), "should find method calls");
    }
}
