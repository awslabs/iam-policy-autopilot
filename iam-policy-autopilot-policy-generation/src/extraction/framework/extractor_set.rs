//! [`LanguageExtractorSet<L, IR>`] — runs all registered [`SdkExtractor`]s on source files
//! using a **single AST scan per file**.

use std::collections::HashSet;
use std::sync::Arc;

use ast_grep_config::from_yaml_string;
use ast_grep_core::tree_sitter::LanguageExt;
use serde::Deserialize;

use crate::errors::{ExtractorError, Result};
use crate::extraction::SourceFile;

use super::sdk_extractor::SdkExtractor;

// ================================================================================================
// DuplicateDiscriminatorError
// ================================================================================================

/// Error returned when two extractors in a set share the same discriminator label.
#[derive(Debug, thiserror::Error)]
#[error("duplicate discriminator label '{label}' in extractor set")]
pub(crate) struct DuplicateDiscriminatorError {
    pub(crate) label: &'static str,
}

// ================================================================================================
// LanguageExtractorSet
// ================================================================================================

/// Runs all registered [`SdkExtractor`]s on source files using a **single AST scan per file**.
///
/// # Construction
/// Use [`LanguageExtractorSet::new`] to construct. Returns `Err(DuplicateDiscriminatorError)`
/// if any two extractors share a discriminator label — this is a programming error and
/// indicates a bug in the extractor set definition.
///
/// # Extraction
/// [`extract_from_files`] fans out across files using `spawn_blocking` (CPU-bound AST work),
/// merges the per-file `IR` values via `IR::extend_from`, and returns the combined result.
/// The `IR` type must implement `Default` (for the initial accumulator) and [`IrExtend`]
/// (for merging).
///
/// [`extract_from_files`]: LanguageExtractorSet::extract_from_files
pub(crate) struct LanguageExtractorSet<L: LanguageExt, IR> {
    /// The language instance used for AST parsing. All ast-grep language types are unit
    /// structs that implement `Copy`, so storing one here is zero-cost.
    language: L,
    extractors: Arc<Vec<Box<dyn SdkExtractor<L, ExtractionResult = IR>>>>,
}

impl<L, IR> LanguageExtractorSet<L, IR>
where
    L: LanguageExt + Copy + Send + Sync + 'static + for<'de> Deserialize<'de>,
    IR: Default + IrExtend + Send + 'static,
{
    /// Construct a new extractor set, validating that all discriminator labels are unique.
    ///
    /// `language` is the language instance used for AST parsing. Since all ast-grep language
    /// types are unit structs that implement `Copy`, this is always a zero-cost value.
    pub(crate) fn new(
        language: L,
        extractors: Vec<Box<dyn SdkExtractor<L, ExtractionResult = IR>>>,
    ) -> std::result::Result<Self, DuplicateDiscriminatorError> {
        let mut seen = HashSet::new();
        for e in &extractors {
            let label = e.discriminator_label();
            if !seen.insert(label) {
                return Err(DuplicateDiscriminatorError { label });
            }
        }
        Ok(Self {
            language,
            extractors: Arc::new(extractors),
        })
    }

    /// Fan out across `source_files` using `spawn_blocking`, merge results, and return
    /// the combined IR.
    ///
    /// This is the method called by the [`LanguageExtractor::extract`] provided default.
    /// It contains the full parallel extraction pipeline; language modules do not need
    /// to reimplement it.
    ///
    /// [`LanguageExtractor::extract`]: super::language_extractor::LanguageExtractor::extract
    pub(crate) async fn extract_from_files(&self, source_files: Vec<SourceFile>) -> Result<IR> {
        // Build the combined rule YAML once — it is the same for all files.
        let combined_yaml = Arc::new(self.build_combined_rule());
        log::trace!("LanguageExtractorSet combined rule:\n{combined_yaml}");

        let language = self.language;

        let mut handles: Vec<tokio::task::JoinHandle<Result<IR>>> =
            Vec::with_capacity(source_files.len());

        for source_file in source_files {
            if !source_file.language.matches(self.language) {
                return Err(ExtractorError::method_extraction(
                    "unknown",
                    source_file.path.clone(),
                    format!(
                        "source file language '{}' does not match extractor language",
                        source_file.language,
                    ),
                ));
            }

            let yaml = Arc::clone(&combined_yaml);
            let extractors = Arc::clone(&self.extractors);

            handles.push(tokio::task::spawn_blocking(move || {
                log::debug!(
                    "LanguageExtractorSet: processing file '{}'",
                    source_file.path.display()
                );
                extract_from_file_with_yaml(&source_file, &yaml, &extractors, language)
            }));
        }

        // Await in submission order so that the merged IR is deterministic.
        let mut combined_result = IR::default();
        for handle in handles {
            let file_result = handle.await.map_err(|e| {
                ExtractorError::method_extraction(
                    "unknown",
                    std::path::PathBuf::from("unknown"),
                    format!("Extraction task panicked: {e}"),
                )
            })??;
            combined_result.extend_from(file_result);
        }

        Ok(combined_result)
    }

    /// Build a combined `any:` rule YAML from all registered extractors.
    ///
    /// Each extractor provides a rule body (content under `rule:`). These are assembled as
    /// items under `rule:\n  any:`.
    pub(crate) fn build_combined_rule(&self) -> String {
        // Derive the display language name (PascalCase) from the type name for the rule id
        // and language field. e.g. "ast_grep_language::Java" → "Java"
        let lang_name = std::any::type_name::<L>()
            .split("::")
            .last()
            .unwrap_or("Unknown");

        let mut yaml = format!("id: {lang_name}_combined\nlanguage: {lang_name}\nrule:\n  any:\n");

        for extractor in self.extractors.as_ref() {
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

/// Process a single source file using the pre-built combined rule YAML.
///
/// This free function is called from `spawn_blocking` tasks in `extract_from_files`.
/// It is generic over `L` and `IR` so the compiler monomorphises it for each language.
fn extract_from_file_with_yaml<L, IR>(
    source_file: &SourceFile,
    combined_yaml: &str,
    extractors: &[Box<dyn SdkExtractor<L, ExtractionResult = IR>>],
    language: L,
) -> Result<IR>
where
    L: LanguageExt + Copy + for<'de> Deserialize<'de>,
    IR: Default,
{
    let globals = ast_grep_config::GlobalRules::default();
    let configs = from_yaml_string::<L>(combined_yaml, &globals).map_err(|e| {
        ExtractorError::method_extraction(
            "unknown",
            source_file.path.clone(),
            format!("Failed to compile combined rule: {e}"),
        )
    })?;

    let config = &configs[0];

    // Parse the source file into an AST using the language instance.
    // All ast-grep language types are unit structs (Copy), so passing by value is cheap.
    let ast_grep = language.ast_grep(&source_file.content);

    let mut result = IR::default();

    // Single AST scan — route each match to the correct extractor via discriminator label.
    for node_match in ast_grep.root().find_all(&config.matcher) {
        let env = node_match.get_env();

        for extractor in extractors {
            if env.get_match(extractor.discriminator_label()).is_some() {
                extractor.process(&node_match, source_file, &mut result);
                break;
            }
        }
    }

    Ok(result)
}

// ================================================================================================
// IrExtend helper trait
// ================================================================================================

/// Helper trait for merging IR values.
///
/// Implemented by language-specific `ExtractionResult` types to support the merge step
/// in [`LanguageExtractorSet::extract_from_files`].
pub(crate) trait IrExtend: Sized {
    /// Merge `other` into `self`.
    fn extend_from(&mut self, other: Self);
}
