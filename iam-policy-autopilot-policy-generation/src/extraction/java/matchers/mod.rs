//! Sub-matchers for AWS SDK for Java v2.
//!
//! Each module handles one category of extraction result:
//! - [`service_call`] — direct method calls × botocore `method_lookup`
//! - [`waiter`]       — waiter calls × `waiter_lookup`
//! - [`paginator`]    — paginator calls × `method_lookup`
//! - [`utility`]      — utility calls × `java-sdk-v2-utilities.json`

pub(crate) mod paginator;
pub(crate) mod service_call;
pub(crate) mod utility;
pub(crate) mod waiter;

use std::collections::HashSet;

use crate::extraction::java::types::Import;
use crate::extraction::ServiceModelIndex;

/// Apply import-based filtering to a list of candidate services.
///
/// If `imported_services` is non-empty, retain only services that appear in it.
/// Returns an empty list if no candidates match the imports; callers must not emit
/// an [`SdkMethodCall`] in that case.
///
/// [`SdkMethodCall`]: crate::extraction::SdkMethodCall
pub(crate) fn apply_import_filter(
    services: Vec<String>,
    imported_services: &HashSet<String>,
) -> Vec<String> {
    if imported_services.is_empty() {
        return services;
    }

    services
        .into_iter()
        .filter(|s| imported_services.contains(s))
        .collect()
}

/// Resolve the set of candidate services using the receiver variable's declared type name.
///
/// This is the **Tier 1** resolution path used by the waiter, paginator, and service-call
/// matchers when the extractor has successfully resolved the receiver variable's
/// declared type (e.g. `"S3Waiter"`, `"S3AsyncWaiter"`, `"CloudVaultClient"`,
/// `"CloudVaultAsyncClient"`, or a fully-qualified name).
///
/// Resolution order:
/// 1. **FQN fast path**: if `type_name` starts with `software.amazon.awssdk.services.`,
///    extract the service name directly from the prefix (no import lookup needed).
/// 2. **Import lookup**: match `type_name` against the last segment of each specific import
///    in the file (handles the common `import ...S3AsyncClient;` case).
/// 3. **Service-id matching**: if no specific import matches (e.g. wildcard import), derive
///    the expected type name from each candidate service's `serviceId` metadata and try
///    each of the provided `type_suffixes` in order
///    (`<ServiceId><suffix>`, spaces stripped) until one matches `type_name`.
///    e.g. `serviceId "S3"` + suffixes `["Client", "AsyncClient"]` matches both
///    `"S3Client"` and `"S3AsyncClient"`.
///
/// Returns a filtered subset of `all_services` (may be empty if nothing matches).
pub(crate) fn resolve_services_by_type_name(
    type_name: &str,
    type_suffixes: &[&str],
    all_services: Vec<String>,
    file_imports: &[&Import],
    service_index: &ServiceModelIndex,
) -> Vec<String> {
    // Fast path: fully-qualified type name — extract service directly from the FQN.
    // e.g. "software.amazon.awssdk.services.s3.waiters.S3AsyncWaiter" → "s3"
    if let Some(svc) = extract_service_from_fqn(type_name) {
        return all_services.into_iter().filter(|s| s == &svc).collect();
    }

    // Slow path: simple (unqualified) type name — look up service via the file's import list.
    // The import path's last segment (e.g. "S3AsyncClient") is matched against `type_name`.
    let service = file_imports
        .iter()
        .find(|imp| imp.expr.split('.').next_back() == Some(type_name))
        .map(|imp| imp.service.clone());

    match service {
        Some(svc) => {
            // Filter the full candidate list to only the pinned service
            all_services.into_iter().filter(|s| s == &svc).collect()
        }
        None => {
            // No specific import matched — the file may use a wildcard import.
            // Derive the expected type name from each candidate service's `serviceId` metadata.
            // AWS SDK Java v2 naming: `<ServiceId><suffix>` (spaces stripped).
            // Try each suffix in order; match if any produces the observed type name.
            // e.g. serviceId "S3" + suffixes ["Client", "AsyncClient"] matches "S3AsyncClient"
            all_services
                .into_iter()
                .filter(|svc| {
                    service_index
                        .services
                        .get(svc)
                        .map(|def| {
                            let base = def.metadata.service_id.replace(' ', "");
                            type_suffixes
                                .iter()
                                .any(|suffix| format!("{base}{suffix}") == type_name)
                        })
                        .unwrap_or(false)
                })
                .collect()
        }
    }
}

/// Extract the AWS service name from a fully-qualified type name.
///
/// Returns `Some(service)` only when `type_name` starts with the canonical
/// `software.amazon.awssdk.services.` prefix — i.e. it is a FQN, not a simple name.
///
/// # Examples
/// - `"software.amazon.awssdk.services.s3.waiters.S3Waiter"` → `Some("s3")`
/// - `"software.amazon.awssdk.services.cloudvault.CloudVaultClient"` → `Some("cloudvault")`
/// - `"S3Client"` → `None`  (simple name — caller must use import lookup)
pub(crate) fn extract_service_from_fqn(type_name: &str) -> Option<String> {
    type_name
        .strip_prefix("software.amazon.awssdk.services.")
        .and_then(|rest| rest.split('.').next())
        .map(std::string::ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(vec!["s3", "s3control"], vec!["s3"], vec!["s3"], "narrows to s3")]
    #[case(vec!["s3", "dynamodb"], vec![], vec!["s3", "dynamodb"], "empty imports pass all")]
    #[case(vec!["s3"], vec!["dynamodb"], vec![], "no match returns empty")]
    #[case(vec!["s3", "dynamodb", "sqs"], vec!["dynamodb", "sqs"], vec!["dynamodb", "sqs"], "partial overlap")]
    fn test_apply_import_filter(
        #[case] services: Vec<&str>,
        #[case] imported: Vec<&str>,
        #[case] expected: Vec<&str>,
        #[case] msg: &str,
    ) {
        let services: Vec<String> = services.into_iter().map(str::to_string).collect();
        let imported: HashSet<String> = imported.into_iter().map(str::to_string).collect();
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        assert_eq!(apply_import_filter(services, &imported), expected, "{msg}");
    }

    #[test]
    fn test_extract_service_from_fqn_qualified() {
        assert_eq!(
            extract_service_from_fqn("software.amazon.awssdk.services.s3.waiters.S3Waiter")
                .as_deref(),
            Some("s3")
        );
        assert_eq!(
            extract_service_from_fqn("software.amazon.awssdk.services.cloudvault.CloudVaultClient")
                .as_deref(),
            Some("cloudvault")
        );
    }

    #[test]
    fn test_extract_service_from_fqn_simple_name() {
        assert_eq!(extract_service_from_fqn("S3Client"), None);
        assert_eq!(extract_service_from_fqn("CloudVaultWaiter"), None);
    }
}
