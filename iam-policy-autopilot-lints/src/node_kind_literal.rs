//! Lint to enforce use of constants instead of string literals for node kinds

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_ast::LitKind;
use rustc_hir::{BinOpKind, Expr, ExprKind};
use rustc_lint::{LateContext, LateLintPass, LintPass, LintStore};
use rustc_session::{declare_lint, Session};

declare_lint! {
    /// ### What it does
    /// Detects string literals used in comparisons with `.kind()` method calls,
    /// which typically indicate Tree-sitter node kind checks that should use constants.
    ///
    /// ### Why is this bad?
    /// Using string literals for node kinds:
    /// - Lacks compile-time checking
    /// - Misses IDE autocomplete support
    /// - Makes refactoring harder
    /// - Can lead to typos
    ///
    /// ### Example
    /// ```rust
    /// // Bad - string literal
    /// if node.kind() == "composite_literal" {
    ///     // ...
    /// }
    /// ```
    ///
    /// Use instead:
    /// ```rust
    /// // Good - constant from node_kinds module
    /// use crate::extraction::go::node_kinds::COMPOSITE_LITERAL;
    /// if node.kind() == COMPOSITE_LITERAL {
    ///     // ...
    /// }
    /// ```
    pub NODE_KIND_LITERAL,
    Deny,
    "use of string literals in comparisons with .kind() method calls"
}

pub struct NodeKindLiteral;

/// Check if an expression is a call to the `.kind()` method on a tree-sitter Node
fn is_kind_method_call<'tcx>(cx: &rustc_lint::LateContext<'tcx>, expr: &'tcx Expr<'_>) -> bool {
    if let ExprKind::MethodCall(path_segment, receiver, _, _) = &expr.kind {
        if path_segment.ident.name.as_str() != "kind" {
            return false;
        }

        // Check if the receiver type contains "Node" in its path
        // This catches tree_sitter::Node and similar types
        let receiver_ty = cx.typeck_results().expr_ty(receiver);
        let ty_str = format!("{receiver_ty:?}");
        ty_str.contains("Node")
    } else {
        false
    }
}

impl LintPass for NodeKindLiteral {
    fn name(&self) -> &'static str {
        "NodeKindLiteral"
    }

    fn get_lints(&self) -> Vec<&'static rustc_lint::Lint> {
        vec![&NODE_KIND_LITERAL]
    }
}

impl<'tcx> LateLintPass<'tcx> for NodeKindLiteral {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'_>) {
        // Check if this is a binary operation (== or !=)
        if let ExprKind::Binary(op, left, right) = &expr.kind {
            // Only check equality and inequality operations
            if !matches!(op.node, BinOpKind::Eq | BinOpKind::Ne) {
                return;
            }

            // Check if one side is a .kind() call and the other is a string literal
            let (kind_call, literal_expr) = if is_kind_method_call(cx, left) {
                if let ExprKind::Lit(_) = right.kind {
                    (Some(left), Some(right))
                } else {
                    (None, None)
                }
            } else if is_kind_method_call(cx, right) {
                if let ExprKind::Lit(_) = left.kind {
                    (Some(right), Some(left))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

            if let (Some(_kind_call), Some(literal_expr)) = (kind_call, literal_expr) {
                if let ExprKind::Lit(lit) = &literal_expr.kind {
                    if let LitKind::Str(symbol, _) = lit.node {
                        let literal_value = symbol.as_str();
                        let constant_name = literal_value.to_uppercase();

                        let msg = format!(
                            "comparing .kind() with string literal \"{}\"",
                            literal_value
                        );
                        let help = format!(
                            "define and use a constant like `const {}: &str = \"{}\";` in a node_kinds module",
                            constant_name,
                            literal_value
                        );

                        span_lint_and_help(
                            cx,
                            NODE_KIND_LITERAL,
                            literal_expr.span,
                            msg,
                            None,
                            help,
                        );
                    }
                }
            }
        }
    }
}

pub fn register_lints(_sess: &Session, lint_store: &mut LintStore) {
    lint_store.register_lints(&[&NODE_KIND_LITERAL]);
    lint_store.register_late_pass(|_| Box::new(NodeKindLiteral));
}
