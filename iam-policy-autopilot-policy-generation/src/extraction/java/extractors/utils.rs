//! Shared utilities for Java AST extractors.
//!
//! This module provides three kinds of shared helpers used across all Java extractor
//! implementations:
//!
//! 1. **Node kind constants** — named constants for Tree-sitter Java grammar node kinds,
//!    replacing magic string literals and providing compile-time safety.
//!
//! 2. **Scope-walk helpers** — AST traversal utilities for resolving receiver variable
//!    declarations, shared by [`JavaWaiterCallExtractor`], [`JavaPaginatorExtractor`], and
//!    [`JavaMethodCallExtractor`]. Results are stored as [`ReceiverDeclaration`] values.
//!
//! 3. **Argument extraction helpers** — [`resolve_java_literal`] and
//!    [`extract_arguments_from_node`], shared by all extractors that parse method arguments.
//!
//! [`JavaWaiterCallExtractor`]: super::waiter_extractor::JavaWaiterCallExtractor
//! [`JavaPaginatorExtractor`]: super::paginator_extractor::JavaPaginatorExtractor
//! [`JavaMethodCallExtractor`]: super::method_extractor::JavaMethodCallExtractor

use crate::extraction::java::types::ReceiverDeclaration;
use crate::extraction::{Parameter, ParameterValue};
use crate::Location;
use crate::SourceFile;

// ================================================================================================
// Node kind constants
// ================================================================================================
//
// These constants represent the node kinds returned by Tree-sitter's Java grammar.
// Using constants instead of string literals provides:
// - Compile-time checking of constant names
// - IDE autocomplete support
// - Centralized documentation of node kinds
// - Easier refactoring
//
// Note: The actual values come from the Tree-sitter Java grammar and cannot be
// changed. We're just providing named constants to avoid magic strings.

/// An identifier node (e.g., a variable name or class name)
pub(crate) const IDENTIFIER: &str = "identifier";

/// An argument list node containing method call arguments
pub(crate) const ARGUMENT_LIST: &str = "argument_list";

/// A local variable declaration node (e.g., `S3Client s3 = S3Client.create()`)
pub(crate) const LOCAL_VARIABLE_DECLARATION: &str = "local_variable_declaration";

/// A variable declarator node (e.g., `s3 = S3Client.create()`)
pub(crate) const VARIABLE_DECLARATOR: &str = "variable_declarator";

/// A block node (e.g., `{ ... }`)
pub(crate) const BLOCK: &str = "block";

/// A method declaration node
pub(crate) const METHOD_DECLARATION: &str = "method_declaration";

/// A constructor declaration node
pub(crate) const CONSTRUCTOR_DECLARATION: &str = "constructor_declaration";

/// A formal parameters node (the parameter list of a method/constructor)
pub(crate) const FORMAL_PARAMETERS: &str = "formal_parameters";

/// A formal parameter node (a single parameter in a method/constructor signature)
pub(crate) const FORMAL_PARAMETER: &str = "formal_parameter";

/// A class body node (the `{ ... }` body of a class declaration)
pub(crate) const CLASS_BODY: &str = "class_body";

/// A field declaration node (e.g., `private final S3Client s3 = S3Client.create();`)
pub(crate) const FIELD_DECLARATION: &str = "field_declaration";

/// A lambda expression node (e.g., `x -> x.foo()` or `(S3Client s3) -> s3.putObject(...)`)
pub(crate) const LAMBDA_EXPRESSION: &str = "lambda_expression";

/// An inferred parameters node — the parenthesised parameter list of a lambda with no type
/// annotations, e.g. `(x, y)` in `(x, y) -> x + y`.
pub(crate) const INFERRED_PARAMETERS: &str = "inferred_parameters";

/// A modifiers node (e.g., `private final`, `public static`)
pub(crate) const MODIFIERS: &str = "modifiers";

/// A `try`-with-resources statement node (distinct from a plain `try` statement in the
/// tree-sitter Java grammar)
pub(crate) const TRY_WITH_RESOURCES_STATEMENT: &str = "try_with_resources_statement";

/// A resource specification node — the `(...)` clause of a `try`-with-resources statement
pub(crate) const RESOURCE_SPECIFICATION: &str = "resource_specification";

/// A single resource node inside a `resource_specification`
/// (e.g., `S3Client s3 = S3Client.create()`)
pub(crate) const RESOURCE: &str = "resource";

/// A record declaration node (Java 16+).
/// The record's components live in a `formal_parameters` child of this node,
/// and the record body is a `class_body` child (same node kind as a regular class body).
pub(crate) const RECORD_DECLARATION: &str = "record_declaration";

/// A compact constructor declaration node (Java 16+).
/// Only valid inside a `record_declaration`.  Unlike a regular constructor, it has no
/// `formal_parameters` child — the record components are the implicit parameters.
/// The scope-walk must recognise this node kind so it continues upward to `class_body`
/// (the record body) and then to `record_declaration` to find the component declarations.
pub(crate) const COMPACT_CONSTRUCTOR_DECLARATION: &str = "compact_constructor_declaration";

/// An `if` statement node (e.g., `if (client instanceof S3Client s3) { ... }`).
/// The scope-walk checks the condition of an `if_statement` for an `instanceof_expression`
/// that introduces a binding variable in scope for the then-branch.
pub(crate) const IF_STATEMENT: &str = "if_statement";

/// A parenthesized expression node — the `(...)` condition of an `if_statement`.
/// In the tree-sitter Java grammar the `if` condition is always wrapped in a
/// `parenthesized_expression` node, so the `instanceof_expression` is a child of this
/// node rather than a direct child of `if_statement`.
pub(crate) const PARENTHESIZED_EXPRESSION: &str = "parenthesized_expression";

/// An `instanceof` expression node (Java 16+ pattern form:
/// `client instanceof S3Client s3`).
///
/// In the version of the tree-sitter Java grammar used here, the binding variable is a
/// **direct child** of `instanceof_expression` — there is no intermediate `type_pattern`
/// wrapper.  The children are:
///   - `identifier`       — left operand (e.g. `client`)
///   - `instanceof`       — keyword token
///   - `type_identifier`  — the declared type (e.g. `S3Client`)
///   - `identifier`       — the binding variable name (e.g. `s3`)
pub(crate) const INSTANCEOF_EXPRESSION: &str = "instanceof_expression";

/// A `type_identifier` node — the name of a reference type (e.g. `S3Client`).
/// Used when extracting the declared type from an `instanceof_expression` pattern.
pub(crate) const TYPE_IDENTIFIER: &str = "type_identifier";

/// Left parenthesis token
pub(crate) const LEFT_PAREN: &str = "(";

/// Right parenthesis token
pub(crate) const RIGHT_PAREN: &str = ")";

/// Comma separator token
pub(crate) const COMMA: &str = ",";

// ================================================================================================
// Scope-walk helpers
// ================================================================================================

/// Walk up the AST from `call_node` to find the declaration of the receiver variable named
/// `receiver_var_name`.
///
/// Checks declaration sources in order of proximity (innermost scope wins):
///
/// 1. **`block`** — `local_variable_declaration` statements that precede the call node by
///    source position.
/// 2. **`lambda_expression`** — lambda parameters (typed or inferred) that introduce a
///    declaration for the receiver name.  Checked on the way up so that a lambda parameter
///    correctly shadows an outer local variable or class field.
/// 3. **`method_declaration` / `constructor_declaration` / `compact_constructor_declaration`**
///    — `formal_parameter` nodes in the enclosing method/constructor signature.
///    `compact_constructor_declaration` has no `formal_parameters` child, so the scan finds
///    nothing and the walk continues upward to the record body.
/// 4. **`class_body`** — `field_declaration` nodes at class level.  Reached only when no
///    closer declaration was found (e.g. a field-injected client called inside a
///    `thenCompose` lambda).  If the `class_body`'s parent is a `record_declaration`, the
///    record's `formal_parameters` (its components) are also scanned.  The walk stops here
///    so that outer/enclosing types are not searched.
///
/// For local variable declarations and field declarations, the initializer expression is
/// extracted and returned as a [`ReceiverDeclaration`].
/// For formal parameters, lambda parameters, and record components, the full parameter
/// declaration text is used as `expr`.
pub(super) fn find_receiver_declaration(
    call_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
    receiver_var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    let call_start = call_node.range().start;

    for ancestor in call_node.ancestors() {
        let kind = ancestor.kind();
        let kind_str = kind.as_ref();

        if kind_str == BLOCK {
            // Scan children of this block that appear before the call node
            for child in ancestor.children() {
                if child.range().end > call_start {
                    break;
                }
                if child.kind().as_ref() == LOCAL_VARIABLE_DECLARATION {
                    if let Some(decl) = extract_declaration_from_local_var_decl(
                        &child,
                        receiver_var_name,
                        source_file,
                    ) {
                        return Some(decl);
                    }
                }
            }
        } else if kind_str == LAMBDA_EXPRESSION {
            // Check whether this lambda introduces a parameter named receiver_var_name.
            // Tree-sitter Java represents lambda parameters in three ways:
            //   inferred_parameters  → (x, y)           — no type annotation
            //   formal_parameters    → (S3Client s3)     — typed
            //   identifier           → x                 — single unparenthesised param
            for child in ancestor.children() {
                let child_kind = child.kind();
                let child_kind_str = child_kind.as_ref();

                if child_kind_str == INFERRED_PARAMETERS {
                    // (x, y) — identifiers inside the inferred_parameters node
                    for param in child.children() {
                        if param.kind().as_ref() == IDENTIFIER && param.text() == receiver_var_name
                        {
                            return Some(ReceiverDeclaration {
                                expr: param.text().to_string(),
                                type_name: None,
                                location: Location::from_node(source_file.path.clone(), &param),
                            });
                        }
                    }
                } else if child_kind_str == FORMAL_PARAMETERS {
                    // (S3Client s3) — typed lambda parameters
                    for param in child.children() {
                        if param.kind().as_ref() == FORMAL_PARAMETER {
                            if let Some(decl) = extract_declaration_from_formal_param(
                                &param,
                                receiver_var_name,
                                source_file,
                            ) {
                                return Some(decl);
                            }
                        }
                    }
                } else if child_kind_str == IDENTIFIER && child.text() == receiver_var_name {
                    // x -> x.foo()  — single unparenthesised lambda parameter
                    return Some(ReceiverDeclaration {
                        expr: child.text().to_string(),
                        type_name: None,
                        location: Location::from_node(source_file.path.clone(), &child),
                    });
                }
            }
            // No match in this lambda's params — continue walking outward
        } else if kind_str == METHOD_DECLARATION
            || kind_str == CONSTRUCTOR_DECLARATION
            || kind_str == COMPACT_CONSTRUCTOR_DECLARATION
        {
            // Check formal parameters of this method/constructor.
            // Note: compact_constructor_declaration has no formal_parameters child —
            // the record components are the implicit parameters.  The loop below finds
            // nothing for compact constructors, and the walk continues upward to
            // class_body (the record body) where record components are resolved.
            for child in ancestor.children() {
                if child.kind().as_ref() == FORMAL_PARAMETERS {
                    for param in child.children() {
                        if param.kind().as_ref() == FORMAL_PARAMETER {
                            if let Some(decl) = extract_declaration_from_formal_param(
                                &param,
                                receiver_var_name,
                                source_file,
                            ) {
                                return Some(decl);
                            }
                        }
                    }
                }
            }
            // Do NOT break here — continue up to class_body so that field-injected
            // clients called inside lambda chains can be resolved.
        } else if kind_str == TRY_WITH_RESOURCES_STATEMENT {
            // try-with-resources: the resource_specification is a sibling of the try body
            // block, not an ancestor of the call node, so it is not visited by the normal
            // block scan.  Walk the resource_specification child here instead.
            //
            // No position guard is needed: resources always textually precede the try body,
            // so any matching resource is always in scope for the entire try block.
            for child in ancestor.children() {
                if child.kind().as_ref() == RESOURCE_SPECIFICATION {
                    for resource in child.children() {
                        if resource.kind().as_ref() == RESOURCE {
                            if let Some(decl) = extract_declaration_from_resource(
                                &resource,
                                receiver_var_name,
                                source_file,
                            ) {
                                return Some(decl);
                            }
                        }
                    }
                }
            }
            // No match in this try's resources — continue walking outward (try may be nested)
        } else if kind_str == IF_STATEMENT {
            // Check whether the if-condition contains an instanceof pattern that binds
            // receiver_var_name.  Java 16+ `instanceof` pattern matching introduces a
            // binding variable that is in scope for the entire then-branch:
            //
            //   if (client instanceof S3Client s3) {
            //       s3.putObject(request);   ← s3 is bound here
            //   }
            //
            // Actual tree-sitter Java grammar AST shape:
            //
            //   if_statement
            //     ├── "if"
            //     ├── parenthesized_expression   ← the condition in parens
            //     │     ├── "("
            //     │     ├── instanceof_expression
            //     │     │     ├── identifier       ← left operand (e.g. "client")
            //     │     │     ├── "instanceof"
            //     │     │     ├── type_identifier  ← declared type (e.g. "S3Client")
            //     │     │     └── identifier       ← binding variable (e.g. "s3")
            //     │     └── ")"
            //     └── block                        ← then-branch (ancestor of the call)
            //
            // Note: there is NO intermediate `type_pattern` wrapper in this grammar version.
            // The binding variable is the last `identifier` child of `instanceof_expression`,
            // and the declared type is the `type_identifier` child immediately before it.
            //
            // No position guard is needed: the call is inside the then-block, which is
            // always textually after the condition, so the binding is always in scope.
            for child in ancestor.children() {
                if child.kind().as_ref() == PARENTHESIZED_EXPRESSION {
                    for paren_child in child.children() {
                        if paren_child.kind().as_ref() == INSTANCEOF_EXPRESSION {
                            if let Some(decl) = extract_declaration_from_instanceof_expr(
                                &paren_child,
                                receiver_var_name,
                                source_file,
                            ) {
                                return Some(decl);
                            }
                        }
                    }
                }
            }
            // No match in this if's condition — continue walking outward
        } else if kind_str == CLASS_BODY {
            // Scan field declarations at class level.  No position guard is applied
            // because field ordering relative to the call site is not meaningful.
            for child in ancestor.children() {
                if child.kind().as_ref() == FIELD_DECLARATION {
                    if let Some(decl) =
                        extract_declaration_from_field_decl(&child, receiver_var_name, source_file)
                    {
                        return Some(decl);
                    }
                }
            }

            // If this class_body belongs to a record_declaration, also scan the record's
            // formal_parameters (its components).  In the tree-sitter Java grammar, a
            // record_declaration has the shape:
            //
            //   record_declaration
            //     ├── "record"
            //     ├── identifier          ← record name
            //     ├── formal_parameters   ← record components (e.g. `(S3Client s3)`)
            //     └── class_body          ← record body (methods, compact constructors)
            //
            // Record components are formal_parameter nodes, so the existing
            // extract_declaration_from_formal_param helper handles them directly.
            if let Some(record_decl) = ancestor.parent() {
                if record_decl.kind().as_ref() == RECORD_DECLARATION {
                    for child in record_decl.children() {
                        if child.kind().as_ref() == FORMAL_PARAMETERS {
                            for param in child.children() {
                                if param.kind().as_ref() == FORMAL_PARAMETER {
                                    if let Some(decl) = extract_declaration_from_formal_param(
                                        &param,
                                        receiver_var_name,
                                        source_file,
                                    ) {
                                        return Some(decl);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Stop at the class/record boundary — do not walk into enclosing types.
            break;
        }
    }

    None
}

// ================================================================================================
// Private helpers
// ================================================================================================

/// Extract a [`ReceiverDeclaration`] from a `local_variable_declaration` node if it declares
/// `var_name`.
///
/// The Java AST shape is:
/// ```text
/// local_variable_declaration
///   ├── <type>                    ← first child (e.g. "S3Client" or "var")
///   └── variable_declarator
///         ├── identifier          ← variable name
///         └── <initializer>       ← the initializer expression (optional)
/// ```
///
/// Returns `None` if the declaration does not declare `var_name`.
///
/// When an initializer is present, `expr` is set to the initializer expression and
/// `location` points to it.  When no initializer is present but the declared type is
/// known (not `var`), `expr` is set to the variable name and `location` points to the
/// name identifier — this is sufficient for the matcher's Tier-1 type-based resolution,
/// which only needs `type_name`.
fn extract_declaration_from_local_var_decl(
    decl_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
    var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    let mut children = decl_node.children();

    // First child is the declared type
    let type_node = children.next()?;
    let type_text = type_node.text().to_string();

    // Remaining children are variable_declarator nodes
    for declarator in children {
        if declarator.kind().as_ref() != VARIABLE_DECLARATOR {
            continue;
        }

        let decl_children: Vec<_> = declarator.children().collect();
        if decl_children.is_empty() {
            continue;
        }

        // First child of variable_declarator is the variable name identifier
        let name_node = &decl_children[0];
        let name = name_node.text().to_string();
        if name != var_name {
            continue;
        }

        // `var` is Java's inferred type keyword — not a concrete type name we can look up
        let type_name = if type_text == "var" {
            None
        } else {
            Some(type_text)
        };

        // The initializer is the last child of variable_declarator (after the `=` token)
        let last_node = decl_children.last()?;

        if last_node.text() == var_name {
            // No initializer present — use the variable name as the expr placeholder.
            // type_name is still known and sufficient for Tier-1 type-based resolution.
            return Some(ReceiverDeclaration {
                expr: var_name.to_string(),
                type_name,
                location: Location::from_node(source_file.path.clone(), name_node),
            });
        }

        let init_expr = last_node.text().to_string();
        let init_location = Location::from_node(source_file.path.clone(), last_node);

        return Some(ReceiverDeclaration {
            expr: init_expr,
            type_name,
            location: init_location,
        });
    }

    None
}

/// Extract a [`ReceiverDeclaration`] from a `field_declaration` node if it declares `var_name`.
///
/// Unlike [`extract_declaration_from_local_var_decl`], this function handles the optional
/// `modifiers` child that precedes the type in a field declaration:
///
/// ```text
/// field_declaration
///   ├── modifiers?                ← e.g. "private final"  (may be absent)
///   ├── <type>                    ← e.g. "S3AsyncClient"
///   └── variable_declarator
///         ├── identifier          ← variable name
///         └── <initializer>       ← the initializer expression (optional)
/// ```
///
/// Returns `None` if the declaration does not declare `var_name`.
///
/// When an initializer is present, `expr` is set to the initializer expression and
/// `location` points to it.  When no initializer is present but the declared type is
/// known (not `var`), `expr` is set to the variable name and `location` points to the
/// name identifier — this is sufficient for the matcher's Tier-1 type-based resolution,
/// which only needs `type_name`.
fn extract_declaration_from_field_decl(
    decl_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
    var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    // Skip the optional leading `modifiers` node to find the declared type.
    let mut children = decl_node.children().peekable();

    // Skip modifiers if present
    if children
        .peek()
        .map(|n| n.kind().as_ref() == MODIFIERS)
        .unwrap_or(false)
    {
        children.next();
    }

    // Next child is the declared type
    let type_node = children.next()?;
    let type_text = type_node.text().to_string();

    // Remaining children include variable_declarator nodes
    for declarator in children {
        if declarator.kind().as_ref() != VARIABLE_DECLARATOR {
            continue;
        }

        let decl_children: Vec<_> = declarator.children().collect();
        if decl_children.is_empty() {
            continue;
        }

        // First child of variable_declarator is the variable name identifier
        let name_node = &decl_children[0];
        let name = name_node.text().to_string();
        if name != var_name {
            continue;
        }

        // `var` is Java's inferred type keyword — not a concrete type name we can look up
        let type_name = if type_text == "var" {
            None
        } else {
            Some(type_text)
        };

        // The initializer is the last child of variable_declarator (after the `=` token)
        let last_node = decl_children.last()?;

        if last_node.text() == var_name {
            // No initializer present — use the variable name as the expr placeholder.
            // type_name is still known and sufficient for Tier-1 type-based resolution.
            return Some(ReceiverDeclaration {
                expr: var_name.to_string(),
                type_name,
                location: Location::from_node(source_file.path.clone(), name_node),
            });
        }

        let init_expr = last_node.text().to_string();
        let init_location = Location::from_node(source_file.path.clone(), last_node);

        return Some(ReceiverDeclaration {
            expr: init_expr,
            type_name,
            location: init_location,
        });
    }

    None
}

/// Extract a [`ReceiverDeclaration`] from a `resource` node inside a `try`-with-resources
/// statement if it declares `var_name`.
///
/// The Java AST shape for a `resource` node is:
/// ```text
/// resource
///   ├── <type>        ← e.g. "S3Client"
///   ├── identifier    ← variable name, e.g. "s3"
///   ├── "="           ← assignment operator token
///   └── <initializer> ← e.g. "S3Client.create()"
/// ```
///
/// Unlike [`extract_declaration_from_local_var_decl`], there is no intermediate
/// `variable_declarator` wrapper — the type, name, and initializer are direct children of
/// the `resource` node.
///
/// Returns `None` if the resource does not declare `var_name`.
fn extract_declaration_from_resource(
    resource_node: &ast_grep_core::Node<
        ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>,
    >,
    var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    let children: Vec<_> = resource_node.children().collect();

    // Need at least: <type>, identifier, "=", <initializer>
    if children.len() < 2 {
        return None;
    }

    // First child is the declared type (may be preceded by modifiers in some grammars,
    // but the standard resource node starts directly with the type).
    let type_node = &children[0];
    let type_text = type_node.text().to_string();

    // Second child is the variable name identifier
    let name_node = &children[1];
    if name_node.kind().as_ref() != IDENTIFIER {
        return None;
    }
    if name_node.text() != var_name {
        return None;
    }

    // `var` is Java's inferred type keyword — not a concrete type name we can look up
    let type_name = if type_text == "var" {
        None
    } else {
        Some(type_text)
    };

    // The initializer is the last child (after the `=` token).
    // If the last child is the identifier itself, there is no initializer (shouldn't
    // happen for a resource, but guard defensively).
    let last_node = children.last()?;
    if last_node.text() == var_name {
        return Some(ReceiverDeclaration {
            expr: var_name.to_string(),
            type_name,
            location: Location::from_node(source_file.path.clone(), name_node),
        });
    }

    Some(ReceiverDeclaration {
        expr: last_node.text().to_string(),
        type_name,
        location: Location::from_node(source_file.path.clone(), last_node),
    })
}

/// Extract a [`ReceiverDeclaration`] from a `formal_parameter` node if its name matches `var_name`.
///
/// `expr` is set to the full parameter declaration text (e.g. `"S3Client s3"`).
///
/// The Java AST shape is:
/// ```text
/// formal_parameter
///   ├── <type>              ← first child (e.g. "S3Client")
///   └── identifier          ← last child is the parameter name
/// ```
fn extract_declaration_from_formal_param(
    param_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
    var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    let children: Vec<_> = param_node.children().collect();
    if children.len() < 2 {
        return None;
    }

    let type_text = children[0].text().to_string();
    let name = children.last()?.text().to_string();

    if name != var_name {
        return None;
    }

    let type_name = if type_text == "var" {
        None
    } else {
        Some(type_text)
    };
    let expr = param_node.text().to_string();
    let location = Location::from_node(source_file.path.clone(), param_node);

    Some(ReceiverDeclaration {
        expr,
        type_name,
        location,
    })
}

/// Extract a [`ReceiverDeclaration`] from an `instanceof_expression` node if its binding
/// variable matches `var_name`.
///
/// Java 16+ pattern matching introduces a binding variable in an `instanceof` expression:
///
/// ```java
/// if (client instanceof S3Client s3) { … }
/// ```
///
/// In the version of the tree-sitter Java grammar used here, the `instanceof_expression`
/// node has the following direct children (no intermediate `type_pattern` wrapper):
///
/// ```text
/// instanceof_expression
///   ├── identifier       ← left operand (e.g. "client")
///   ├── "instanceof"     ← keyword token
///   ├── type_identifier  ← declared type (e.g. "S3Client")
///   └── identifier       ← binding variable name (e.g. "s3")
/// ```
///
/// Returns `None` if:
/// - the `instanceof_expression` has no `type_identifier` child, or
/// - the last `identifier` child does not match `var_name`.
///
/// When a match is found, `expr` is set to `"<TypeName> <varName>"` (matching the
/// `formal_parameter` convention), `type_name` is set to the type identifier text, and
/// `location` points to the `instanceof_expression` node.
fn extract_declaration_from_instanceof_expr(
    instanceof_node: &ast_grep_core::Node<
        ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>,
    >,
    var_name: &str,
    source_file: &SourceFile,
) -> Option<ReceiverDeclaration> {
    let children: Vec<_> = instanceof_node.children().collect();

    // Find the type_identifier child and the last identifier child.
    let mut type_text: Option<String> = None;
    let mut last_identifier: Option<String> = None;

    for child in &children {
        let kind = child.kind();
        let kind_str = kind.as_ref();
        if kind_str == TYPE_IDENTIFIER {
            type_text = Some(child.text().to_string());
        } else if kind_str == IDENTIFIER {
            last_identifier = Some(child.text().to_string());
        }
    }

    // The binding variable is the last identifier child.
    // If it doesn't match var_name, this instanceof doesn't bind the receiver.
    let binding_name = last_identifier?;
    if binding_name != var_name {
        return None;
    }

    // The type must be known (no `var` in instanceof patterns).
    let type_name = type_text?;

    let expr = format!("{type_name} {binding_name}");
    let location = Location::from_node(source_file.path.clone(), instanceof_node);

    Some(ReceiverDeclaration {
        expr,
        type_name: Some(type_name),
        location,
    })
}

// ================================================================================================
// Argument extraction helpers
// ================================================================================================

/// Resolve a Java AST argument node to a [`ParameterValue`].
///
/// Literal nodes whose value is statically known are returned as
/// [`ParameterValue::Resolved`] with quotes stripped where applicable.
/// Everything else (identifiers, method calls, field accesses, …) is
/// [`ParameterValue::Unresolved`].
///
/// Java tree-sitter literal node kinds:
/// - `string_literal`                  → strip surrounding `"…"` quotes
/// - `decimal_integer_literal`         → keep text as-is
/// - `decimal_floating_point_literal`  → keep text as-is
/// - `hex_integer_literal`             → keep text as-is
/// - `octal_integer_literal`           → keep text as-is
/// - `binary_integer_literal`          → keep text as-is
/// - `true` / `false`                  → keep text as-is
/// - `null_literal`                    → keep text as-is
/// - `character_literal`               → strip surrounding `'…'` quotes
pub(crate) fn resolve_java_literal(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
) -> ParameterValue {
    let kind = node.kind();
    let text = node.text();
    match kind.as_ref() {
        "string_literal" => {
            // tree-sitter includes the surrounding double-quotes in the text
            let inner = text.trim_matches('"');
            ParameterValue::Resolved(inner.to_string())
        }
        "character_literal" => {
            // tree-sitter includes the surrounding single-quotes in the text
            let inner = text.trim_matches('\'');
            ParameterValue::Resolved(inner.to_string())
        }
        "decimal_integer_literal"
        | "decimal_floating_point_literal"
        | "hex_integer_literal"
        | "octal_integer_literal"
        | "binary_integer_literal"
        | "true"
        | "false"
        | "null_literal" => ParameterValue::Resolved(text.to_string()),
        _ => ParameterValue::Unresolved(text.to_string()),
    }
}

/// Extract positional arguments from a `method_invocation` AST node.
pub(crate) fn extract_arguments_from_node(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<ast_grep_language::Java>>,
) -> Vec<Parameter> {
    let mut parameters = Vec::new();

    let arg_list = node
        .children()
        .find(|child| child.kind().as_ref() == ARGUMENT_LIST);

    let Some(arg_list) = arg_list else {
        return parameters;
    };

    let mut position = 0usize;
    for child in arg_list.children() {
        let kind = child.kind();
        if kind.as_ref() == LEFT_PAREN || kind.as_ref() == RIGHT_PAREN || kind.as_ref() == COMMA {
            continue;
        }
        parameters.push(Parameter::Positional {
            value: resolve_java_literal(&child),
            position,
            type_annotation: None,
            struct_fields: None,
        });
        position += 1;
    }

    parameters
}
