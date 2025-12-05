//! Tree-sitter node kind constants for Go AST
//!
//! These constants represent the node kinds returned by Tree-sitter's Go grammar.
//! Using constants instead of string literals provides:
//! - Compile-time checking of constant names
//! - IDE autocomplete support
//! - Centralized documentation of node kinds
//! - Easier refactoring
//!
//! Note: The actual values come from the Tree-sitter Go grammar and cannot be
//! changed. We're just providing named constants to avoid magic strings.
//!
//! TODO: Add automated linting to enforce usage of these constants instead of
//! string literals. See: https://github.com/awslabs/iam-policy-autopilot/issues/60

/// A composite literal node (e.g., `Type{field: value}`)
pub(crate) const COMPOSITE_LITERAL: &str = "composite_literal";

/// A unary expression node (e.g., `&value`, `*ptr`)
pub(crate) const UNARY_EXPRESSION: &str = "unary_expression";

/// A literal value node containing struct field assignments
pub(crate) const LITERAL_VALUE: &str = "literal_value";

/// A keyed element in a composite literal (e.g., `field: value`)
pub(crate) const KEYED_ELEMENT: &str = "keyed_element";

/// A literal element representing a field name or simple value
pub(crate) const LITERAL_ELEMENT: &str = "literal_element";

/// An argument list node containing function/method arguments
/// Note: Currently only used in YAML pattern strings, not in Rust code comparisons
#[allow(dead_code)]
pub(crate) const ARGUMENT_LIST: &str = "argument_list";

/// Left parenthesis token
pub(crate) const LEFT_PAREN: &str = "(";

/// Right parenthesis token
pub(crate) const RIGHT_PAREN: &str = ")";

/// Comma separator token
pub(crate) const COMMA: &str = ",";
