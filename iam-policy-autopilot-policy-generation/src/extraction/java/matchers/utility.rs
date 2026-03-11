//! [`match_utilities`] — maps [`Call`]s to [`SdkMethodCall`]s via
//! the `java-sdk-v2-utilities.json` model and `utility_imports`-based filtering.
//!
//! Matches a call against the utility model using:
//! 1. `call.receiver` → resolved to a class name via `utility_imports` from the same file
//! 2. `call.method` → `MethodName` lookup within the matched service entries
//!
//! Only utility imports from the same source file as the call are considered.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::extraction::java::types::{ExtractionResult, UtilityImport};
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata, ServiceModelIndex};

// ================================================================================================
// Utility model types (mirrors java-sdk-v2-utilities.json)
// ================================================================================================

/// An operation reference in `java-sdk-v2-utilities.json`.
///
/// Stored as `{ "Service": "s3", "Name": "PutObject" }` — the enrichment phase
/// resolves these to IAM actions via the service model.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct JavaUtilityOperation {
    /// Service identifier, e.g. `"s3"`, `"sqs"`
    pub(crate) service: String,
    /// API operation name (PascalCase), e.g. `"PutObject"`, `"SendMessageBatch"`
    pub(crate) name: String,
}

/// A single utility feature entry from `java-sdk-v2-utilities.json`.
// `import` and `operations` are deserialized from JSON and stored for future use
// (e.g. policy generation); they are not yet consumed by the matching logic.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct JavaUtilityFeature {
    /// SDK method name, e.g. `"uploadFile"`
    pub(crate) method_name: String,
    /// Receiver class name, e.g. `"S3TransferManager"`
    pub(crate) receiver_class: String,
    /// Import package prefix, e.g. `"software.amazon.awssdk.transfer.s3"`
    pub(crate) import: String,
    /// API operations this utility call requires (resolved to IAM actions by the enrichment phase)
    pub(crate) operations: Vec<JavaUtilityOperation>,
}

/// Top-level structure of `java-sdk-v2-utilities.json`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct JavaUtilitiesModel {
    pub(crate) services: HashMap<String, HashMap<String, JavaUtilityFeature>>,
}

// ================================================================================================
// UtilityMatcher
// ================================================================================================

/// Match utility calls from an [`ExtractionResult`] using the
/// `java-sdk-v2-utilities.json` model.
///
/// For each [`Call`] in `result.calls`, checks whether the receiver's class
/// (resolved via `utility_imports` **from the same source file**) matches a
/// `ReceiverClass` in the model, and whether the method name matches a `MethodName`.
/// If both match, emits one [`SdkMethodCall`] **per operation** listed in the feature,
/// using the operation's `Name` (PascalCase API operation name) and `Service`.
pub(crate) fn match_utilities(
    result: &ExtractionResult,
    model: &JavaUtilitiesModel,
    _service_index: &ServiceModelIndex,
    utility_imports_by_file: &HashMap<PathBuf, Vec<&UtilityImport>>,
) -> Vec<SdkMethodCall> {
    let mut output = Vec::new();

    for call in &result.calls {
        let Some(receiver) = &call.receiver else {
            continue;
        };

        // Resolve receiver variable → class name via utility_imports from the same file only.
        let file_utility_imports: &[&UtilityImport] = utility_imports_by_file
            .get(&call.location.file_path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        // Direct match: receiver variable name equals class name (rare but valid)
        let matched_class: Option<&str> = file_utility_imports
            .iter()
            .find(|ui| receiver == &ui.class_name)
            .map(|ui| ui.class_name.as_str());

        // For each service in the model, look for a feature whose ReceiverClass matches
        // either the matched_class or any utility import class in the same file, and whose
        // MethodName matches the call method.
        for features in model.services.values() {
            for feature in features.values() {
                // Check method name matches
                if feature.method_name != call.method {
                    continue;
                }

                // Check receiver class matches:
                // - direct class name match (e.g. receiver == "S3TransferManager")
                // - OR a utility import with this class_name exists in the same file
                let class_imported = file_utility_imports
                    .iter()
                    .any(|ui| ui.class_name == feature.receiver_class);

                let receiver_matches =
                    matched_class == Some(feature.receiver_class.as_str()) || class_imported;

                if !receiver_matches {
                    continue;
                }

                // Emit one SdkMethodCall per operation in the feature, using the
                // operation's API name (PascalCase) and service from the model.
                for op in &feature.operations {
                    let metadata =
                        SdkMethodCallMetadata::new(call.expr.clone(), call.location.clone())
                            .with_parameters(call.parameters.clone())
                            .with_receiver(receiver.clone());

                    output.push(SdkMethodCall {
                        name: op.name.clone(),
                        possible_services: vec![op.service.clone()],
                        metadata: Some(metadata),
                    });
                }
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use crate::java_matcher_test;

    java_matcher_test!("tests/java/matchers/utility/*.json", test_utility_matching);
}
