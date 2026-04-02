//! [`JavaPaginatorExtractor`] — extracts paginator method calls from Java source files.

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{ExtractionResult, Paginator};
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
        r"kind: method_invocation
all:
  - has:
      field: object
      pattern: $PAG_CLIENT
  - has:
      field: name
      regex: 'Paginator$'
      pattern: $PAGINATOR_METHOD"
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
        let paginator_method = match node_match.get_env().get_match("PAGINATOR_METHOD") {
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
        let parameters = utils::extract_arguments_from_node(node);

        let receiver_declaration =
            utils::find_receiver_declaration_from_env(node_match, "PAG_CLIENT", source_file);

        result.paginators.push(Paginator {
            expr,
            operation,
            parameters,
            location,
            receiver_declaration,
        });
    }
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
