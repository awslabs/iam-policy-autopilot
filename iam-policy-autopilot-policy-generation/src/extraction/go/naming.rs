//! General Go naming conventions: how Go spells function symbols, and how to
//! normalize the decorations the Go runtime / reflection add to them.
//!
//! This is the always-compiled string layer shared by everything that has to
//! interpret a Go function name:
//! - the model builder's `GoConventions` (gated behind `model-generation`), which
//!   turns gopls call-graph nodes into model keys, and
//! - the Terraform plan consumer (always compiled), which turns CRUD-map handler
//!   symbols into the same keys.
//!
//! Nothing here is Terraform-specific — the `internal/service/` layout lives with
//! the Terraform code. These are the general Go rules: a method node is spelled
//! `(*Type).Method`, and reflection decorates method values with a trailing `-fm`
//! and closures with a trailing `.funcN`.

/// Split a Go function/method spelling into `(class_name, function_name)`.
///
/// gopls spells a method `(*Type).Method` (or `(Type).Method`); a free function
/// is just its name. Methods yield `class_name = Some("Type")`; free functions
/// yield `None`.
pub(crate) fn split_receiver(entry: &str) -> (Option<String>, String) {
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

/// Strip the Go runtime's closure / method-value decorations from a qualifier
/// (the part after `pkg.`), yielding the resolvable entry point that gopls has a
/// node for and whose body holds the SDK calls.
///
/// The Go runtime (via `runtime.FuncForPC`, as reflection sees it) decorates
/// handler function names in two ways:
/// - **method value**: trailing `-fm`, e.g. `(*queueAttributeHandler).Upsert-fm`
///   → keep the `(*Type).Method` receiver form whole.
/// - **closure**: trailing `.funcN` (possibly nested), prefixed by the enclosing
///   function chain, e.g. `resourceResourcePolicy.resourceResourcePolicyPut.func1`
///   → the innermost *named* enclosing function (`resourceResourcePolicyPut`).
pub(crate) fn normalize_go_entry_point(qualifier: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    // Free function: no receiver.
    #[case("resourceBucketRead", None, "resourceBucketRead")]
    // Pointer receiver.
    #[case(
        "(*queueAttributeHandler).Upsert",
        Some("queueAttributeHandler"),
        "Upsert"
    )]
    // Value receiver.
    #[case("(Server).Method", Some("Server"), "Method")]
    fn split_receiver_separates_class_and_method(
        #[case] entry: &str,
        #[case] class: Option<&str>,
        #[case] func: &str,
    ) {
        assert_eq!(
            split_receiver(entry),
            (class.map(str::to_string), func.to_string())
        );
    }

    #[rstest]
    // Plain free function: unchanged.
    #[case("resourceBucketRead", "resourceBucketRead")]
    // Method value: `-fm` stripped, receiver form kept whole.
    #[case("(*queueAttributeHandler).Read-fm", "(*queueAttributeHandler).Read")]
    // Closure: `.funcN` stripped to the enclosing named function.
    #[case("resourceResourcePolicyPut.func1", "resourceResourcePolicyPut")]
    // Nested closure chain: still resolves to the innermost named function.
    #[case(
        "resourceResourcePolicy.resourceResourcePolicyPut.func2",
        "resourceResourcePolicyPut"
    )]
    fn normalize_strips_runtime_decorations(#[case] qualifier: &str, #[case] expected: &str) {
        assert_eq!(normalize_go_entry_point(qualifier), expected);
    }
}
