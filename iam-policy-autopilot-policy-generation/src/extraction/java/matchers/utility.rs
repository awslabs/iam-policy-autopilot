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
//!   1a. **FQN fast path**: if `type_name` starts with `feature.import_prefix + "."`, extract the
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

use crate::extraction::framework::UtilitiesModel;
use crate::extraction::java::types::{ExtractionResult, UtilityImport};
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata, ServiceModelIndex};

/// Match utility calls from an [`ExtractionResult`] using the utilities model.
///
/// For each [`Call`] in `result.calls`, checks whether the receiver's class
/// (resolved via `utility_imports` **from the same source file**) matches a
/// `receiver_class` in the model, and whether the method name matches.
/// If both match, emits one [`SdkMethodCall`] **per operation** listed in the feature,
/// using the operation's `Name` (PascalCase API operation name) and `Service`.
pub(crate) fn match_utilities(
    result: &ExtractionResult,
    model: &UtilitiesModel,
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

        // For each service in the model, look for a feature whose receiver_class matches
        // and whose method name matches the call method.
        for (service_name, methods) in &model.services {
            let Some(utility_method) = methods.get(&call.method) else {
                continue;
            };

            // Extract the resolved type name from receiver_declaration, if available.
            let resolved_type = call
                .receiver_declaration
                .as_ref()
                .and_then(|d| d.type_name.as_ref());

            let receiver_matches = match &utility_method.receiver_class {
                Some(expected_class) => {
                    if let Some(type_name) = resolved_type {
                        // Tier 1: type name is known — use it exclusively.
                        //
                        // 1a. FQN fast path: the type was declared with a fully-qualified name
                        //     (e.g. `software.amazon.awssdk.enhanced.dynamodb.DynamoDbTable<Customer>`).
                        //     Strip the feature's import prefix, then compare the last segment
                        //     (with any trailing generic suffix removed) against receiver_class.
                        let fqn_match = utility_method
                            .import_prefix
                            .as_ref()
                            .and_then(|prefix| {
                                let fqn_prefix = format!("{prefix}.");
                                type_name.strip_prefix(fqn_prefix.as_str()).map(|rest| {
                                    let class_part = rest.split('<').next().unwrap_or(rest);
                                    class_part == expected_class.as_str()
                                })
                            })
                            .unwrap_or(false);

                        // 1b. Simple name: the extractor already stripped generic parameters
                        fqn_match || type_name == expected_class
                    } else {
                        // Tier 2: type name unknown — fall back to import evidence.
                        //
                        // 2a: a specific UtilityImport with this class_name exists in the file
                        let class_imported = file_utility_imports
                            .iter()
                            .any(|ui| ui.class_name == *expected_class);
                        // 2b: a wildcard UtilityImport for the same service exists in the file
                        let wildcard_imported = file_utility_imports
                            .iter()
                            .any(|ui| ui.class_name == "*" && &ui.utility_name == service_name);

                        class_imported || wildcard_imported
                    }
                }
                // No receiver class constraint — match on method name alone.
                None => !file_utility_imports.is_empty(),
            };

            if !receiver_matches {
                continue;
            }

            // Emit one SdkMethodCall per operation in the utility method.
            for op in &utility_method.operations {
                let metadata = SdkMethodCallMetadata::new(call.expr.clone(), call.location.clone())
                    .with_parameters(call.parameters.clone());

                output.push(SdkMethodCall {
                    name: op.name.clone(),
                    possible_services: vec![op.service.clone()],
                    metadata: Some(metadata),
                });
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
