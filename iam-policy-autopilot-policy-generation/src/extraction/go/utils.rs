//! Shared utilities for Go AWS SDK extraction

use crate::extraction::go::extractor::StructField;
use crate::extraction::Parameter;
use ast_grep_language::Go;

/// Extract arguments from argument nodes
pub(crate) fn extract_arguments(
    args_nodes: &[ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>],
) -> Vec<Parameter> {
    let mut parameters = Vec::new();
    let mut position = 0;

    for arg_node in args_nodes {
        let arg_text = arg_node.text().to_string();

        // Skip parsing artifacts (commas, whitespace, etc.)
        if is_parsing_artifact(&arg_text) {
            continue;
        }

        // Check if this is a struct literal (&Type{...})
        if is_struct_literal(arg_node) {
            if let Some(param) = parse_struct_literal(arg_node, position) {
                parameters.push(param);
                position += 1;
            }
        }
        // Otherwise, it's a general expression
        else {
            parameters.push(Parameter::expression(arg_text, position));
            position += 1;
        }
    }

    parameters
}

/// Check if a text is a parsing artifact that should be ignored
fn is_parsing_artifact(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed == "," || trimmed == "(" || trimmed == ")"
}

/// Check if a node represents a struct literal (&Type{...})
pub(crate) fn is_struct_literal(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
) -> bool {
    let text = node.text();
    let trimmed = text.trim();
    trimmed.starts_with('&') && trimmed.contains('{') && trimmed.ends_with('}')
}

/// Parse a struct literal node
pub(crate) fn parse_struct_literal(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Go>>,
    position: usize,
) -> Option<Parameter> {
    let text = node.text();

    // Extract type name from &TypeName{...}
    let type_start = usize::from(text.starts_with('&'));
    let brace_pos = text.find('{')?;
    let type_name = text[type_start..brace_pos].trim().to_string();

    // Extract fields from the struct literal
    let fields_text = &text[brace_pos + 1..text.len() - 1];
    let fields = parse_struct_fields(fields_text);

    Some(Parameter::struct_literal(type_name, fields, position))
}

/// Parse struct fields from the content between braces
pub(crate) fn parse_struct_fields(fields_text: &str) -> Vec<StructField> {
    let mut fields = Vec::new();

    for field_part in fields_text.split(',') {
        let field_part = field_part.trim();
        if field_part.is_empty() {
            continue;
        }

        if let Some(colon_pos) = field_part.find(':') {
            let field_name = field_part[..colon_pos].trim().to_string();
            let field_value = field_part[colon_pos + 1..].trim().to_string();

            fields.push(StructField {
                name: field_name,
                value: field_value,
            });
        }
    }

    fields
}
