//! [`JavaPaginatorExtractor`] — extracts paginator method calls from Java source files.

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{ExtractionResult, Paginator};
use crate::extraction::MethodCallResultUsage;
use crate::Location;
use crate::SourceFile;

/// Extracts paginator method calls from a Java source file.
///
/// Matches methods ending with `"Paginator"` and strips the suffix to derive the
/// base operation name (e.g. `listObjectsV2Paginator` → `listObjectsV2`).
///
/// When the receiver is a plain identifier, performs a scope walk to find the receiver
/// variable's declaration and stores it in [`Paginator::receiver_declaration`].
///
/// # Rule body
///
/// ```yaml
/// kind: method_invocation
/// all:
///   - has:
///       field: object
///       pattern: $PAG_CLIENT
///   - has:
///       field: name
///       regex: 'Paginator$'
///       pattern: $PAGINATOR_METHOD
/// ```
///
/// The label `$PAGINATOR_METHOD` is the discriminator (captures the `*Paginator` method name).
pub(crate) struct JavaPaginatorExtractor;

impl SdkExtractor for JavaPaginatorExtractor {
    fn rule_yaml(&self) -> &'static str {
        "kind: method_invocation\nall:\n  - has:\n      field: object\n      pattern: $PAG_CLIENT\n  - has:\n      field: name\n      regex: 'Paginator$'\n      pattern: $PAGINATOR_METHOD"
    }

    fn discriminator_label(&self) -> &'static str {
        "PAGINATOR_METHOD"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let env = node_match.get_env();

        let paginator_method = match env.get_match("PAGINATOR_METHOD") {
            Some(n) => n.text().to_string(),
            None => return,
        };

        // Strip the "Paginator" suffix to get the base operation name
        let operation = paginator_method
            .strip_suffix("Paginator")
            .unwrap_or(&paginator_method)
            .to_string();

        let node = node_match.get_node();
        let location = Location::from_node(source_file.path.clone(), node);
        let expr = node.text().to_string();
        let usage = detect_assignment_usage(node);
        let parameters = utils::extract_arguments_from_node(node);

        // Attempt to find the receiver declaration for simple identifier receivers (Tier 1).
        // Field-access and method-invocation receivers (Tier 2/3) cannot be resolved without
        // a type checker, so receiver_declaration is left as None for those cases.
        let receiver_declaration = match env.get_match("PAG_CLIENT") {
            Some(receiver_node) if receiver_node.kind().as_ref() == utils::IDENTIFIER => {
                let receiver_name = receiver_node.text().to_string();
                utils::find_receiver_declaration(node, &receiver_name, source_file)
            }
            _ => None,
        };

        result.paginators.push(Paginator {
            expr,
            operation,
            parameters,
            usage,
            location,
            receiver_declaration,
        });
    }
}

/// Detect whether the paginator call result is assigned to a variable.
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
    use rstest::rstest;

    java_extractor_test!(
        "tests/java/extractors/paginators/*.java",
        crate::extraction::java::types::Paginator,
        paginators
    );

    /// Parameterized test: stripping the `Paginator` suffix to derive the base operation name.
    /// This is a pure string transformation test — no source file needed.
    #[rstest]
    #[case("listObjectsV2Paginator", "listObjectsV2")]
    #[case("scanPaginator", "scan")]
    #[case("queryPaginator", "query")]
    #[case("listTablesPaginator", "listTables")]
    #[case("describeInstancesPaginator", "describeInstances")]
    fn test_strip_paginator_suffix(#[case] input: &str, #[case] expected: &str) {
        let result = input.strip_suffix("Paginator").unwrap_or(input);
        assert_eq!(result, expected, "failed for '{input}'");
    }
}
