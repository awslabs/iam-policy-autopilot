//! Individual extractor implementations for AWS SDK for Java v2.
//!
//! Each module handles one syntactic pattern. All are registered in
//! [`JavaLanguageExtractorSet::default_aws_v2`].
//!
//! [`JavaLanguageExtractorSet::default_aws_v2`]: crate::extraction::java::extractor::JavaLanguageExtractorSet::default_aws_v2

pub(crate) mod import_extractor;
pub(crate) mod method_extractor;
pub(crate) mod paginator_extractor;
pub(crate) mod utility_import_extractor;
pub(crate) mod utils;
pub(crate) mod waiter_extractor;
