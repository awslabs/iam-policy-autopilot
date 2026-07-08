//! Index over the embedded `terraform-model.json`.
//!
//! The committed model is an [`ExternalLibraryModel`] (`library_name:
//! "terraform-provider-aws"`, `language: go`). Each `call_pattern` maps a
//! handler `(module_path, class_name, function_name)` to the AWS SDK
//! operations it invokes. This module parses the model once and exposes a
//! lookup keyed by [`CallPatternKey`] so the mapper can resolve a CRUD handler
//! symbol to its operations.

use std::collections::HashMap;

use anyhow::{ensure, Context, Result};

use crate::extraction::external_library_models::{
    CallPatternKey, ExternalLibraryModel, SdkOperationMapping,
};
use crate::Language;

use super::{TerraformArtifacts, TERRAFORM_LIBRARY_NAME};

/// Lookup of handler join key → SDK operations, plus the model's version tag.
pub(crate) struct ModelIndex {
    by_handler: HashMap<CallPatternKey, Vec<SdkOperationMapping>>,
    version: Option<String>,
}

impl ModelIndex {
    /// Parse and index the embedded `terraform-model.json`.
    pub(crate) fn load() -> Result<Self> {
        let bytes = TerraformArtifacts::model_bytes();
        Self::from_slice(&bytes)
    }

    /// Build an index from raw model JSON bytes.
    fn from_slice(bytes: &[u8]) -> Result<Self> {
        let model: ExternalLibraryModel =
            serde_json::from_slice(bytes).context("Failed to parse terraform-model.json")?;
        Self::from_model(model)
    }

    /// Build an index from an already-parsed model, validating its identity.
    fn from_model(model: ExternalLibraryModel) -> Result<Self> {
        ensure!(
            model.library_name == TERRAFORM_LIBRARY_NAME,
            "Unexpected terraform model library_name: {:?} (expected {:?})",
            model.library_name,
            TERRAFORM_LIBRARY_NAME
        );
        ensure!(
            model.language == Language::Go,
            "Unexpected terraform model language: {:?} (expected Go)",
            model.language
        );

        let mut by_handler: HashMap<CallPatternKey, Vec<SdkOperationMapping>> = HashMap::new();
        for pattern in model.call_patterns {
            // The model builder unions duplicate (module_path, class, func)
            // patterns, so keys are unique; extend defensively in case that
            // ever changes.
            by_handler
                .entry(pattern.key())
                .or_default()
                .extend(pattern.sdk_operations);
        }

        Ok(Self {
            by_handler,
            version: model.version,
        })
    }

    /// SDK operations invoked by the handler with this join key, if modeled.
    pub(crate) fn operations(&self, key: &CallPatternKey) -> Option<&[SdkOperationMapping]> {
        self.by_handler.get(key).map(Vec::as_slice)
    }

    /// The provider version tag the model was built against (e.g. `v6.34.0`).
    pub(crate) fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Every handler join key present in the model.
    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &CallPatternKey> {
        self.by_handler.keys()
    }

    /// Build an index from raw JSON bytes, for cross-module tests.
    #[cfg(test)]
    pub(crate) fn from_slice_for_test(bytes: &[u8]) -> Self {
        Self::from_slice(bytes).expect("valid test model JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const SAMPLE: &str = r#"{
        "library_name": "terraform-provider-aws",
        "language": "go",
        "version": "v6.34.0",
        "call_patterns": [
            {
                "module_path": "accessanalyzer",
                "class_name": null,
                "function_name": "resourceAnalyzerCreate",
                "call_type": "function",
                "sdk_operations": [
                    { "service": "accessanalyzer", "operation": "CreateAnalyzer" },
                    { "service": "accessanalyzer", "operation": "GetAnalyzer" }
                ]
            },
            {
                "module_path": "sqs",
                "class_name": "queueAttributeHandler",
                "function_name": "Upsert",
                "call_type": "instance_method",
                "sdk_operations": [
                    { "service": "sqs", "operation": "SetQueueAttributes" }
                ]
            }
        ]
    }"#;

    fn op(service: &str, operation: &str) -> SdkOperationMapping {
        SdkOperationMapping {
            service: service.to_string(),
            operation: operation.to_string(),
        }
    }

    #[test]
    fn indexes_free_function_handler() {
        let index = ModelIndex::from_slice(SAMPLE.as_bytes()).unwrap();
        let key = CallPatternKey {
            module_path: "accessanalyzer".to_string(),
            class_name: None,
            function_name: "resourceAnalyzerCreate".to_string(),
        };
        assert_eq!(
            index.operations(&key),
            Some(
                [
                    op("accessanalyzer", "CreateAnalyzer"),
                    op("accessanalyzer", "GetAnalyzer")
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn indexes_method_handler_by_class_and_method() {
        let index = ModelIndex::from_slice(SAMPLE.as_bytes()).unwrap();
        let key = CallPatternKey {
            module_path: "sqs".to_string(),
            class_name: Some("queueAttributeHandler".to_string()),
            function_name: "Upsert".to_string(),
        };
        assert_eq!(
            index.operations(&key),
            Some([op("sqs", "SetQueueAttributes")].as_slice())
        );
    }

    #[test]
    fn unknown_handler_is_none() {
        let index = ModelIndex::from_slice(SAMPLE.as_bytes()).unwrap();
        let key = CallPatternKey {
            module_path: "s3".to_string(),
            class_name: None,
            function_name: "resourceBucketCreate".to_string(),
        };
        assert_eq!(index.operations(&key), None);
    }

    #[test]
    fn exposes_version() {
        let index = ModelIndex::from_slice(SAMPLE.as_bytes()).unwrap();
        assert_eq!(index.version(), Some("v6.34.0"));
    }

    #[rstest]
    #[case(r#"{"library_name":"wrong","language":"go","call_patterns":[]}"#)]
    #[case(r#"{"library_name":"terraform-provider-aws","language":"python","call_patterns":[]}"#)]
    fn rejects_wrong_identity(#[case] json: &str) {
        assert!(ModelIndex::from_slice(json.as_bytes()).is_err());
    }

    #[test]
    fn embedded_model_parses_and_resolves_known_handler() {
        let index = ModelIndex::load().unwrap();
        assert_eq!(index.version(), Some("v6.34.0"));
        let key = CallPatternKey {
            module_path: "accessanalyzer".to_string(),
            class_name: None,
            function_name: "resourceAnalyzerCreate".to_string(),
        };
        // Action set must be present for a known handler.
        assert!(index.operations(&key).is_some());
    }
}
