//! [`SdkExtractor<L>`] trait — a single-pattern AST extractor for one syntactic construct.

use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_core::NodeMatch;
use ast_grep_language::LanguageExt;

use crate::extraction::SourceFile;

/// A single-pattern AST extractor for one syntactic construct in a given language.
///
/// # Type parameter
/// `L` is the ast-grep language type (e.g. `ast_grep_language::Java`).
/// Each language's extractor set is parameterised over its own `L`.
///
/// # Discriminator contract
/// Every `SdkExtractor<L>` registered in a [`LanguageExtractorSet`] must return a
/// **unique** `discriminator_label`. The set enforces this at construction time via
/// [`LanguageExtractorSet::new`], which returns `Err(DuplicateDiscriminatorError)` if
/// any two extractors share a label.
///
/// # Rule format
/// `rule_yaml` must return the **rule body** — the content under `rule:` in the YAML,
/// indented with two spaces. The `id:` and `language:` keys are added by
/// [`LanguageExtractorSet`]. The rule body **must** capture `discriminator_label` as a
/// metavariable so the set can route each match to the correct extractor.
///
/// [`LanguageExtractorSet`]: super::extractor_set::LanguageExtractorSet
pub(crate) trait SdkExtractor<L: LanguageExt>: Send + Sync {
    /// The IR type this extractor writes into.
    /// All extractors in a [`LanguageExtractorSet`] must share the same `ExtractionResult`.
    ///
    /// [`LanguageExtractorSet`]: super::extractor_set::LanguageExtractorSet
    type ExtractionResult;

    /// The rule body YAML — content under `rule:`, indented with two spaces.
    fn rule_yaml(&self) -> &'static str;

    /// Unique metavariable label captured by this extractor's rule.
    /// No two extractors in the same [`LanguageExtractorSet`] may return the same label.
    ///
    /// [`LanguageExtractorSet`]: super::extractor_set::LanguageExtractorSet
    fn discriminator_label(&self) -> &'static str;

    /// Process a single AST match and append findings to `result`.
    fn process(
        &self,
        node_match: &NodeMatch<'_, StrDoc<L>>,
        source_file: &SourceFile,
        result: &mut Self::ExtractionResult,
    );
}
