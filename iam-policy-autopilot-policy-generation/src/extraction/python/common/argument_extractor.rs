//! Common argument extraction utilities for Python AST processing
//!
//! This module provides standardized ways to extract and parse arguments
//! from ast-grep nodes, handling keyword arguments, positional arguments,
//! and dictionary unpacking consistently across all Python extractors.

use crate::extraction::python::node_kinds;
use crate::extraction::{Parameter, ParameterValue};
use ast_grep_language::Python;

/// Utility for extracting arguments from Python AST nodes
pub struct ArgumentExtractor;

impl ArgumentExtractor {
    /// Extract arguments from argument nodes
    pub fn extract_arguments(
        args_nodes: &[ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>],
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();
        let mut parameter_position = 0; // Track actual parameter position (excluding commas)

        for arg_node in args_nodes {
            // Filter out comment nodes
            if arg_node.kind() == node_kinds::COMMENT {
                continue;
            }

            let arg_text = arg_node.text().to_string();

            // Filter out comma nodes - ast-grep captures commas as separate nodes
            if arg_text.trim() == "," {
                continue;
            }

            // Check if this is a keyword argument
            if Self::is_keyword_argument(arg_node) {
                if let Some(param) = Self::parse_keyword_argument(arg_node, parameter_position) {
                    parameters.push(param);
                    parameter_position += 1;
                }
            }
            // Check if this is dictionary unpacking (**kwargs)
            else if Self::is_dictionary_splat(arg_node) {
                parameters.push(Parameter::DictionarySplat {
                    expression: arg_text,
                    position: parameter_position,
                });
                parameter_position += 1;
            }
            // Otherwise, it's a positional argument
            else {
                parameters.push(Parameter::Positional {
                    value: Self::extract_parameter_value(&arg_text),
                    position: parameter_position,
                    type_annotation: None,
                    struct_fields: None,
                });
                parameter_position += 1;
            }
        }

        parameters
    }

    /// Check if a node represents a keyword argument
    pub fn is_keyword_argument(
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> bool {
        node.kind() == node_kinds::KEYWORD_ARGUMENT
    }

    /// Check if a node represents dictionary unpacking (**kwargs)
    pub fn is_dictionary_splat(
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    ) -> bool {
        node.kind() == node_kinds::DICTIONARY_SPLAT
    }

    /// Parse a keyword argument node
    pub fn parse_keyword_argument(
        node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
        position: usize,
    ) -> Option<Parameter> {
        let full_text = node.text();

        if let Some(eq_pos) = full_text.find('=') {
            let name = full_text[..eq_pos].trim().to_string();
            let value_text = full_text[eq_pos + 1..].trim();
            let value = Self::extract_parameter_value(value_text);

            Some(Parameter::Keyword {
                name,
                value,
                position,
                type_annotation: None,
            })
        } else {
            None
        }
    }

    /// Extract a parameter value, distinguishing between resolved literals and unresolved identifiers
    pub fn extract_parameter_value(text: &str) -> ParameterValue {
        let trimmed = text.trim();

        // Handle quoted strings - these are resolved literals
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            ParameterValue::Resolved(trimmed[1..trimmed.len() - 1].to_string())
        } else {
            // Handle unresolved identifiers, method calls, attribute access, etc.
            ParameterValue::Unresolved(trimmed.to_string())
        }
    }

    /// Extract a quoted string, handling both single and double quotes (backward compatibility)
    pub fn extract_quoted_string(text: &str) -> Option<String> {
        let trimmed = text.trim();

        // Handle quoted strings
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            Some(trimmed[1..trimmed.len() - 1].to_string())
        } else {
            // Handle unquoted identifiers (less common but possible)
            Some(trimmed.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_quoted_string() {
        // Test different quote styles
        assert_eq!(
            ArgumentExtractor::extract_quoted_string("'list_objects_v2'"),
            Some("list_objects_v2".to_string())
        );
        assert_eq!(
            ArgumentExtractor::extract_quoted_string("\"list_objects_v2\""),
            Some("list_objects_v2".to_string())
        );
        assert_eq!(
            ArgumentExtractor::extract_quoted_string(" 'list_objects_v2' "),
            Some("list_objects_v2".to_string())
        );

        // Test unquoted (edge case)
        assert_eq!(
            ArgumentExtractor::extract_quoted_string("operation_name"),
            Some("operation_name".to_string())
        );
    }
}
