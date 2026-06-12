//! Language-agnostic extraction framework.
//!
//! This module provides the shared types and traits that formalise the two-phase
//! (extract → match) pipeline for all language extractors.
//!
//! # Framework overview
//!
//! ```text
//! Vec<SourceFile>
//!      |
//!      v  extract()
//! ExtractionResult                       <- language-specific IR, private to the module
//!      |
//!      v  LanguageExtractor::match_calls()
//! Vec<SdkMethodCall>                     <- shared output type
//! ```
//!
//! # Key types
//!
//! - [`SdkExtractor<L>`] — a single-pattern AST extractor for one syntactic construct.
//! - [`LanguageExtractorSet<L, IR>`] — combines multiple `SdkExtractor`s into a single
//!   AST scan per file, with discriminator-label uniqueness enforced at construction.
//! - [`LanguageExtractor`] — the top-level trait implemented by each language module.
//! - [`UtilitiesModel`] — the shared type for language-specific utility method mappings.

pub(crate) mod extractor_set;
pub(crate) mod language_extractor;
pub(crate) mod sdk_extractor;
pub(crate) mod utilities_model;

// Re-export the primary public surface of the framework.
pub(crate) use extractor_set::{IrExtend, LanguageExtractorSet};
pub(crate) use language_extractor::{extract, LanguageExtractor};
pub(crate) use sdk_extractor::SdkExtractor;
pub(crate) use utilities_model::{UtilitiesModel, UtilityMethod, UtilityOperation};
