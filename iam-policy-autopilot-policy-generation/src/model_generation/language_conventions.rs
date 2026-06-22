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
///
/// Requires `Send + Sync` so that a `Box<dyn LanguageConventions>` (and shared
/// `&dyn` references to it) can be held across `.await` points in async model
/// generation without making the resulting future non-`Send`.
pub(crate) trait LanguageConventions: Send + Sync {
    /// Whether a function is visible outside its package/module.
    fn is_exported(&self, node: &FunctionNode) -> bool;

    /// Parse the node's name into structured components.
    fn parse_function_name(&self, node: &FunctionNode) -> ParsedFunctionName;

    /// Detect the workspace/project root from source file paths.
    fn detect_workspace_root(&self, source_files: &[PathBuf]) -> Result<PathBuf>;

    /// Resolve a language-specific symbol spec to a single call-graph node.
    ///
    /// The syntax of `spec` is defined by the language: for Go it is
    /// `pkg.func` (e.g. `s3.resourceBucketCreate`); other languages may use
    /// fully-qualified class names, dotted module paths, etc. Implementations
    /// must error if the spec matches zero or more than one node.
    fn resolve_symbol<'a>(
        &self,
        spec: &str,
        nodes: &'a [FunctionNode],
    ) -> Result<&'a FunctionNode>;
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
            function_name: name.clone(),
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

    fn resolve_symbol<'a>(
        &self,
        spec: &str,
        nodes: &'a [FunctionNode],
    ) -> Result<&'a FunctionNode> {
        let (pkg, func) = parse_go_symbol(spec)?;

        // Match the bare function name, then disambiguate cross-package
        // collisions (e.g. `resourceInstanceRead` in both ec2 and rds) by
        // requiring the node's file to live in a directory named `pkg`.
        let matches: Vec<&FunctionNode> = nodes
            .iter()
            .filter(|n| n.name == func && file_in_package_dir(&n.location.file_path, pkg))
            .collect();

        match matches.as_slice() {
            [node] => Ok(node),
            [] => {
                let same_name: Vec<String> = nodes
                    .iter()
                    .filter(|n| n.name == func)
                    .map(|n| n.location.file_path.display().to_string())
                    .collect();
                if same_name.is_empty() {
                    anyhow::bail!(
                        "No function named '{func}' found in the call graph for symbol '{spec}'"
                    )
                }
                anyhow::bail!(
                    "Function '{func}' exists but not in package '{pkg}' for symbol '{spec}'. \
                     Found in: {same_name:?}"
                )
            }
            multiple => {
                let locations: Vec<String> = multiple
                    .iter()
                    .map(|n| n.location.file_path.display().to_string())
                    .collect();
                anyhow::bail!(
                    "Ambiguous symbol '{spec}': '{func}' matches multiple nodes in package \
                     '{pkg}': {locations:?}"
                )
            }
        }
    }
}

/// Split a Go symbol spec into `(package, function)`.
///
/// Accepts the short form `pkg.func` (`s3.resourceBucketCreate`) as well as the
/// fully-qualified import-path form produced by reflection
/// (`github.com/.../internal/service/s3.resourceBucketCreate`) — in both cases
/// `pkg` is the last path segment before the final `.`.
fn parse_go_symbol(spec: &str) -> Result<(&str, &str)> {
    let (path, func) = spec.rsplit_once('.').with_context(|| {
        format!("Invalid Go symbol '{spec}', expected 'pkg.func' (e.g. s3.resourceBucketCreate)")
    })?;
    if func.is_empty() {
        anyhow::bail!("Invalid Go symbol '{spec}': empty function name");
    }
    let pkg = path.rsplit('/').next().unwrap_or(path);
    if pkg.is_empty() {
        anyhow::bail!("Invalid Go symbol '{spec}': empty package name");
    }
    Ok((pkg, func))
}

/// Whether `file` lives directly in a directory named `pkg` (Go's package =
/// directory rule).
fn file_in_package_dir(file: &std::path::Path, pkg: &str) -> bool {
    file.parent()
        .and_then(|d| d.file_name())
        .map(|n| n == pkg)
        .unwrap_or(false)
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

    fn node_at(name: &str, file: &str) -> FunctionNode {
        FunctionNode {
            name: name.to_string(),
            qualified_name: None,
            location: Location::new(PathBuf::from(file), (1, 1), (10, 1)),
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

    #[rstest]
    #[case("s3.resourceBucketCreate", "s3", "resourceBucketCreate")]
    // Full import-path form (as emitted by reflection) collapses to the last segment.
    #[case(
        "github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketCreate",
        "s3",
        "resourceBucketCreate"
    )]
    fn test_parse_go_symbol_ok(
        #[case] spec: &str,
        #[case] pkg: &str,
        #[case] func: &str,
    ) {
        assert_eq!(parse_go_symbol(spec).unwrap(), (pkg, func));
    }

    #[rstest]
    #[case("noseparator")]
    #[case("s3.")]
    #[case(".func")]
    fn test_parse_go_symbol_err(#[case] spec: &str) {
        assert!(parse_go_symbol(spec).is_err());
    }

    #[test]
    fn test_resolve_symbol_disambiguates_colliding_short_names() {
        // resourceInstanceRead exists in both ec2 and rds in the real provider.
        let nodes = vec![
            node_at("resourceInstanceRead", "internal/service/ec2/instance.go"),
            node_at("resourceInstanceRead", "internal/service/rds/instance.go"),
        ];
        let conv = GoConventions;

        let ec2 = conv.resolve_symbol("ec2.resourceInstanceRead", &nodes).unwrap();
        assert!(ec2.location.file_path.ends_with("ec2/instance.go"));

        let rds = conv.resolve_symbol("rds.resourceInstanceRead", &nodes).unwrap();
        assert!(rds.location.file_path.ends_with("rds/instance.go"));
    }

    #[test]
    fn test_resolve_symbol_accepts_full_import_path() {
        let nodes = vec![node_at(
            "resourceBucketCreate",
            "internal/service/s3/bucket.go",
        )];
        let conv = GoConventions;
        let n = conv
            .resolve_symbol(
                "github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketCreate",
                &nodes,
            )
            .unwrap();
        assert_eq!(n.name, "resourceBucketCreate");
    }

    #[test]
    fn test_resolve_symbol_no_match() {
        let nodes = vec![node_at("resourceBucketRead", "internal/service/s3/bucket.go")];
        let conv = GoConventions;

        // Function name not present at all.
        assert!(conv.resolve_symbol("s3.resourceBucketWrite", &nodes).is_err());
        // Right function, wrong package.
        assert!(conv.resolve_symbol("ec2.resourceBucketRead", &nodes).is_err());
    }
}
