//! [`JavaMethodCallExtractor`] — extracts `receiver.method(args)` calls from Java source files.
//!
//! Uses labeled captures so the process stage can reliably retrieve each component:
//! - `$MC_OBJ` — the receiver expression (e.g. `"s3Client"`)  [discriminator]
//! - `$MC_METHOD` — the method name identifier (e.g. `"putObject"`)
//!
//! Plain function calls without a receiver are excluded by requiring the `object` field.

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{Call, ExtractionResult};
use crate::Location;
use crate::SourceFile;

/// Extracts all `receiver.method(args)` method invocations from a Java source file.
///
/// Uses labeled captures so the process stage can reliably retrieve each component:
/// - `$MC_OBJ` — the receiver expression (e.g. `"s3Client"`)  [discriminator]
/// - `$MC_METHOD` — the method name identifier (e.g. `"putObject"`)
///
/// Plain function calls without a receiver are excluded by requiring the `object` field.
///
/// # Rule body
///
/// ```yaml
/// kind: method_invocation
/// all:
///   - has:
///       field: object
///       pattern: $MC_OBJ
///   - has:
///       field: name
///       pattern: $MC_METHOD
/// ```
pub(crate) struct JavaMethodCallExtractor;

impl SdkExtractor for JavaMethodCallExtractor {
    fn rule_yaml(&self) -> &'static str {
        "kind: method_invocation\nall:\n  - has:\n      field: object\n      pattern: $MC_OBJ\n  - has:\n      field: name\n      pattern: $MC_METHOD"
    }

    fn discriminator_label(&self) -> &'static str {
        "MC_OBJ"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let env = node_match.get_env();

        let receiver = env.get_match("MC_OBJ").map(|n| n.text().to_string());

        let method = match env.get_match("MC_METHOD") {
            Some(n) => n.text().to_string(),
            None => return,
        };

        let node = node_match.get_node();
        let parameters = utils::extract_arguments_from_node(node);
        let location = Location::from_node(source_file.path.clone(), node);
        let expr = node.text().to_string();

        // Attempt to find the receiver declaration for simple identifier receivers (Tier 1).
        // Field-access and method-invocation receivers (Tier 2/3) cannot be resolved without
        // a type checker, so receiver_declaration is left as None for those cases.
        let receiver_declaration = match env.get_match("MC_OBJ") {
            Some(receiver_node) if receiver_node.kind().as_ref() == utils::IDENTIFIER => {
                let receiver_name = receiver_node.text().to_string();
                utils::find_receiver_declaration(node, &receiver_name, source_file)
            }
            _ => None,
        };

        result.calls.push(Call {
            expr,
            method,
            receiver,
            parameters,
            location,
            receiver_declaration,
        });
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::extraction::java::extractors::utils::resolve_java_literal;
    use crate::extraction::ParameterValue;
    use crate::java_extractor_test;
    use rstest::rstest;

    java_extractor_test!(
        "tests/java/extractors/methods/*.java",
        crate::extraction::java::types::Call,
        calls
    );

    // ── resolve_java_literal unit tests (rstest) ──────────────────────────────

    /// Helper: parse `expr` as a Java expression, find the first node of `expected_kind`
    /// whose text equals `node_text`, and return the result of `resolve_java_literal`.
    ///
    /// `node_text` is the raw text as it appears in the source (e.g. `"\"my-bucket\""` for a
    /// string literal, `"42"` for an integer, `"myVar"` for an identifier).
    fn resolve_expr(expr: &str, expected_kind: &str, node_text: &str) -> ParameterValue {
        use ast_grep_core::tree_sitter::LanguageExt;
        use ast_grep_language::Java;

        let src = format!("class T{{ void r(){{ Object x={expr}; }} }}");
        let sg = Java.ast_grep(&src);
        let root = sg.root();

        fn find<'a>(
            node: ast_grep_core::Node<
                'a,
                ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>,
            >,
            kind: &str,
            text: &str,
        ) -> Option<
            ast_grep_core::Node<'a, ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
        > {
            if node.kind().as_ref() == kind && node.text() == text {
                return Some(node);
            }
            for child in node.children() {
                if let Some(n) = find(child, kind, text) {
                    return Some(n);
                }
            }
            None
        }

        let node = find(root, expected_kind, node_text).unwrap_or_else(|| {
            panic!("no {expected_kind} node with text {node_text:?} found in: {src}")
        });
        resolve_java_literal(&node)
    }

    /// String literals are `Resolved` with surrounding quotes stripped.
    ///
    /// `node_text` is the raw source text including quotes (e.g. `"\"my-bucket\""`).
    #[rstest]
    #[case("\"my-bucket\"", "string_literal", "\"my-bucket\"", "my-bucket")]
    #[case("\"\"", "string_literal", "\"\"", "")]
    #[case("\"hello world\"", "string_literal", "\"hello world\"", "hello world")]
    fn test_string_literal_resolved(
        #[case] expr: &str,
        #[case] kind: &str,
        #[case] node_text: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            resolve_expr(expr, kind, node_text),
            ParameterValue::Resolved(expected.to_string()),
        );
    }

    /// Numeric, boolean, and null literals are `Resolved` as-is.
    #[rstest]
    #[case("42", "decimal_integer_literal", "42", "42")]
    #[case("3.14f", "decimal_floating_point_literal", "3.14f", "3.14f")]
    #[case("0xFF", "hex_integer_literal", "0xFF", "0xFF")]
    #[case("0755", "octal_integer_literal", "0755", "0755")]
    #[case("0b1010", "binary_integer_literal", "0b1010", "0b1010")]
    #[case("true", "true", "true", "true")]
    #[case("false", "false", "false", "false")]
    #[case("null", "null_literal", "null", "null")]
    fn test_non_string_literal_resolved(
        #[case] expr: &str,
        #[case] kind: &str,
        #[case] node_text: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            resolve_expr(expr, kind, node_text),
            ParameterValue::Resolved(expected.to_string()),
        );
    }

    /// Identifiers are `Unresolved`.
    #[rstest]
    #[case("myVar", "identifier", "myVar", "myVar")]
    #[case("bucketName", "identifier", "bucketName", "bucketName")]
    fn test_identifier_unresolved(
        #[case] expr: &str,
        #[case] kind: &str,
        #[case] node_text: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            resolve_expr(expr, kind, node_text),
            ParameterValue::Unresolved(expected.to_string()),
        );
    }
}
