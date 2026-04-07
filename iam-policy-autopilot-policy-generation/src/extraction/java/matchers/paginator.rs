//! [`match_paginators`] — maps [`Paginator`]s to [`SdkMethodCall`]s via
//! [`ServiceModelIndex::method_lookup`] and import-based filtering.
//!
//! Import filtering uses only the imports from the same source file as each paginator call.
//! See [`service_call`] module documentation for the rationale.
//!
//! When the extractor has resolved the receiver variable's declared type via
//! [`Paginator::receiver_declaration`] (e.g. `"CloudVaultClient"` or the fully-qualified
//! `"software.amazon.awssdk.services.cloudvault.CloudVaultClient"`), the matcher
//! performs a targeted lookup to pin the call to exactly one service, rather than relying
//! on the coarser file-level import filter.
//!
//! Resolution order for Tier 1 (type name known):
//! 1. **FQN fast path**: if `type_name` starts with `software.amazon.awssdk.services.`,
//!    extract the service name directly from the prefix.
//! 2. **Import lookup**: match `type_name` against the last segment of each specific import
//!    in the file (handles the common `import ...CloudVaultClient;` case).
//! 3. **Service-id matching**: if no specific import matches (e.g. the file uses a `.*`
//!    wildcard import), derive the expected client class name from each candidate service's
//!    `serviceId` metadata (`<ServiceId>Client`, spaces stripped) and match against
//!    `type_name`.  This handles `import software.amazon.awssdk.services.cloudvault.*;`
//!    with `CloudVaultClient client = ...`.
//!
//! [`service_call`]: super::service_call

use std::collections::HashMap;
use std::path::PathBuf;

use crate::extraction::java::types::{ExtractionResult, Import};
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata, ServiceModelIndex};

/// Disambiguate paginator calls from an [`ExtractionResult`].
///
/// The `operation` field on each [`Paginator`] already has the `"Paginator"` suffix
/// stripped by the extractor (e.g. `"listObjectsV2"`).
///
/// Resolution order:
/// 1. **Type-based** (Tier 1): if `paginator.receiver_declaration` carries a `type_name`,
///    use [`super::resolve_services_by_type_name`] with `type_suffix="Client"` to pin the
///    call to exactly one service.
/// 2. **Import-based fallback**: if `receiver_declaration` is `None` or its `type_name`
///    is `None` (Tier 2/3, `var`, or unresolved), fall back to the file-level import filter.
pub(crate) fn match_paginators(
    result: &ExtractionResult,
    service_index: &ServiceModelIndex,
    imports_by_file: &HashMap<PathBuf, Vec<&Import>>,
) -> Vec<SdkMethodCall> {
    let mut output = Vec::new();

    for paginator in &result.paginators {
        let method_name = &paginator.operation;

        // The paginator operation name is already camelCase (the "Paginator" suffix is stripped
        // from the source method name at extraction time, which is already camelCase).
        // The method_lookup is keyed by the same camelCase names, so a direct lookup suffices.
        let refs = service_index.method_lookup.get(method_name);

        let Some(refs) = refs else { continue };

        let all_services: Vec<String> = refs.iter().map(|r| r.service_name.clone()).collect();

        // Extract the resolved type name from receiver_declaration, if available.
        let resolved_type = paginator
            .receiver_declaration
            .as_ref()
            .and_then(|d| d.type_name.as_ref());

        // Tier 1: receiver type resolved at extraction time.
        // Tier 2/3 or unresolved: fall back to file-level import filter.
        // Derive the service-name set on-demand from the full import records.
        let services: Vec<String> = if let Some(type_name) = resolved_type {
            let file_imports = imports_by_file
                .get(&paginator.location.file_path)
                .map(std::vec::Vec::as_slice)
                .unwrap_or(&[]);

            super::resolve_services_by_type_name(
                type_name,
                &["Client", "AsyncClient"],
                all_services,
                file_imports,
                service_index,
            )
        } else {
            let imported_services: std::collections::HashSet<String> = imports_by_file
                .get(&paginator.location.file_path)
                .map(|imps| imps.iter().map(|i| i.service.clone()).collect())
                .unwrap_or_default();
            super::apply_import_filter(all_services, &imported_services)
        };

        if services.is_empty() {
            continue;
        }

        let metadata =
            SdkMethodCallMetadata::new(paginator.expr.clone(), paginator.location.clone())
                .with_parameters(paginator.parameters.clone());

        output.push(SdkMethodCall {
            name: method_name.clone(),
            possible_services: services,
            metadata: Some(metadata),
        });
    }

    output
}

#[cfg(test)]
mod tests {
    use crate::java_matcher_test;

    java_matcher_test!(
        "tests/java/matchers/paginators/*.json",
        test_paginator_matching
    );
}
