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

/// Resolve Python import statements from the AST into a mapping from local
/// names to canonical module paths.
///
/// Returns a `HashMap<String, String>` where the key is the local name used
/// in code and the value is the canonical dotted module path.
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
pub(crate) fn resolve_imports(ast: &AstWithSourceFile<Python>) -> HashMap<String, String> {
    let mut imports: HashMap<String, String> = HashMap::new();
    let root = ast.ast.root();

    for node_match in root.find_all("from $MODULE import $$$NAMES") {
        resolve_import_from_statement(node_match.get_node(), &mut imports);
    }

    for node_match in root.find_all("import $$$MODULES") {
        resolve_import_statement(node_match.get_node(), &mut imports);
    }

    imports
}

/// Resolve a `from X import Y [as Z], ...` statement.
fn resolve_import_from_statement(
    node: &ast_grep_core::Node<ast_grep_core::tree_sitter::StrDoc<Python>>,
    imports: &mut HashMap<String, String>,
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
                // TODO: wildcard imports (`from X import *`) could be resolved
                // using the loaded model's function names for the module path.
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
        let resolved_imports = resolve_imports(ast);

        if resolved_imports.is_empty() {
            return Vec::new();
        }

        let models = self.registry.models_for_language(Language::Python);
        let mut all_calls = Vec::new();

        for model in models {
            let calls = self.match_call_patterns(ast, &resolved_imports, model);
            all_calls.extend(calls);
        }

        all_calls
    }

    /// Match call patterns from a model against `$OBJ.$METHOD($$$ARGS)` call sites.
    fn match_call_patterns(
        &self,
        ast: &AstWithSourceFile<Python>,
        resolved_imports: &HashMap<String, String>,
        model: &ExternalLibraryModel,
    ) -> Vec<SdkMethodCall> {
        let mut results = Vec::new();
        let root = ast.ast.root();

        // Use the same find_all pattern as PythonExtractor::parse()
        let pattern = "$OBJ.$METHOD($$$ARGS)";

        for node_match in root.find_all(pattern) {
            let env = node_match.get_env();

            // Extract the object and method name from the match environment
            let object_node = match env.get_match("OBJ") {
                Some(node) => node,
                None => continue,
            };
            let method_node = match env.get_match("METHOD") {
                Some(node) => node,
                None => continue,
            };

            let object_text = object_node.text().to_string();
            let method_name = method_node.text().to_string();

            for pattern in &model.call_patterns {
                if pattern.function_name != method_name {
                    continue;
                }

                // Check if the object matches a resolved import for this pattern's module_path
                let matches_import = match pattern.call_type {
                    CallType::ModuleLevel => self.matches_module_level_import(
                        &object_text,
                        resolved_imports,
                        &pattern.module_path,
                    ),
                    CallType::InstanceMethod => {
                        // TODO: InstanceMethod requires variable type tracking to
                        // determine that e.g. `provider = SSMProvider()` makes
                        // `provider.get(...)` an SSM call. Without it, any
                        // `xxx.get(...)` would match if the module is imported.
                        // Skip until variable type tracking is available.
                        continue;
                    }
                };

                if !matches_import {
                    continue;
                }

                // First matching pattern wins
                let matched_node = node_match.get_node();
                let calls = Self::to_sdk_method_calls(pattern, matched_node, &ast.source_file);
                results.extend(calls);
                break;
            }
        }

        results
    }

    /// Check if the object resolves to a module-level import matching the pattern's module path.
    fn matches_module_level_import(
        &self,
        object_text: &str,
        resolved_imports: &HashMap<String, String>,
        pattern_module_path: &str,
    ) -> bool {
        if let Some(canonical_path) = resolved_imports.get(object_text) {
            // The canonical path should match the pattern's module_path exactly,
            // or the canonical path should be a prefix that the pattern's module_path
            // starts with (for cases like `import X.Y` where local name is `X`
            // and canonical is `X.Y`, matching pattern module_path `X.Y`).
            canonical_path == pattern_module_path
                || pattern_module_path.starts_with(&format!("{canonical_path}."))
        } else {
            false
        }
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

    #[test]
    fn test_from_x_import_y() {
        let ast = create_test_ast("from aws_lambda_powertools.utilities import parameters");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 1);
        let canonical = imports.get("parameters").expect("should have 'parameters'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.parameters");
    }

    #[test]
    fn test_from_x_import_y_as_z() {
        let ast =
            create_test_ast("from aws_lambda_powertools.utilities import parameters as params");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 1);
        let canonical = imports.get("params").expect("should have 'params'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.parameters");
    }

    #[test]
    fn test_from_x_y_import_func() {
        let ast =
            create_test_ast("from aws_lambda_powertools.utilities.parameters import get_parameter");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 1);
        let canonical = imports
            .get("get_parameter")
            .expect("should have 'get_parameter'");
        assert_eq!(
            canonical,
            "aws_lambda_powertools.utilities.parameters.get_parameter"
        );
    }

    #[test]
    fn test_import_x_y_as_z() {
        let ast = create_test_ast("import aws_lambda_powertools.utilities.parameters as params");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 1);
        let canonical = imports.get("params").expect("should have 'params'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.parameters");
    }

    #[test]
    fn test_import_x_y_no_alias() {
        let ast = create_test_ast("import aws_lambda_powertools.utilities.parameters");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 1);
        let canonical = imports
            .get("aws_lambda_powertools")
            .expect("should have 'aws_lambda_powertools'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.parameters");
    }

    #[test]
    fn test_from_x_import_multiple() {
        let ast =
            create_test_ast("from aws_lambda_powertools.utilities import parameters, idempotency");
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 2);

        let canonical = imports.get("parameters").expect("should have 'parameters'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.parameters");

        let canonical = imports
            .get("idempotency")
            .expect("should have 'idempotency'");
        assert_eq!(canonical, "aws_lambda_powertools.utilities.idempotency");
    }

    #[test]
    fn test_no_imports() {
        let ast = create_test_ast("x = 1\nprint(x)");
        let imports = resolve_imports(&ast);
        assert!(imports.is_empty());
    }

    #[test]
    fn test_wildcard_import_is_skipped() {
        let ast = create_test_ast("from os import *");
        let imports = resolve_imports(&ast);
        // Wildcard imports are skipped for now (see TODO in resolve_import_from_statement)
        assert!(imports.is_empty());
    }

    #[test]
    fn test_parenthesized_from_import() {
        let source = r#"
from aws_lambda_powertools.utilities import (
    parameters,
    idempotency
)
"#;
        let ast = create_test_ast(source);
        let imports = resolve_imports(&ast);

        assert_eq!(imports.len(), 2);
        assert!(imports.contains_key("parameters"));
        assert!(imports.contains_key("idempotency"));
    }

    // ---- Property-based test strategies ----

    /// Generate a valid Python identifier: starts with a letter, followed by letters/digits/underscores.
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

    /// Generate a dotted module path like `abc.def.ghi` with 2-4 segments.
    fn arb_module_path() -> impl Strategy<Value = String> {
        proptest::collection::vec(arb_python_identifier(), 2..=4)
            .prop_map(|segments| segments.join("."))
    }

    // Feature: external-library-models, Property 5: Library call detection produces correct SdkMethodCalls

    /// Generate a PascalCase operation name like `GetParameter` or `CreateBucket`.
    fn arb_pascal_case_operation() -> impl Strategy<Value = String> {
        // Generate 1-3 capitalized words and concatenate them
        proptest::collection::vec("[A-Z][a-z]{2,8}", 1..=3).prop_map(|words| words.join(""))
    }

    /// Generate a non-empty vec of SdkOperationMapping with valid service and PascalCase operation.
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

        /// For any source file that imports a module matching a loaded model and
        /// contains a function call matching a CallPattern, the LibraryCallExtractor
        /// produces exactly N SdkMethodCall entries (where N = number of sdk_operations
        /// in the matched pattern), each with correct service, operation name, and metadata.
        #[test]
        fn prop_library_call_detection_produces_correct_sdk_method_calls(
            module_path in arb_module_path(),
            function_name in arb_python_identifier(),
            alias_name in arb_python_identifier(),
            sdk_operations in arb_sdk_operations(),
        ) {
            // The module_path must have at least 2 segments so we can split into parent + module_name
            let segments: Vec<&str> = module_path.split('.').collect();
            prop_assume!(segments.len() >= 2);

            // Ensure alias differs from function_name to avoid ambiguity
            prop_assume!(alias_name != function_name);

            // Split module_path into parent (for `from` clause) and last component (the module name)
            let last_dot = module_path.rfind('.').unwrap();
            let module_parent = &module_path[..last_dot];
            let module_name = &module_path[last_dot + 1..];

            // Ensure alias differs from module_name to make the test more interesting
            prop_assume!(alias_name != module_name);

            // Build the model with a single call pattern
            let model = ExternalLibraryModel {
                library_name: format!("test_lib_{}", module_name),
                language: crate::Language::Python,
                version: None,
                call_patterns: vec![CallPattern {
                    module_path: module_path.clone(),
                    function_name: function_name.clone(),
                    call_type: CallType::ModuleLevel,
                    sdk_operations: sdk_operations.clone(),
                }],
            };

            // Build a LibraryModelRegistry with this model
            let registry = LibraryModelRegistry::from_models(vec![model]);

            // Construct Python source code:
            //   from <module_parent> import <module_name> as <alias_name>
            //   <alias_name>.<function_name>()
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

            // Run extraction
            let extractor = LibraryCallExtractor::new(&registry);
            let results = extractor.extract_library_method_calls(&ast_with_path);

            // Assert exactly N SdkMethodCall entries (N = number of sdk_operations)
            let expected_count = sdk_operations.len();
            prop_assert_eq!(
                results.len(),
                expected_count,
                "Expected {} SdkMethodCall entries but got {}. Source:\n{}",
                expected_count,
                results.len(),
                source_code
            );

            // Assert each SdkMethodCall has correct service, operation name, and metadata
            for (i, result) in results.iter().enumerate() {
                let expected_op = &sdk_operations[i];

                // Check possible_services contains the correct service
                prop_assert_eq!(
                    &result.possible_services,
                    &vec![expected_op.service.clone()],
                    "SdkMethodCall[{}] should have service '{}' but got {:?}",
                    i,
                    expected_op.service,
                    result.possible_services
                );

                // Check operation name is converted from PascalCase to snake_case
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

                // Check metadata is present
                let metadata = result.metadata.as_ref();
                prop_assert!(
                    metadata.is_some(),
                    "SdkMethodCall[{}] should have metadata",
                    i
                );
                let metadata = metadata.unwrap();

                // Check metadata contains the source expression (the call text)
                let expected_call_expr = format!("{}.{}()", alias_name, function_name);
                prop_assert_eq!(
                    &metadata.expr,
                    &expected_call_expr,
                    "SdkMethodCall[{}] metadata expr should be '{}' but got '{}'",
                    i,
                    expected_call_expr,
                    metadata.expr
                );

                // Check metadata has the correct file path
                prop_assert_eq!(
                    &metadata.location.file_path,
                    &file_path,
                    "SdkMethodCall[{}] metadata file_path should be '{:?}' but got '{:?}'",
                    i,
                    file_path,
                    metadata.location.file_path
                );

                // Check metadata has a valid line number (line 2, since import is line 1)
                prop_assert!(
                    metadata.location.start_position.0 > 0,
                    "SdkMethodCall[{}] metadata line number should be > 0 but got {}",
                    i,
                    metadata.location.start_position.0
                );
                // The call is on line 2 (1-based)
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

    // ---- Unit tests for library call extraction ----

    /// Helper: load the built-in LibraryModelRegistry for Python.
    fn load_python_registry() -> LibraryModelRegistry {
        LibraryModelRegistry::load(crate::Language::Python)
            .expect("built-in Python registry should load successfully")
    }

    #[test]
    fn test_get_parameter_with_from_import_produces_ssm_sdk_call() {
        let source = r#"
from aws_lambda_powertools.utilities import parameters

result = parameters.get_parameter("/my/param")
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(results.len(), 1, "should produce exactly 1 SdkMethodCall");
        let call = &results[0];
        assert_eq!(call.name, "get_parameter");
        assert_eq!(call.possible_services, vec!["ssm"]);
        assert!(call.metadata.is_some());
        let metadata = call.metadata.as_ref().unwrap();
        assert!(
            metadata.expr.contains("parameters.get_parameter"),
            "metadata expr should contain the call expression, got: {}",
            metadata.expr
        );
    }

    #[test]
    fn test_get_secret_produces_secretsmanager_sdk_call() {
        let source = r#"
from aws_lambda_powertools.utilities import parameters

secret = parameters.get_secret("/my/secret")
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(results.len(), 1, "should produce exactly 1 SdkMethodCall");
        let call = &results[0];
        assert_eq!(call.name, "get_secret_value");
        assert_eq!(call.possible_services, vec!["secretsmanager"]);
        assert!(call.metadata.is_some());
        let metadata = call.metadata.as_ref().unwrap();
        assert!(
            metadata.expr.contains("parameters.get_secret"),
            "metadata expr should contain the call expression, got: {}",
            metadata.expr
        );
    }

    #[test]
    fn test_aliased_from_import_detected() {
        let source = r#"
from aws_lambda_powertools.utilities import parameters as params

result = params.get_parameter("/my/param")
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(results.len(), 1, "aliased import should still be detected");
        let call = &results[0];
        assert_eq!(call.name, "get_parameter");
        assert_eq!(call.possible_services, vec!["ssm"]);
        assert!(call.metadata.is_some());
        let metadata = call.metadata.as_ref().unwrap();
        assert!(
            metadata.expr.contains("params.get_parameter"),
            "metadata expr should contain the aliased call expression, got: {}",
            metadata.expr
        );
    }

    #[test]
    fn test_import_as_alias_detected() {
        let source = r#"
import aws_lambda_powertools.utilities.parameters as params

result = params.get_parameter("/my/param")
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert_eq!(
            results.len(),
            1,
            "import ... as alias should still be detected"
        );
        let call = &results[0];
        assert_eq!(call.name, "get_parameter");
        assert_eq!(call.possible_services, vec!["ssm"]);
        assert!(call.metadata.is_some());
        let metadata = call.metadata.as_ref().unwrap();
        assert!(
            metadata.expr.contains("params.get_parameter"),
            "metadata expr should contain the aliased call expression, got: {}",
            metadata.expr
        );
    }

    #[test]
    fn test_no_matching_imports_produces_zero_results() {
        let source = r#"
import os
import json

result = os.path.join("/a", "b")
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert!(
            results.is_empty(),
            "source with no matching imports should produce zero results, got: {:?}",
            results
        );
    }

    #[test]
    fn test_matching_import_but_no_matching_call_produces_zero_results() {
        let source = r#"
from aws_lambda_powertools.utilities import parameters

# Import is present but no matching function call is made
x = parameters.some_other_function()
y = 42
"#;
        let ast = create_test_ast(source);
        let registry = load_python_registry();
        let extractor = LibraryCallExtractor::new(&registry);
        let results = extractor.extract_library_method_calls(&ast);

        assert!(
            results.is_empty(),
            "matching import but no matching call should produce zero results, got: {:?}",
            results
        );
    }
}
