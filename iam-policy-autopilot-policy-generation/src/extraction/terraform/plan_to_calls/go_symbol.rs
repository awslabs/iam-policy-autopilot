//! Normalize a Terraform CRUD-map handler symbol into the model's join key.
//!
//! The CRUD map stores full Go import paths, e.g.
//! `github.com/hashicorp/terraform-provider-aws/internal/service/sqs.(*queueAttributeHandler).Upsert-fm`.
//! The model (`terraform-model.json`) keys its `call_patterns` on the
//! [`CallPatternKey`] triple `(module_path, class_name, function_name)` — the
//! same triple the model builder derives from a call-graph node. To join a plan
//! resource's handler to its SDK operations, the consumer must reproduce that
//! triple identically.
//!
//! This module owns only the Terraform-specific part of that join: peeling the
//! `internal/service/<pkg>` prefix to recover `module_path`. The general-Go
//! string normalization (stripping the runtime's `-fm`/`.funcN` decorations,
//! splitting a `(*Type).Method` receiver) is shared with the model builder via
//! [`crate::extraction::go::naming`], so both sides normalize identically.

use crate::extraction::external_library_models::CallPatternKey;
use crate::extraction::go::naming::{normalize_go_entry_point, split_receiver};

/// Marker separating the import path prefix from `<package>.<qualifier>`.
const SERVICE_PATH_MARKER: &str = "/internal/service/";

/// Parse a CRUD-map handler symbol into its model join key ([`CallPatternKey`]).
///
/// Returns `None` for symbols that do not live under `/internal/service/`
/// (the model builder skips these too, so they have no `call_pattern`).
pub(crate) fn handler_key(full_symbol: &str) -> Option<CallPatternKey> {
    let after = full_symbol.split_once(SERVICE_PATH_MARKER)?.1;

    // package = first path-or-dot-delimited segment after the marker.
    let pkg_end = after.find(['/', '.'])?;
    let package = &after[..pkg_end];
    // The qualifier must be dot-separated from the package (a '/' would be a
    // sub-package path we do not model).
    if after.as_bytes().get(pkg_end) != Some(&b'.') {
        return None;
    }
    let qualifier = &after[pkg_end + 1..];
    if package.is_empty() || qualifier.is_empty() {
        return None;
    }

    let entry = normalize_go_entry_point(qualifier);
    if entry.is_empty() {
        return None;
    }

    let (class_name, function_name) = split_receiver(&entry);
    Some(CallPatternKey {
        module_path: package.to_string(),
        class_name,
        function_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const PFX: &str = "github.com/hashicorp/terraform-provider-aws/internal/service/";

    #[rstest]
    // Plain free function.
    #[case(
        "accessanalyzer.resourceAnalyzerCreate",
        "accessanalyzer",
        None,
        "resourceAnalyzerCreate"
    )]
    // Method value handler (sqs): receiver split into class + method, `-fm` stripped.
    #[case(
        "sqs.(*queueAttributeHandler).Upsert-fm",
        "sqs",
        Some("queueAttributeHandler"),
        "Upsert"
    )]
    #[case(
        "sqs.(*queueAttributeHandler).Read-fm",
        "sqs",
        Some("queueAttributeHandler"),
        "Read"
    )]
    // Closure handler (glue): innermost named enclosing function, `.funcN` stripped.
    #[case(
        "glue.resourceResourcePolicy.resourceResourcePolicyPut.func1",
        "glue",
        None,
        "resourceResourcePolicyPut"
    )]
    // Transparent-tagging entry points: the service package's ListTags/UpdateTags
    // methods. Same `(*Type).Method` shape as the sqs case, so the receiver
    // splits to class `servicePackage` + the method name.
    #[case(
        "s3.(*servicePackage).ListTags",
        "s3",
        Some("servicePackage"),
        "ListTags"
    )]
    #[case(
        "lambda.(*servicePackage).UpdateTags",
        "lambda",
        Some("servicePackage"),
        "UpdateTags"
    )]
    fn handler_key_matches_model_join_triple(
        #[case] suffix: &str,
        #[case] module_path: &str,
        #[case] class_name: Option<&str>,
        #[case] function_name: &str,
    ) {
        let key = handler_key(&format!("{PFX}{suffix}")).unwrap();
        assert_eq!(
            key,
            CallPatternKey {
                module_path: module_path.to_string(),
                class_name: class_name.map(str::to_string),
                function_name: function_name.to_string(),
            }
        );
    }

    #[rstest]
    #[case("github.com/hashicorp/terraform-provider-aws/internal/provider.something")]
    #[case("some/other/path.func")]
    fn handler_key_rejects_non_service_symbols(#[case] symbol: &str) {
        assert_eq!(handler_key(symbol), None);
    }
}
