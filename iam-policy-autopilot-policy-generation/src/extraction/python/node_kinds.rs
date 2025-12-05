//! Tree-sitter node kind constants for Python AST
//!
//! These constants represent the node kinds returned by Tree-sitter's Python grammar.
//! Using constants instead of string literals provides:
//! - Compile-time checking of constant names
//! - IDE autocomplete support
//! - Centralized documentation of node kinds
//! - Easier refactoring
//!
//! Note: The actual values come from the Tree-sitter Python grammar and cannot be
//! changed. We're just providing named constants to avoid magic strings.
//!
//! TODO: Add automated linting to enforce usage of these constants instead of
//! string literals. See: https://github.com/awslabs/iam-policy-autopilot/issues/60

/// A comment node
pub(crate) const COMMENT: &str = "comment";

/// A keyword argument in a function call (e.g., `key=value`)
pub(crate) const KEYWORD_ARGUMENT: &str = "keyword_argument";

/// A dictionary splat/unpacking operator (e.g., `**kwargs`)
pub(crate) const DICTIONARY_SPLAT: &str = "dictionary_splat";
