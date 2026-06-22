//! Individual extractor implementations for AWS SDK for Java v2.
//!
//! Each module handles one syntactic pattern. All are registered in
//! [`JavaLanguageExtractor::extractor_set`].
//!
//! [`JavaLanguageExtractor::extractor_set`]: crate::extraction::java::JavaLanguageExtractor::extractor_set

pub(crate) mod import_extractor;
pub(crate) mod method_extractor;
pub(crate) mod paginator_extractor;
pub(crate) mod utility_import_extractor;
pub(crate) mod utils;
pub(crate) mod waiter_extractor;
