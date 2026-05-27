//! Library call extraction for external library models.
//!
//! This module resolves Python import statements and matches function calls
//! against loaded `ExternalLibraryModel` patterns to produce `SdkMethodCall`
//! entries for third-party libraries that wrap AWS SDK operations.

use std::collections::HashMap;

use ast_grep_language::Python;

use crate::extraction::external_library_models::{
    CallPattern, CallType, ExternalLibraryModel, LibraryModelRegistry,
};
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::{AstWithSourceFile, SdkMethodCall, SdkMethodCallMetadata};
use crate::{Language, Location};

mod import_node_kinds {
    /// `Y as Z` within an import
    pub(super) const ALIASED_IMPORT: &str = "aliased_import";
    /// Dotted name like `X.Y.Z`
    pub(super) const DOTTED_NAME: &str = "dotted_name";
    /// Simple identifier
    pub(super) const IDENTIFIER: &str = "identifier";
    /// Wildcard import `*`
    pub(super) const WILDCARD_IMPORT: &str = "wildcard_import";
}

/// Resolved import information from a Python source file.
pub(crate) struct ResolvedImports {
    /// Mapping from local name to canonical dotted module path.
    imports: HashMap<String, String>,
    /// Module paths that had `from X import *` (wildcard imports).
    wildcard_modules: Vec<String>,
}

/// Resolve Python import statements from the AST into a mapping from local
/// names to canonical module paths.
///
/// Returns a `ResolvedImports` where `imports` maps local names to canonical
/// dotted module paths, and `wildcard_modules` lists module paths that used `from X import *`.
///
/// # Import patterns handled
///
/// | Python code | Local name | Canonical path |
/// |---|---|---|
/// | `from X import Y` | `"Y"` | `"X.Y"` |
/// | `from X import Y as Z` | `"Z"` | `"X.Y"` |
/// | `from X.Y import func` | `"func"` | `"X.Y.func"` |
/// | `import X.Y as Z` | `"Z"` | `"X.Y"` |
/// | `import X.Y` | `"X"` | `"X.Y"` |
///
/// Wildcard imports (`from X import *`) are recorded in `wildcard_modules`
/// for later expansion against loaded model patterns.
pub(crate) fn resolve_imports(ast: &AstWithSourceFile<Python>) -> ResolvedImports {
    let mut imports: HashMap<String, String> = HashMap::new();
    let mut wildcard_modules: Vec<String> = Vec::new();
    let root = ast.ast.root();

    for node_match in root.find_all("from $MODULE import $$$NAMES") {
        resolve_import_from_statement(node_match.get_node(), &mut imports, &mut wildcard_modules);
    }

    for node_match in root.find_all("import $$$MODULES") {
        resolve_import_statement(node_match.get_node(), &mut imports);
    }

    ResolvedImports {
        imports,
        wildcard_modules,
    }
}

/// Resolve a `from X import Y [as Z], ...` statement.
fn resolve_import_from_statement(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    imports: &mut HashMap<String, String>,
    wildcard_modules: &mut Vec<String>,
) {
    let children: Vec<_> = node.children().collect();

    let mut module_path = String::new();
    let mut found_module = false;
    let mut past_import_keyword = false;

    for child in &children {
        let child_kind = child.kind();
        let child_text = child.text();

        if child_text.as_ref() == "from" {
            continue;
        }

        if child_text.as_ref() == "import" {
            past_import_keyword = true;
            continue;
        }

        if !past_import_keyword && !found_module {
            if child_kind == import_node_kinds::DOTTED_NAME
                || child_kind == import_node_kinds::IDENTIFIER
            {
                module_path = child_text.to_string();
                found_module = true;
            }
            continue;
        }

        if past_import_keyword {
            if child_text.as_ref() == ","
                || child_text.as_ref() == "("
                || child_text.as_ref() == ")"
            {
                continue;
            }

            if child_kind == import_node_kinds::WILDCARD_IMPORT {
                if !module_path.is_empty() {
                    wildcard_modules.push(module_path.clone());
                }
                continue;
            }

            if child_kind == import_node_kinds::ALIASED_IMPORT {
                resolve_aliased_import_from(child, &module_path, imports);
            } else if child_kind == import_node_kinds::DOTTED_NAME
                || child_kind == import_node_kinds::IDENTIFIER
            {
                let imported_name = child_text.to_string();
                if !imported_name.is_empty() {
                    let canonical = format!("{module_path}.{imported_name}");
                    imports.insert(imported_name, canonical);
                }
            }
        }
    }
}

/// Resolve `from X import Y as Z` within a from-import statement.
fn resolve_aliased_import_from(
    aliased_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    module_path: &str,
    imports: &mut HashMap<String, String>,
) {
    let children: Vec<_> = aliased_node.children().collect();
    let mut original_name = String::new();
    let mut alias_name = String::new();
    let mut found_as = false;

    for child in &children {
        let child_text = child.text();

        if child_text.as_ref() == "as" {
            found_as = true;
            continue;
        }

        let child_kind = child.kind();
        if found_as {
            if child_kind == import_node_kinds::IDENTIFIER {
                alias_name = child_text.to_string();
            }
        } else if child_kind == import_node_kinds::DOTTED_NAME
            || child_kind == import_node_kinds::IDENTIFIER
        {
            original_name = child_text.to_string();
        }
    }

    if !alias_name.is_empty() && !original_name.is_empty() {
        let canonical = format!("{module_path}.{original_name}");
        imports.insert(alias_name, canonical);
    }
}

/// Resolve an `import X.Y [as Z]` statement.
///
/// For `import X.Y`, the local name is `"X"` (first component).
/// For `import X.Y as Z`, the local name is `"Z"`.
fn resolve_import_statement(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    imports: &mut HashMap<String, String>,
) {
    let children: Vec<_> = node.children().collect();

    for child in &children {
        let child_kind = child.kind();
        let child_text = child.text();

        if child_text.as_ref() == "import" || child_text.as_ref() == "," {
            continue;
        }

        if child_kind == import_node_kinds::ALIASED_IMPORT {
            resolve_aliased_import_plain(child, imports);
        } else if child_kind == import_node_kinds::DOTTED_NAME {
            let full_path = child_text.to_string();
            if let Some(first_component) = full_path.split('.').next() {
                if !first_component.is_empty() {
                    imports.insert(first_component.to_string(), full_path);
                }
            }
        } else if child_kind == import_node_kinds::IDENTIFIER {
            let name = child_text.to_string();
            if !name.is_empty() {
                imports.insert(name.clone(), name);
            }
        }
    }
}

/// Resolve `import X.Y as Z` within a plain import statement.
fn resolve_aliased_import_plain(
    aliased_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    imports: &mut HashMap<String, String>,
) {
    let children: Vec<_> = aliased_node.children().collect();

    let mut full_path = String::new();
    let mut alias_name = String::new();
    let mut found_as = false;

    for child in &children {
        let child_text = child.text();

        if child_text.as_ref() == "as" {
            found_as = true;
            continue;
        }

        let child_kind = child.kind();
        if found_as {
            if child_kind == import_node_kinds::IDENTIFIER {
                alias_name = child_text.to_string();
            }
        } else if child_kind == import_node_kinds::DOTTED_NAME
            || child_kind == import_node_kinds::IDENTIFIER
        {
            full_path = child_text.to_string();
        }
    }

    if !alias_name.is_empty() && !full_path.is_empty() {
        imports.insert(alias_name, full_path);
    }
}

// ─── LibraryCallExtractor ───────────────────────────────────────────

/// Extracts external library calls from Python source code and maps them to
/// `SdkMethodCall` entries using loaded `ExternalLibraryModel` patterns.
pub(crate) struct LibraryCallExtractor<'a> {
    registry: &'a LibraryModelRegistry,
}

impl<'a> LibraryCallExtractor<'a> {
    pub(crate) fn new(registry: &'a LibraryModelRegistry) -> Self {
        Self { registry }
    }

    /// Extract library calls from an already-parsed Python AST.
    pub(crate) fn extract_library_method_calls(
        &self,
        ast: &AstWithSourceFile<Python>,
    ) -> Vec<SdkMethodCall> {
        let resolved = resolve_imports(ast);

        let models = self.registry.models();

        let mut imports = resolved.imports;
        if imports.is_empty() && resolved.wildcard_modules.is_empty() {
            return Vec::new();
        }

        // Expand wildcard imports using model call patterns:
        // `from X import *` synthesizes entries for each function the model
        // declares under module path X.
        for wildcard_module in &resolved.wildcard_modules {
            for model in models {
                for pattern in &model.call_patterns {
                    if pattern.module_path == *wildcard_module {
                        let canonical = format!("{}.{}", wildcard_module, pattern.function_name);
                        imports
                            .entry(pattern.function_name.clone())
                            .or_insert(canonical);
                    }
                }
            }
        }

        let mut all_calls = Vec::new();
        for model in models {
            let calls = self.match_call_patterns(ast, &imports, model);
            all_calls.extend(calls);
        }

        all_calls
    }

    /// Match call patterns from a model against call sites in the AST.
    ///
    /// Uses a single `$FUNC($$$ARGS)` pass over all call nodes, then branches:
    /// - Dotted callee (`parameters.get_parameter`) -> split into object + method,
    ///   check qualified import resolution
    /// - Simple callee (`get_parameter`) -> check direct import resolution
    fn match_call_patterns(
        &self,
        ast: &AstWithSourceFile<Python>,
        resolved_imports: &HashMap<String, String>,
        model: &ExternalLibraryModel,
    ) -> Vec<SdkMethodCall> {
        let mut results = Vec::new();
        let root = ast.ast.root();

        for node_match in root.find_all("$FUNC($$$ARGS)") {
            let env = node_match.get_env();
            let func_node = match env.get_match("FUNC") {
                Some(node) => node,
                None => continue,
            };

            let func_text = func_node.text().to_string();

            let func_kind = func_node.kind();
            let matched_pattern = if func_kind.as_ref() == "attribute" {
                let (object_text, method_name) = match func_text.rsplit_once('.') {
                    Some(pair) => pair,
                    None => continue,
                };
                self.match_qualified_call(object_text, method_name, resolved_imports, model)
            } else if func_kind.as_ref() == "identifier" {
                self.match_direct_call(&func_text, resolved_imports, model)
            } else {
                continue;
            };

            if let Some(pattern) = matched_pattern {
                let matched_node = node_match.get_node();
                let calls = Self::to_sdk_method_calls(pattern, matched_node, &ast.source_file);
                results.extend(calls);
            }
        }

        results
    }

    /// Match a qualified call like `parameters.get_parameter(...)`.
    fn match_qualified_call<'m>(
        &self,
        object_text: &str,
        method_name: &str,
        resolved_imports: &HashMap<String, String>,
        model: &'m ExternalLibraryModel,
    ) -> Option<&'m CallPattern> {
        for pattern in &model.call_patterns {
            if pattern.function_name != method_name {
                continue;
            }

            let matches = match pattern.call_type {
                CallType::Function => self.matches_imported_module(
                    object_text,
                    resolved_imports,
                    &pattern.module_path,
                ),
                CallType::StaticMethod => {
                    let class_name = match &pattern.class_name {
                        Some(name) => name,
                        None => continue,
                    };
                    self.matches_imported_class(
                        object_text,
                        resolved_imports,
                        &pattern.module_path,
                        class_name,
                    )
                }
                CallType::InstanceMethod => {
                    // TODO: requires variable type tracking.
                    continue;
                }
            };

            if matches {
                return Some(pattern);
            }
        }
        None
    }

    /// Match a direct call like `get_parameter(...)` from `from X import func`
    /// or wildcard imports.
    fn match_direct_call<'m>(
        &self,
        func_name: &str,
        resolved_imports: &HashMap<String, String>,
        model: &'m ExternalLibraryModel,
    ) -> Option<&'m CallPattern> {
        let canonical = resolved_imports.get(func_name)?;

        for pattern in &model.call_patterns {
            if pattern.function_name != func_name {
                continue;
            }

            let expected_canonical = format!("{}.{}", pattern.module_path, pattern.function_name);
            if canonical == &expected_canonical {
                return Some(pattern);
            }
        }
        None
    }

    /// Check if the object resolves to an imported module matching the pattern's module path.
    fn matches_imported_module(
        &self,
        object_text: &str,
        resolved_imports: &HashMap<String, String>,
        pattern_module_path: &str,
    ) -> bool {
        resolved_imports
            .get(object_text)
            .is_some_and(|canonical_path| canonical_path == pattern_module_path)
    }

    /// Check if the object resolves to an imported class from the expected module.
    ///
    /// Handles `from X.Y import ClassName` (canonical: `X.Y.ClassName`)
    /// and `from X.Y import ClassName as Alias` (alias maps to `X.Y.ClassName`).
    fn matches_imported_class(
        &self,
        object_text: &str,
        resolved_imports: &HashMap<String, String>,
        module_path: &str,
        class_name: &str,
    ) -> bool {
        let expected_canonical = format!("{module_path}.{class_name}");
        resolved_imports
            .get(object_text)
            .is_some_and(|canonical_path| canonical_path == &expected_canonical)
    }

    /// Convert a matched call pattern to `SdkMethodCall` entries.
    ///
    /// For each `SdkOperationMapping` in the matched pattern, produces one `SdkMethodCall`
    /// with:
    /// - `possible_services` set to `[mapping.service]`
    /// - `name` set to the operation converted from PascalCase to snake_case (Python convention)
    /// - `metadata` with the original source expression text, file path, and line number
    fn to_sdk_method_calls(
        call_pattern: &CallPattern,
        matched_node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
        source_file: &crate::SourceFile,
    ) -> Vec<SdkMethodCall> {
        call_pattern
            .sdk_operations
            .iter()
            .map(|mapping| {
                let method_name = ServiceDiscovery::operation_to_method_name(
                    &mapping.operation,
                    Language::Python,
                );

                let location = Location::from_node(source_file.path.clone(), matched_node);
                let metadata =
                    SdkMethodCallMetadata::new(matched_node.text().to_string(), location);

                SdkMethodCall {
                    name: method_name,
                    possible_services: vec![mapping.service.clone()],
                    metadata: Some(metadata),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::Python;
    use proptest::prelude::*;
    use rstest::rstest;
    use std::path::PathBuf;

    use crate::SourceFile;

    fn create_test_ast(source_code: &str) -> AstWithSourceFile<Python> {
        let source_file = SourceFile::with_language(
            PathBuf::new(),
            source_code.to_string(),
            crate::Language::Python,
        );
        let ast_grep = Python.ast_grep(&source_file.content);
        AstWithSourceFile::new(ast_grep, source_file)
    }

    #[rstest]
    #[case::from_x_import_y(
        "from aws_lambda_powertools.utilities import parameters",
        &[("parameters", "aws_lambda_powertools.utilities.parameters")],
        &[],
    )]
    #[case::from_x_import_y_as_z(
        "from aws_lambda_powertools.utilities import parameters as params",
        &[("params", "aws_lambda_powertools.utilities.parameters")],
        &[],
    )]
    #[case::from_x_y_import_func(
        "from aws_lambda_powertools.utilities.parameters import get_parameter",
        &[("get_parameter", "aws_lambda_powertools.utilities.parameters.get_parameter")],
        &[],
    )]
    #[case::import_x_y_as_z(
        "import aws_lambda_powertools.utilities.parameters as params",
        &[("params", "aws_lambda_powertools.utilities.parameters")],
        &[],
    )]
    #[case::import_x_y_no_alias(
        "import aws_lambda_powertools.utilities.parameters",
        &[("aws_lambda_powertools", "aws_lambda_powertools.utilities.parameters")],
        &[],
    )]
    #[case::multiple_imports(
        "from aws_lambda_powertools.utilities import parameters, idempotency",
        &[
            ("parameters", "aws_lambda_powertools.utilities.parameters"),
            ("idempotency", "aws_lambda_powertools.utilities.idempotency"),
        ],
        &[],
    )]
    #[case::parenthesized_imports(
        "from aws_lambda_powertools.utilities import (\n    parameters,\n    idempotency\n)",
        &[
            ("parameters", "aws_lambda_powertools.utilities.parameters"),
            ("idempotency", "aws_lambda_powertools.utilities.idempotency"),
        ],
        &[],
    )]
    #[case::no_imports(
        "x = 1\nprint(x)",
        &[],
        &[],
    )]
    #[case::wildcard_import(
        "from os import *",
        &[],
        &["os"],
    )]
    fn test_import_resolution(
        #[case] source: &str,
        #[case] expected_imports: &[(&str, &str)],
        #[case] expected_wildcards: &[&str],
    ) {
        let ast = create_test_ast(source);
        let resolved = resolve_imports(&ast);

        assert_eq!(resolved.imports.len(), expected_imports.len());
        for (local_name, canonical) in expected_imports {
            let actual = resolved
                .imports
                .get(*local_name)
                .unwrap_or_else(|| panic!("should have '{local_name}'"));
            assert_eq!(actual, canonical);
        }

        assert_eq!(resolved.wildcard_modules.len(), expected_wildcards.len());
        for expected in expected_wildcards {
            assert!(
                resolved.wildcard_modules.contains(&expected.to_string()),
                "wildcard_modules should contain '{expected}'",
            );
        }
    }

    /// Helper: load the built-in LibraryModelRegistry for Python.
    fn load_python_registry() -> LibraryModelRegistry {
        LibraryModelRegistry::load(crate::Language::Python)
            .expect("built-in Python registry should load successfully")
    }

    #[rstest]
    #[case::from_import(
        "from aws_lambda_powertools.utilities import parameters\nresult = parameters.get_parameter(\"/my/param\")\n",
        "get_parameter",
        "ssm",
        "parameters.get_parameter",
    )]
    #[case::get_secret(
        "from aws_lambda_powertools.utilities import parameters\nsecret = parameters.get_secret(\"/my/secret\")\n",
        "get_secret_value",
        "secretsmanager",
        "parameters.get_secret",
    )]
    #[case::aliased_from_import(
        "from aws_lambda_powertools.utilities import parameters as params\nresult = params.get_parameter(\"/my/param\")\n",
        "get_parameter",
        "ssm",
        "params.get_parameter",
    )]
    #[case::import_as_alias(
        "import aws_lambda_powertools.utilities.parameters as params\nresult = params.get_parameter(\"/my/param\")\n",
        "get_parameter",
        "ssm",
        "params.get_parameter",
    )]
    #[case::wildcard_import(
        "from aws_lambda_powertools.utilities.parameters import *\nresult = get_parameter(\"/my/param\")\n",
        "get_parameter",
        "ssm",
        "get_parameter",
    )]
    #[case::direct_function_import(
        "from aws_lambda_powertools.utilities.parameters import get_parameter\nresult = get_parameter(\"/my/param\")\n",
        "get_parameter",
        "ssm",
        "get_parameter",
    )]
    fn test_library_call_extraction(
        #[case] source: &str,
        #[case] expected_name: &str,
        #[case] expected_service: &str,
        #[case] expected_expr_substr: &str,
    ) {
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(results.len(), 1, "should produce exactly 1 SdkMethodCall");
        let call = &results[0];
        assert_eq!(call.name, expected_name);
        assert_eq!(call.possible_services, vec![expected_service]);
        assert!(call.metadata.is_some());
        let metadata = call.metadata.as_ref().unwrap();
        assert!(
            metadata.expr.contains(expected_expr_substr),
            "metadata expr should contain '{}', got: {}",
            expected_expr_substr,
            metadata.expr
        );
    }

    #[rstest]
    #[case::no_matching_imports("import os\nimport json\nresult = os.path.join(\"/a\", \"b\")\n")]
    #[case::matching_import_no_matching_call(
        "from aws_lambda_powertools.utilities import parameters\nx = parameters.some_other_function()\n",
    )]
    fn test_no_library_calls_detected(#[case] source: &str) {
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert!(
            results.is_empty(),
            "should produce zero results, got: {:?}",
            results
        );
    }

    #[rstest]
    #[case::direct_import(
        "from aws_lambda_powertools.utilities.parameters import SSMProvider\nresult = SSMProvider.get(\"/my/param\")\n",
    )]
    #[case::aliased_import(
        "from aws_lambda_powertools.utilities.parameters import SSMProvider as SSM\nresult = SSM.get(\"/my/param\")\n",
    )]
    fn test_static_method_call(#[case] source: &str) {
        let model = ExternalLibraryModel {
            library_name: "aws_lambda_powertools".to_string(),
            language: crate::Language::Python,
            version: None,
            call_patterns: vec![CallPattern {
                module_path: "aws_lambda_powertools.utilities.parameters".to_string(),
                class_name: Some("SSMProvider".to_string()),
                function_name: "get".to_string(),
                call_type: CallType::StaticMethod,
                sdk_operations: vec![
                    crate::extraction::external_library_models::SdkOperationMapping {
                        service: "ssm".to_string(),
                        operation: "GetParameter".to_string(),
                    },
                ],
            }],
        };
        let registry = LibraryModelRegistry::from_models(vec![model]);
        let ast = create_test_ast(source);
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(results.len(), 1, "static method call should be detected");
        let call = &results[0];
        assert_eq!(call.name, "get_parameter");
        assert_eq!(call.possible_services, vec!["ssm"]);
    }

    // ---- Property-based tests ----

    fn arb_python_identifier() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]{0,15}".prop_filter("must not be Python keyword", |s| {
            !matches!(
                s.as_str(),
                "from"
                    | "import"
                    | "as"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "def"
                    | "class"
                    | "return"
                    | "try"
                    | "except"
                    | "with"
                    | "in"
                    | "not"
                    | "and"
                    | "or"
                    | "is"
                    | "None"
                    | "True"
                    | "False"
                    | "pass"
                    | "break"
                    | "continue"
                    | "raise"
                    | "yield"
                    | "del"
                    | "global"
                    | "nonlocal"
                    | "assert"
                    | "lambda"
                    | "finally"
                    | "elif"
            )
        })
    }

    fn arb_module_path() -> impl Strategy<Value = String> {
        proptest::collection::vec(arb_python_identifier(), 2..=4)
            .prop_map(|segments| segments.join("."))
    }

    fn arb_pascal_case_operation() -> impl Strategy<Value = String> {
        proptest::collection::vec("[A-Z][a-z]{2,8}", 1..=3).prop_map(|words| words.join(""))
    }

    fn arb_sdk_operations(
    ) -> impl Strategy<Value = Vec<crate::extraction::external_library_models::SdkOperationMapping>>
    {
        proptest::collection::vec(
            (arb_python_identifier(), arb_pascal_case_operation()).prop_map(
                |(service, operation)| {
                    crate::extraction::external_library_models::SdkOperationMapping {
                        service,
                        operation,
                    }
                },
            ),
            1..=3,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_library_call_detection_produces_correct_sdk_method_calls(
            module_path in arb_module_path(),
            function_name in arb_python_identifier(),
            alias_name in arb_python_identifier(),
            sdk_operations in arb_sdk_operations(),
        ) {
            let segments: Vec<&str> = module_path.split('.').collect();
            prop_assume!(segments.len() >= 2);
            prop_assume!(alias_name != function_name);

            let last_dot = module_path.rfind('.').unwrap();
            let module_parent = &module_path[..last_dot];
            let module_name = &module_path[last_dot + 1..];
            prop_assume!(alias_name != module_name);

            let model = ExternalLibraryModel {
                library_name: format!("test_lib_{}", module_name),
                language: crate::Language::Python,
                version: None,
                call_patterns: vec![CallPattern {
                    module_path: module_path.clone(),
                    class_name: None,
                    function_name: function_name.clone(),
                    call_type: CallType::Function,
                    sdk_operations: sdk_operations.clone(),
                }],
            };

            let registry = LibraryModelRegistry::from_models(vec![model]);

            let source_code = format!(
                "from {} import {} as {}\n{}.{}()\n",
                module_parent, module_name, alias_name, alias_name, function_name
            );

            let file_path = PathBuf::from("test_file.py");
            let source_file = SourceFile::with_language(
                file_path.clone(),
                source_code.clone(),
                crate::Language::Python,
            );
            let ast_with_path = AstWithSourceFile::new(
                ast_grep_language::Python.ast_grep(&source_file.content),
                source_file,
            );

            let extractor = LibraryCallExtractor::new(&registry);
            let results = extractor.extract_library_method_calls(&ast_with_path);

            let expected_count = sdk_operations.len();
            prop_assert_eq!(
                results.len(),
                expected_count,
                "Expected {} SdkMethodCall entries but got {}. Source:\n{}",
                expected_count,
                results.len(),
                source_code
            );

            for (i, result) in results.iter().enumerate() {
                let expected_op = &sdk_operations[i];

                prop_assert_eq!(
                    &result.possible_services,
                    &vec![expected_op.service.clone()],
                    "SdkMethodCall[{}] should have service '{}' but got {:?}",
                    i,
                    expected_op.service,
                    result.possible_services
                );

                let expected_name =
                    ServiceDiscovery::operation_to_method_name(&expected_op.operation, crate::Language::Python);
                prop_assert_eq!(
                    &result.name,
                    &expected_name,
                    "SdkMethodCall[{}] should have name '{}' but got '{}'",
                    i,
                    expected_name,
                    result.name
                );

                let metadata = result.metadata.as_ref();
                prop_assert!(
                    metadata.is_some(),
                    "SdkMethodCall[{}] should have metadata",
                    i
                );
                let metadata = metadata.unwrap();

                let expected_call_expr = format!("{}.{}()", alias_name, function_name);
                prop_assert_eq!(
                    &metadata.expr,
                    &expected_call_expr,
                    "SdkMethodCall[{}] metadata expr should be '{}' but got '{}'",
                    i,
                    expected_call_expr,
                    metadata.expr
                );

                prop_assert_eq!(
                    &metadata.location.file_path,
                    &file_path,
                    "SdkMethodCall[{}] metadata file_path should be '{:?}' but got '{:?}'",
                    i,
                    file_path,
                    metadata.location.file_path
                );

                // All results come from the same call site (line 2), since one call
                // pattern can map to multiple SdkOperationMappings.
                prop_assert_eq!(
                    metadata.location.start_position.0,
                    2,
                    "SdkMethodCall[{}] metadata line should be 2 (call is on second line) but got {}",
                    i,
                    metadata.location.start_position.0
                );
            }
        }
    }
}
