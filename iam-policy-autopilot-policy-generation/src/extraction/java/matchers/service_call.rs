//! [`ServiceCallMatcher`] — maps [`Call`]s to [`SdkMethodCall`]s via botocore
//! `method_lookup` and import-based filtering.
//!
//! ## Per-file import filtering
//!
//! In Java every source file must declare its own imports.  Import information from other
//! files in the same project must **not** be used when filtering candidates for a call in
//! this file.  The caller therefore passes a `HashMap<PathBuf, HashSet<String>>` mapping
//! each source file to the set of AWS service names imported in that file.
//!
//! ## Receiver-type matching
//!
//! When the extractor has resolved the receiver variable's declared type via
//! [`Call::receiver_declaration`] (e.g. `"CloudVaultClient"` or the fully-qualified
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
//!    `type_name`.
//!    
//! ## Why there is no parameter-based filtering for Java
//!
//! Java SDK v2 emits a no-arg convenience overload (e.g. `listBuckets()`) for operations
//! whose input shape has no required members and that are listed in the service's
//! `verifiedSimpleMethods` customization, as well as for operations with no botocore input
//! shape at all (~80 across all services). Zero-argument calls are therefore valid SDK
//! calls. We do not filter on argument count because import-based filtering already
//! provides strong matching, and determining which operations have a no-arg overload
//! requires the `verifiedSimpleMethods` allowlists that are outside the embedded botocore
//! model.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::extraction::java::types::{Call, ExtractionResult, Import};
use crate::extraction::{SdkMethodCall, SdkMethodCallMetadata, ServiceModelIndex};

/// Match direct method calls from an [`ExtractionResult`].
///
/// Resolution order per call:
/// 1. **Type-based** (Tier 1): if `call.receiver_declaration` carries a `type_name`,
///    use [`super::resolve_services_by_type_name`] with `type_suffix="Client"` to pin the
///    call to exactly one service.
/// 2. **Import-based fallback**: if `receiver_declaration` is `None` or its `type_name`
///    is `None` (Tier 2/3, `var`, or unresolved), fall back to the file-level import filter.
pub(crate) fn match_service_calls(
    result: &ExtractionResult,
    service_index: &ServiceModelIndex,
    imports_by_file: &HashMap<PathBuf, Vec<&Import>>,
) -> Vec<SdkMethodCall> {
    result
        .calls
        .iter()
        .filter_map(|call| match_call(call, service_index, imports_by_file))
        .collect()
}

fn match_call(
    call: &Call,
    service_index: &ServiceModelIndex,
    imports_by_file: &HashMap<PathBuf, Vec<&Import>>,
) -> Option<SdkMethodCall> {
    let method_name = &call.method;

    // Step 1: Name-based lookup.
    // The Java SDK v2 codegen emits `...AsBytes()` convenience overloads for operations
    // whose output shape has a streaming/blob payload (e.g. `getObjectAsBytes` →
    // `GetObject`).  These are not separate operations in the botocore service model, so
    // we strip the suffix before the lookup and use the stripped name in the output.
    let lookup_name = method_name.strip_suffix("AsBytes").unwrap_or(method_name);
    let refs = service_index.method_lookup.get(lookup_name)?;

    let all_services: Vec<String> = refs.iter().map(|r| r.service_name.clone()).collect();

    // Extract the resolved type name from receiver_declaration, if available.
    let resolved_type = call
        .receiver_declaration
        .as_ref()
        .and_then(|d| d.type_name.as_ref());

    // Tier 1: receiver type resolved at extraction time.
    // Tier 2/3 or unresolved: fall back to file-level import filter.
    // Derive the service-name set on-demand from the full import records.
    let services: Vec<String> = if let Some(type_name) = resolved_type {
        let file_imports = imports_by_file
            .get(&call.location.file_path)
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
            .get(&call.location.file_path)
            .map(|imps| imps.iter().map(|i| i.service.clone()).collect())
            .unwrap_or_default();
        super::apply_import_filter(all_services, &imported_services)
    };

    if services.is_empty() {
        return None;
    }

    let mut metadata = SdkMethodCallMetadata::new(call.expr.clone(), call.location.clone())
        .with_parameters(call.parameters.clone());
    if let Some(receiver) = &call.receiver {
        metadata = metadata.with_receiver(receiver.clone());
    }

    Some(SdkMethodCall {
        name: lookup_name.to_string(),
        possible_services: services,
        metadata: Some(metadata),
    })
}

#[cfg(test)]
mod tests {
    use crate::java_matcher_test;

    java_matcher_test!(
        "tests/java/matchers/service_calls/*.json",
        test_service_call_matching
    );
}
