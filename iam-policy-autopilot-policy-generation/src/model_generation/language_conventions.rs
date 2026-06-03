use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::extraction::call_graph::FunctionNode;

/// Parsed components of a function name.
pub(crate) struct ParsedFunctionName {
    pub module_path: String,
    pub class_name: Option<String>,
    pub function_name: String,
}

/// Language-specific conventions for interpreting function nodes.
pub(crate) trait LanguageConventions {
    /// Whether a function is visible outside its package/module.
    fn is_exported(&self, node: &FunctionNode) -> bool;

    /// Parse the node's name into structured components.
    fn parse_function_name(&self, node: &FunctionNode) -> ParsedFunctionName;

    /// Detect the workspace/project root from source file paths.
    fn detect_workspace_root(&self, source_files: &[PathBuf]) -> Result<PathBuf>;
}

/// Go language conventions.
///
/// - Exported: function/method name starts with uppercase (enforced by compiler).
/// - Naming: gopls uses `"FuncName"` for functions, `"(*Type).Method"` for methods.
/// - Workspace root: directory containing `go.mod`.
pub(crate) struct GoConventions;

impl LanguageConventions for GoConventions {
    fn is_exported(&self, node: &FunctionNode) -> bool {
        let method_name = match node.name.find(").") {
            Some(pos) => &node.name[pos + 2..],
            None => &node.name,
        };
        method_name.starts_with(|c: char| c.is_uppercase())
    }

    fn parse_function_name(&self, node: &FunctionNode) -> ParsedFunctionName {
        let name = &node.name;

        if let Some(dot_pos) = name.find(").") {
            let receiver_part = &name[..=dot_pos];
            let method_name = &name[dot_pos + 2..];

            let type_name = receiver_part
                .trim_start_matches('(')
                .trim_start_matches('*')
                .trim_end_matches(')');

            return ParsedFunctionName {
                module_path: String::new(),
                class_name: Some(type_name.to_string()),
                function_name: method_name.to_string(),
            };
        }

        ParsedFunctionName {
            module_path: String::new(),
            class_name: None,
            function_name: name.to_string(),
        }
    }

    fn detect_workspace_root(&self, source_files: &[PathBuf]) -> Result<PathBuf> {
        let start = source_files
            .first()
            .and_then(|f| f.parent())
            .context("No source files provided")?;

        let mut dir = start;
        loop {
            if dir.join("go.mod").exists() {
                return Ok(dir.to_path_buf());
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }

        Ok(start.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Location;
    use rstest::rstest;
    use std::path::PathBuf;

    fn node(name: &str) -> FunctionNode {
        FunctionNode {
            name: name.to_string(),
            qualified_name: None,
            location: Location::new(PathBuf::from("test.go"), (1, 1), (10, 1)),
        }
    }

    #[rstest]
    #[case("HandleRequest", true)]
    #[case("GetObject", true)]
    #[case("fetchData", false)]
    #[case("helper", false)]
    #[case("(*Server).HandleRequest", true)]
    #[case("(*Server).fetchData", false)]
    #[case("(*server).Export", true)]
    fn test_go_is_exported(#[case] name: &str, #[case] expected: bool) {
        let conventions = GoConventions;
        assert_eq!(conventions.is_exported(&node(name)), expected);
    }

    #[rstest]
    #[case("main", "", None, "main")]
    #[case("helper", "", None, "helper")]
    #[case("(*Server).HandleRequest", "", Some("Server"), "HandleRequest")]
    #[case("(*Server).fetchData", "", Some("Server"), "fetchData")]
    #[case("(Server).Method", "", Some("Server"), "Method")]
    fn test_go_parse_function_name(
        #[case] name: &str,
        #[case] expected_module: &str,
        #[case] expected_class: Option<&str>,
        #[case] expected_func: &str,
    ) {
        let conventions = GoConventions;
        let parsed = conventions.parse_function_name(&node(name));
        assert_eq!(parsed.module_path, expected_module);
        assert_eq!(parsed.class_name.as_deref(), expected_class);
        assert_eq!(parsed.function_name, expected_func);
    }
}
