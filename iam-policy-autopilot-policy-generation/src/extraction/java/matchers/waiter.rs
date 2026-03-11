//! [`match_waiters`] — maps [`Waiter`]s to [`SdkMethodCall`]s via
//! [`ServiceModelIndex::waiter_lookup`] and import-based filtering.
//!
//! Import filtering uses only the imports from the same source file as each waiter call.
//! See [`service_call`] module documentation for the rationale.
//!
//! When the extractor has resolved the receiver variable's declared type via
//! [`Waiter::receiver_declaration`] (e.g. `"S3Waiter"` or the fully-qualified
//! `"software.amazon.awssdk.services.s3.waiters.S3Waiter"`), the matcher performs
//! a targeted lookup to pin the call to exactly one service, rather than relying on
//! the coarser file-level import filter.
//!
//! Resolution order for Tier 1 (type name known):
//! 1. **FQN fast path**: if `type_name` starts with `software.amazon.awssdk.services.`,
//!    extract the service name directly from the prefix.
//! 2. **Import lookup**: match `type_name` against the last segment of each specific import
//!    in the file (handles the common `import ...S3Waiter;` case).
//! 3. **Service-id matching**: if no specific import matches (e.g. the file uses a `.*`
//!    wildcard import), derive the expected waiter interface name from each candidate
//!    service's `serviceId` metadata (`<ServiceId>Waiter`, spaces stripped) and match
//!    against `type_name`.  This handles `import software.amazon.awssdk.services.s3.*;`
//!    with `S3Waiter waiter = ...`.
//!
//! # Why `SdkMethodCall.name` uses the underlying operation, not the waiter name
//!
//! A Java waiter call like `waiter.waitUntilTableExists(req)` internally polls
//! `DynamoDB::DescribeTable`.  The enrichment layer converts `SdkMethodCall.name`
//! from camelCase to PascalCase and looks it up in the service reference to find the
//! required IAM actions.  If we stored the waiter name (`"tableExists"`) the enrichment
//! would look for a `"TableExists"` operation — which does not exist in the service
//! reference — and produce an empty policy.  Instead we store the **underlying operation
//! name** in camelCase (e.g. `"describeTable"`), so enrichment correctly resolves it to
//! `"DescribeTable"` → `dynamodb:DescribeTable`.
//!
//! When a waiter maps to the same underlying operation across multiple services (e.g.
//! two fictional services both poll `DescribeResource`), all refs share the same
//! `operation_name`, so the single camelCase name is unambiguous.
//!
//! [`service_call`]: super::service_call

use convert_case::{Case, Casing};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::extraction::java::types::{ExtractionResult, Import};
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata, ServiceModelIndex};

/// Match waiter calls from an [`ExtractionResult`].
///
/// All waiters in the result are `waitUntil*` calls — there are no longer separate init
/// records. The `waiter_type` field has the `"waitUntil"` prefix already stripped by the
/// extractor.
///
/// The `SdkMethodCall.name` is set to the **underlying polling operation** in camelCase
/// (e.g. `"describeTable"` for `waitUntilTableExists`), not the waiter name.  This
/// ensures the enrichment layer can resolve the correct IAM action (`dynamodb:DescribeTable`)
/// rather than searching for a non-existent `"TableExists"` operation.
///
/// Resolution order:
/// 1. **Type-based** (Tier 1): if `waiter.receiver_declaration` carries a `type_name`,
///    use [`super::resolve_services_by_type_name`] to pin the call to exactly one service.
/// 2. **Import-based fallback**: if `receiver_declaration` is `None` or its `type_name`
///    is `None` (Tier 2/3, `var`, or unresolved), fall back to the file-level import filter.
pub(crate) fn match_waiters(
    result: &ExtractionResult,
    service_index: &ServiceModelIndex,
    imports_by_file: &HashMap<PathBuf, Vec<&Import>>,
) -> Vec<SdkMethodCall> {
    let mut output = Vec::new();

    for waiter in &result.waiters {
        let waiter_type = &waiter.name;

        // waiter_type is already camelCase (converted at extraction time by the extractor).
        // The waiter_lookup is keyed by the same camelCase names, so a direct lookup suffices.
        let refs = service_index.waiter_lookup.get(waiter_type);

        let Some(refs) = refs else { continue };

        let all_services: Vec<String> = refs.iter().map(|r| r.service_name.clone()).collect();

        // Extract the resolved type name from receiver_declaration, if available.
        let resolved_type = waiter
            .receiver_declaration
            .as_ref()
            .and_then(|d| d.type_name.as_ref());

        // Tier 1: receiver type resolved at extraction time.
        // Tier 2/3 or unresolved: fall back to file-level import filter.
        // Derive the service-name set on-demand from the full import records.
        let services: Vec<String> = if let Some(type_name) = resolved_type {
            let file_imports = imports_by_file
                .get(&waiter.location.file_path)
                .map(std::vec::Vec::as_slice)
                .unwrap_or(&[]);

            super::resolve_services_by_type_name(
                type_name,
                &["Waiter", "AsyncWaiter"],
                all_services,
                file_imports,
                service_index,
            )
        } else {
            let imported_services: std::collections::HashSet<String> = imports_by_file
                .get(&waiter.location.file_path)
                .map(|imps| imps.iter().map(|i| i.service.clone()).collect())
                .unwrap_or_default();
            super::apply_import_filter(all_services, &imported_services)
        };

        if services.is_empty() {
            continue;
        }

        // Group the matched services by their underlying polling operation name.
        //
        // Different services can share the same waiter name but poll different operations.
        // For example, both `eks` (DescribeCluster) and `dsql` (GetCluster) have a
        // `ClusterActive` waiter.  Taking `refs.first().operation_name` would produce the
        // wrong operation name for whichever service happened to be loaded second.
        //
        // We therefore build one `SdkMethodCall` per distinct (operation_name, services)
        // group so that each call carries the correct camelCase operation name for its
        // service set.
        //
        // The enrichment layer (Operation::from_call with SdkType::JavaV2) converts
        // camelCase → PascalCase, so "describeCluster" → "DescribeCluster", which correctly
        // resolves to eks:DescribeCluster in the service reference.
        let mut by_operation: Vec<(String, Vec<String>)> = Vec::new();
        for ref_entry in refs {
            if !services.contains(&ref_entry.service_name) {
                continue;
            }
            let method_name = ref_entry.operation_name.to_case(Case::Camel);
            if let Some(group) = by_operation.iter_mut().find(|(op, _)| op == &method_name) {
                group.1.push(ref_entry.service_name.clone());
            } else {
                by_operation.push((method_name, vec![ref_entry.service_name.clone()]));
            }
        }

        // If grouping produced nothing (shouldn't happen given the services check above),
        // fall back to a single entry using the first ref's operation name.
        if by_operation.is_empty() {
            let method_name = refs
                .first()
                .map(|r| r.operation_name.to_case(Case::Camel))
                .unwrap_or_else(|| waiter_type.clone());
            by_operation.push((method_name, services));
        }

        for (method_name, group_services) in by_operation {
            let metadata = SdkMethodCallMetadata::new(waiter.expr.clone(), waiter.location.clone())
                .with_parameters(waiter.parameters.clone());

            output.push(SdkMethodCall {
                name: method_name,
                possible_services: group_services,
                metadata: Some(metadata),
            });
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use crate::java_matcher_test;

    java_matcher_test!("tests/java/matchers/waiters/*.json", test_waiter_matching);
}
