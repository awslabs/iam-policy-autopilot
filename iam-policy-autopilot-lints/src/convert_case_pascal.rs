//! Lint to detect calls to convert_case's to_case with Case::Pascal argument

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::{Expr, ExprKind, QPath};
use rustc_lint::{LateContext, LateLintPass, LintPass, LintStore};
use rustc_session::{declare_lint, Session};

declare_lint! {
    /// ### What it does
    /// Detects calls to `convert_case::to_case` with `Case::Pascal` as an argument.
    ///
    /// ### Why is this bad?
    /// Using `Case::Pascal` with `to_case` might indicate a pattern that should be
    /// avoided or replaced with a more appropriate alternative in this codebase.
    ///
    /// ### Example
    /// ```rust
    /// use convert_case::{Case, Casing};
    ///
    /// // This will trigger the lint
    /// let result = some_string.to_case(Case::Pascal);
    /// ```
    ///
    /// Consider using an alternative approach or a different case conversion.
    pub CONVERT_CASE_PASCAL,
    Deny,
    "use of convert_case::to_case with Case::Pascal argument"
}

pub struct ConvertCasePascal;

/// Check if an expression is a path that matches Case::Pascal
fn is_case_pascal_path(expr: &Expr<'_>) -> bool {
    if let ExprKind::Path(QPath::Resolved(_, path)) = &expr.kind {
        // Check if the path ends with Pascal and has Case as a segment
        let segments: Vec<&str> = path
            .segments
            .iter()
            .map(|s| s.ident.name.as_str())
            .collect();

        // Look for patterns like Case::Pascal or convert_case::Case::Pascal
        if segments.len() >= 2 {
            let last_two = &segments[segments.len() - 2..];
            return last_two == ["Case", "Pascal"];
        }
    }
    false
}

/// Check if an expression is a method call to to_case
fn is_to_case_method_call<'tcx>(expr: &'tcx Expr<'_>) -> Option<&'tcx [Expr<'tcx>]> {
    if let ExprKind::MethodCall(path_segment, _receiver, args, _) = &expr.kind {
        if path_segment.ident.name.as_str() == "to_case" {
            return Some(args);
        }
    }
    None
}

impl LintPass for ConvertCasePascal {
    fn name(&self) -> &'static str {
        "ConvertCasePascal"
    }

    fn get_lints(&self) -> Vec<&'static rustc_lint::Lint> {
        vec![&CONVERT_CASE_PASCAL]
    }
}

impl<'tcx> LateLintPass<'tcx> for ConvertCasePascal {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'_>) {
        // Check if this is a method call to to_case
        if let Some(args) = is_to_case_method_call(expr) {
            // Check if the first argument is Case::Pascal
            if let Some(first_arg) = args.first() {
                if is_case_pascal_path(first_arg) {
                    let help = "if you are converting a method name to an operation name, using PascalCase conversion is not what you want: Use ServiceModelIndex::method_lookup instead.";

                    span_lint_and_help(
                        cx,
                        CONVERT_CASE_PASCAL,
                        expr.span,
                        "calling `to_case` with `Case::Pascal`",
                        None,
                        help,
                    );
                }
            }
        }
    }
}

pub fn register_lints(_sess: &Session, lint_store: &mut LintStore) {
    lint_store.register_lints(&[&CONVERT_CASE_PASCAL]);
    lint_store.register_late_pass(|_| Box::new(ConvertCasePascal));
}
