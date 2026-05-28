//! [`match_utilities`] — maps [`Call`]s to [`SdkMethodCall`]s via
//! the `java-sdk-v2-utilities.json` model and `utility_imports`-based filtering.
//!
//! Matches a call against the utility model using:
//! 1. `call.receiver_declaration.type_name` → resolved type of the receiver variable
//! 2. `call.method` → `MethodName` lookup within the matched service entries
//!
//! Only utility imports from the same source file as the call are considered.
//!
//! ## Receiver matching tiers
//!
//! Resolution mirrors the service-call matcher: type-name evidence takes priority over
//! import evidence, and import evidence is only consulted when the type is unknown.
//!
//! **Tier 1 — type name known** (`receiver_declaration.type_name` is `Some`):
//!   1a. **FQN fast path**: if `type_name` starts with `feature.import + "."`, extract the
//!       class name from the FQN (last `.`-separated segment, generic suffix stripped) and
//!       compare against `feature.receiver_class`.
//!       e.g. `"software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable<Customer>"` →
//!       class `"DynamoDbTable"` matches `ReceiverClass = "DynamoDbTable"`.
//!   1b. **Simple name**: `type_name == feature.receiver_class` (the common case after
//!       the extractor has already stripped generic type parameters).
//!   This tier is definitive: no import evidence is needed or consulted.
//!
//! **Tier 2 — type name unknown** (`type_name` is `None`, e.g. `var`, unresolved, or a
//!   complex receiver expression like a chained call):
//!   Fall back to import evidence from the same file:
//!   - A specific `UtilityImport` with `class_name == feature.receiver_class` exists, **or**
//!   - A wildcard `UtilityImport` (`class_name == "*"`) for the same service exists
//!     (covers `import ...dynamodb.*` — the package is in scope).
//!
//! The raw `receiver` string is not used for matching: utility classes are always used via
//! instance methods on objects created through factory/builder patterns (e.g.
//! `S3TransferManager.create()`), so the receiver text never equals the class name.

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
        // Collect utility imports from the same file for Tier-2 import-based fallback.
        let file_utility_imports: &[&UtilityImport] = utility_imports_by_file
            .get(&call.location.file_path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        // For each service in the model, look for a feature whose ReceiverClass matches
        // and whose MethodName matches the call method.
        for (service_name, features) in &model.services {
            for feature in features.values() {
                // Check method name matches
                if feature.method_name != call.method {
                    continue;
                }

                // Extract the resolved type name from receiver_declaration, if available.
                let resolved_type = call
                    .receiver_declaration
                    .as_ref()
                    .and_then(|d| d.type_name.as_ref());

                let receiver_matches = if let Some(type_name) = resolved_type {
                    // Tier 1: type name is known — use it exclusively.
                    // This is definitive: no import evidence is needed or consulted.
                    //
                    // 1a. FQN fast path: the type was declared with a fully-qualified name
                    //     (e.g. `software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable<Customer>`).
                    //     The extractor stores the full scoped text for such types.
                    //     Strip the feature's import prefix, then compare the last segment
                    //     (with any trailing generic suffix removed) against receiver_class.
                    let fqn_prefix = format!("{}.", feature.import);
                    let fqn_match = type_name
                        .strip_prefix(fqn_prefix.as_str())
                        .map(|rest| {
                            // rest may be e.g. "DynamoDbTable<Customer>" or "S3TransferManager"
                            let class_part = rest.split('<').next().unwrap_or(rest);
                            class_part == feature.receiver_class
                        })
                        .unwrap_or(false);

                    // 1b. Simple name: the extractor already stripped generic parameters
                    //     (e.g. `DynamoDbTable<Customer>` → `DynamoDbTable`).
                    fqn_match || type_name == &feature.receiver_class
                } else {
                    // Tier 2: type name unknown (var, unresolved, or complex receiver expression
                    // such as a chained call) — fall back to import evidence from the same file.
                    //
                    // 2a: a specific UtilityImport with this class_name exists in the file
                    let class_imported = file_utility_imports
                        .iter()
                        .any(|ui| ui.class_name == feature.receiver_class);
                    // 2b: a wildcard UtilityImport for the same service exists in the file
                    // (e.g. `import software.amazon.awssdk.enhanced.dynamodb.*`)
                    let wildcard_imported = file_utility_imports
                        .iter()
                        .any(|ui| ui.class_name == "*" && &ui.utility_name == service_name);

                    class_imported || wildcard_imported
                };

                if !receiver_matches {
                    continue;
                }

                // Emit one SdkMethodCall per operation in the feature, using the
                // operation's API name (PascalCase) and service from the model.
                for op in &feature.operations {
                    let metadata =
                        SdkMethodCallMetadata::new(call.expr.clone(), call.location.clone())
                            .with_parameters(call.parameters.clone());

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
