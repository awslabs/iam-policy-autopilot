//! Normalize a Terraform CRUD-map handler symbol into the model's join key.
//!
//! The CRUD map stores full Go import paths, e.g.
//! `github.com/hashicorp/terraform-provider-aws/internal/service/sqs.(*queueAttributeHandler).Upsert-fm`.
//! The model (`terraform-model.json`) keys its `call_patterns` on
//! `(module_path, class_name, function_name)` — the same triple the model
//! builder derives via `service_symbol` (xtask) + `parse_function_name`
//! (`model_generation::language_conventions`). To join a plan resource's
//! handler to its SDK operations, the consumer must reproduce that triple
//! identically.
//!
//! This module is the single shared implementation of that normalization.
//! The Go runtime decorates handler names in two ways the model builder
//! already accounts for, so we mirror them here:
//! - method value: trailing `-fm`, e.g. `(*queueAttributeHandler).Upsert-fm`
//!   → keep the `(*Type).Method` receiver form, then split into class+method.
//! - closure: trailing `.funcN` (possibly nested), e.g.
//!   `resourceResourcePolicy.resourceResourcePolicyPut.func1` → the innermost
//!   *named* enclosing function (`resourceResourcePolicyPut`).

/// Marker separating the import path prefix from `<package>.<qualifier>`.
const SERVICE_PATH_MARKER: &str = "/internal/service/";

/// A handler's join key into `terraform-model.json`'s `call_patterns`.
///
/// Mirrors `model_generation::language_conventions::ParsedFunctionName` for Go:
/// `module_path` is the service package short name, and methods carry a
/// `class_name` (the receiver type) while free functions do not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HandlerKey {
    pub(crate) module_path: String,
    pub(crate) class_name: Option<String>,
    pub(crate) function_name: String,
}

/// Parse a CRUD-map handler symbol into its model join key.
///
/// Returns `None` for symbols that do not live under `/internal/service/`
/// (the model builder skips these too, so they have no `call_pattern`).
pub(crate) fn handler_key(full_symbol: &str) -> Option<HandlerKey> {
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
    Some(HandlerKey {
        module_path: package.to_string(),
        class_name,
        function_name,
    })
}

/// Strip the Go runtime's closure/method-value decorations from the qualifier
/// (everything after `pkg.`), yielding the resolvable entry point.
///
/// Mirrors `xtask::build_terraform_model::normalize_go_entry_point`.
fn normalize_go_entry_point(qualifier: &str) -> String {
    // Method value: strip `-fm`, keep the `(*Type).Method` form whole.
    if let Some(method) = qualifier.strip_suffix("-fm") {
        return method.to_string();
    }

    // Closure: strip trailing `.funcN` segments, then take the last named
    // segment of the remaining dotted chain.
    let mut q = qualifier;
    let mut had_closure = false;
    while let Some((head, tail)) = q.rsplit_once('.') {
        let is_closure_seg = tail.len() > 4
            && tail.starts_with("func")
            && tail[4..].bytes().all(|b| b.is_ascii_digit());
        if is_closure_seg {
            q = head;
            had_closure = true;
        } else {
            break;
        }
    }
    if had_closure {
        return q.rsplit('.').next().unwrap_or(q).to_string();
    }

    q.to_string()
}

/// Split a normalized entry point into `(class_name, function_name)`.
///
/// Methods take the gopls `(*Type).Method` form; mirrors
/// `GoConventions::parse_function_name`'s receiver handling. Free functions
/// have no class name.
fn split_receiver(entry: &str) -> (Option<String>, String) {
    if let Some(dot_pos) = entry.find(").") {
        let receiver_part = &entry[..=dot_pos];
        let method_name = &entry[dot_pos + 2..];
        let type_name = receiver_part
            .trim_start_matches('(')
            .trim_start_matches('*')
            .trim_end_matches(')');
        return (Some(type_name.to_string()), method_name.to_string());
    }
    (None, entry.to_string())
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
            HandlerKey {
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
