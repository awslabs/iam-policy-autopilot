//! Language-agnostic utilities model.
//!
//! Every language's SDK has high-level methods that do not map 1:1 to a single API operation
//! but instead expand to a known set of underlying operations. This module provides the shared
//! types that represent those mappings.
//!
//! # JSON normalisation
//!
//! Each language loads its own JSON into [`UtilitiesModel`]. The JSON format may differ between
//! languages (the Java and Python files have different schemas today); normalisation happens
//! at load time inside each language's `LazyLock` static, not in the framework itself.

use std::collections::HashMap;

// ================================================================================================
// UtilityOperation
// ================================================================================================

/// A single underlying API operation referenced by a utility method.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct UtilityOperation {
    /// Botocore service name, e.g. `"s3"`, `"sqs"`
    pub(crate) service: String,
    /// API operation name (PascalCase), e.g. `"PutObject"`
    pub(crate) name: String,
}

// ================================================================================================
// UtilityMethod
// ================================================================================================

/// A single utility method entry: one high-level SDK call → N underlying API operations.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct UtilityMethod {
    /// The underlying API operations this utility call requires.
    pub(crate) operations: Vec<UtilityOperation>,
    /// Optional receiver class name for languages that use class-based dispatch
    /// (e.g. Java's `S3TransferManager`). Used by the matcher to verify that the
    /// correct class is imported before emitting an SDK call.
    ///
    /// `None` for languages where receiver class matching is not needed.
    #[serde(default)]
    pub(crate) receiver_class: Option<String>,
    /// Optional import package prefix used to classify utility imports during AST extraction.
    ///
    /// For Java, this is the package prefix under which the utility class lives
    /// (e.g. `"software.amazon.awssdk.transfer.s3"`). The import extractor uses this
    /// to decide whether an `import` declaration refers to a utility class.
    ///
    /// `None` for languages that do not use import-based utility classification.
    #[serde(default)]
    pub(crate) import_prefix: Option<String>,
}

// ================================================================================================
// UtilitiesModel
// ================================================================================================

/// The complete utilities model for a language's SDK.
///
/// Maps `service_name → method_name → UtilityMethod`.
/// Loaded once per process from the language's embedded JSON file via a `LazyLock` static.
pub(crate) struct UtilitiesModel {
    pub(crate) services: HashMap<String, HashMap<String, UtilityMethod>>,
}
