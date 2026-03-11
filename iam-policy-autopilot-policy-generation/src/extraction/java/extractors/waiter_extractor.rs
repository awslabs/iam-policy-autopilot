//! [`JavaWaiterCallExtractor`] — extracts `waitUntil*` calls from Java source files.
//!
//! The extractor captures each `waitUntil*` invocation as a [`Waiter`]. When the receiver
//! is a plain identifier (Tier 1), it performs a scope walk to find the receiver variable's
//! declaration (local var declaration or formal parameter) and stores it in
//! [`Waiter::receiver_declaration`].
//!
//! The scope-walk logic is shared with the paginator and method-call extractors via
//! [`utils::find_receiver_declaration`].

use convert_case::{Case, Casing};

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{ExtractionResult, Waiter};
use crate::extraction::MethodCallResultUsage;
use crate::Location;
use crate::SourceFile;

// ================================================================================================
// JavaWaiterCallExtractor
// ================================================================================================

/// Extracts `waitUntil*` calls (e.g. `waiter.waitUntilBucketExists(request)`).
///
/// When the receiver is a plain identifier, performs a scope walk to find the receiver
/// variable's declaration and stores it in [`Waiter::receiver_declaration`].
///
/// # Rule body
///
/// ```yaml
/// kind: method_invocation
/// all:
///   - has:
///       field: object
///       pattern: $WAITER_OBJ
///   - has:
///       field: name
///       regex: '^waitUntil'
///       pattern: $WAIT_METHOD
/// ```
///
/// The label `$WAIT_METHOD` is the discriminator (captures the `waitUntil*` method name).
pub(crate) struct JavaWaiterCallExtractor;

impl SdkExtractor for JavaWaiterCallExtractor {
    fn rule_yaml(&self) -> &'static str {
        "kind: method_invocation\nall:\n  - has:\n      field: object\n      pattern: $WAITER_OBJ\n  - has:\n      field: name\n      regex: '^waitUntil'\n      pattern: $WAIT_METHOD"
    }

    fn discriminator_label(&self) -> &'static str {
        "WAIT_METHOD"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let env = node_match.get_env();

        let full_method = match env.get_match("WAIT_METHOD") {
            Some(n) => n.text().to_string(),
            None => return,
        };

        // Strip the "waitUntil" prefix and convert to camelCase so that waiter_type
        // matches the waiter_lookup index keys produced by ServiceDiscovery
        // (e.g. "waitUntilBucketExists" → strip → "BucketExists" → camel → "bucketExists").
        let waiter_type = full_method
            .strip_prefix("waitUntil")
            .unwrap_or(&full_method)
            .to_case(Case::Camel);

        let node = node_match.get_node();
        let location = Location::from_node(source_file.path.clone(), node);
        let expr = node.text().to_string();
        let usage = detect_assignment_usage(node);

        let parameters = utils::extract_arguments_from_node(node);

        // Attempt to find the receiver declaration for simple identifier receivers (Tier 1).
        // Field-access and method-invocation receivers (Tier 2/3) cannot be resolved without
        // a type checker, so receiver_declaration is left as None for those cases.
        let receiver_declaration = match env.get_match("WAITER_OBJ") {
            Some(receiver_node) if receiver_node.kind().as_ref() == utils::IDENTIFIER => {
                let receiver_name = receiver_node.text().to_string();
                utils::find_receiver_declaration(node, &receiver_name, source_file)
            }
            _ => None,
        };

        result.waiters.push(Waiter {
            expr,
            waiter_type,
            parameters,
            usage,
            location,
            receiver_declaration,
        });
    }
}

// ================================================================================================
// Shared helpers
// ================================================================================================

/// Detect whether the waiter call result is assigned to a variable.
///
/// The Java AST shape for a local variable assignment is always:
/// ```text
/// local_variable_declaration          (grandparent)
///   └── variable_declarator           (parent)
///         └── <expr>                  (the matched node)
/// ```
/// We walk exactly these two levels rather than using a hop-counting loop.
fn detect_assignment_usage(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
) -> Option<MethodCallResultUsage> {
    // Level 1: the immediate parent must be `variable_declarator`.
    let variable_declarator = node
        .parent()
        .filter(|p| p.kind().as_ref() == utils::VARIABLE_DECLARATOR)?;

    // The first child of `variable_declarator` is the declared variable name.
    let variable_name = variable_declarator
        .children()
        .next()
        .map(|n| n.text().to_string())
        .unwrap_or_default();

    // Level 2: the grandparent must be `local_variable_declaration`.
    // Its first child is the declared type.
    let type_name = variable_declarator
        .parent()
        .filter(|gp| gp.kind().as_ref() == utils::LOCAL_VARIABLE_DECLARATION)
        .and_then(|gp| gp.children().next().map(|n| n.text().to_string()));

    Some(MethodCallResultUsage::Assigned {
        variable_name,
        type_name,
    })
}

#[cfg(test)]
mod tests {
    use crate::java_extractor_test;

    java_extractor_test!(
        "tests/java/extractors/waiters/*.java",
        crate::extraction::java::types::Waiter,
        waiters
    );
}
