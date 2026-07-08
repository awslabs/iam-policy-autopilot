use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::extraction::call_graph::FunctionNode;
use crate::extraction::external_library_models::CallPatternKey;
use crate::extraction::go::naming::split_receiver;

/// Language-specific conventions for interpreting function nodes.
///
/// Requires `Send + Sync` so that a `Box<dyn LanguageConventions>` and shared
/// `&dyn` references can be held across `.await` points in async model
/// generation without making the resulting future non-`Send`.
pub(crate) trait LanguageConventions: Send + Sync {
    /// Whether a function is visible outside its package/module.
    fn is_exported(&self, node: &FunctionNode) -> bool;

    /// Parse the node's name into the [`CallPatternKey`] the model is keyed by.
    fn parse_function_name(&self, node: &FunctionNode) -> CallPatternKey;

    /// Detect the workspace/project root from source file paths.
    fn detect_workspace_root(&self, source_files: &[PathBuf]) -> Result<PathBuf>;

    /// Resolve a language-specific symbol spec to a single call-graph node.
    ///
    /// The syntax of `spec` is defined by the language: for Go it is
    /// `pkg.func` (e.g. `s3.resourceBucketCreate`); other languages may use
    /// fully-qualified class names, dotted module paths, etc. Implementations
    /// must error if the spec matches zero or more than one node.
    fn resolve_symbol<'a>(&self, spec: &str, nodes: &'a [FunctionNode])
        -> Result<&'a FunctionNode>;
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

    fn parse_function_name(&self, node: &FunctionNode) -> CallPatternKey {
        // In Go, package == the directory containing the source file. gopls does
        // not put the package on the symbol name, so recover it from the path.
        // This makes (module_path, function_name) a provider-wide unique key,
        // which matters when merging per-service models (the short function name
        // alone collides, e.g. resourceInstanceRead in both ec2 and rds).
        let module_path = go_package_of(&node.location.file_path).unwrap_or_default();

        // gopls already gives a clean node name (no runtime `-fm`/`.funcN`
        // decoration), so only the receiver split is needed — shared with the
        // Terraform consumer so both derive the same key.
        let (class_name, function_name) = split_receiver(&node.name);
        CallPatternKey {
            module_path,
            class_name,
            function_name,
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
/// (`github.com/.../internal/service/s3.resourceBucketCreate`).
///
/// The package is the path segment after the last `/`, up to the FIRST `.`.
/// Everything after that first `.` is the function — which may itself contain
/// dots and a receiver for methods, e.g. `sqs.(*queueAttributeHandler).Upsert`
/// → pkg `sqs`, func `(*queueAttributeHandler).Upsert`. gopls names method
/// nodes the same way (`(*Type).Method`), so the func string matches directly.
fn parse_go_symbol(spec: &str) -> Result<(&str, &str)> {
    // Drop any import-path prefix; keep the final path segment (`pkg.func...`).
    let segment = spec.rsplit('/').next().unwrap_or(spec);
    let (pkg, func) = segment.split_once('.').with_context(|| {
        format!("Invalid Go symbol '{spec}', expected 'pkg.func' (e.g. s3.resourceBucketCreate)")
    })?;
    if pkg.is_empty() {
        anyhow::bail!("Invalid Go symbol '{spec}': empty package name");
    }
    if func.is_empty() {
        anyhow::bail!("Invalid Go symbol '{spec}': empty function name");
    }
    Ok((pkg, func))
}

/// The Go package a file belongs to (its parent directory name), per Go's
/// package = directory rule. Returns `None` if the path has no usable parent.
fn go_package_of(file: &std::path::Path) -> Option<String> {
    file.parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// Whether `file` lives directly in a directory named `pkg` (Go's package =
/// directory rule).
fn file_in_package_dir(file: &std::path::Path, pkg: &str) -> bool {
    go_package_of(file).as_deref() == Some(pkg)
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

    // module_path is recovered from the file's parent directory (the Go package).
    #[rstest]
    #[case("main", "internal/service/s3/bucket.go", "s3", None, "main")]
    #[case(
        "resourceBucketCreate",
        "internal/service/s3/bucket.go",
        "s3",
        None,
        "resourceBucketCreate"
    )]
    #[case(
        "(*Server).HandleRequest",
        "internal/service/ec2/server.go",
        "ec2",
        Some("Server"),
        "HandleRequest"
    )]
    #[case(
        "(*Server).fetchData",
        "internal/service/ec2/server.go",
        "ec2",
        Some("Server"),
        "fetchData"
    )]
    #[case("(Server).Method", "rds/instance.go", "rds", Some("Server"), "Method")]
    // Bare filename with no parent directory => empty package.
    #[case("helper", "helper.go", "", None, "helper")]
    fn test_go_parse_function_name(
        #[case] name: &str,
        #[case] file: &str,
        #[case] expected_module: &str,
        #[case] expected_class: Option<&str>,
        #[case] expected_func: &str,
    ) {
        let conventions = GoConventions;
        let parsed = conventions.parse_function_name(&node_at(name, file));
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
    // Method form: package splits at the FIRST dot; the receiver+method (which
    // contains a dot) stays intact as the function part.
    #[case(
        "sqs.(*queueAttributeHandler).Upsert",
        "sqs",
        "(*queueAttributeHandler).Upsert"
    )]
    #[case(
        "github.com/hashicorp/terraform-provider-aws/internal/service/sqs.(*queueAttributeHandler).Read",
        "sqs",
        "(*queueAttributeHandler).Read"
    )]
    fn test_parse_go_symbol_ok(#[case] spec: &str, #[case] pkg: &str, #[case] func: &str) {
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

        let ec2 = conv
            .resolve_symbol("ec2.resourceInstanceRead", &nodes)
            .unwrap();
        assert!(ec2.location.file_path.ends_with("ec2/instance.go"));

        let rds = conv
            .resolve_symbol("rds.resourceInstanceRead", &nodes)
            .unwrap();
        assert!(rds.location.file_path.ends_with("rds/instance.go"));
    }

    #[test]
    fn test_resolve_symbol_method_form() {
        // gopls names method nodes "(*Type).Method"; a method-value handler
        // symbol must resolve to that node.
        let nodes = vec![
            node_at(
                "(*queueAttributeHandler).Upsert",
                "internal/service/sqs/queue.go",
            ),
            node_at("resourceQueueCreate", "internal/service/sqs/queue.go"),
        ];
        let conv = GoConventions;
        let n = conv
            .resolve_symbol("sqs.(*queueAttributeHandler).Upsert", &nodes)
            .unwrap();
        assert_eq!(n.name, "(*queueAttributeHandler).Upsert");
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
        let nodes = vec![node_at(
            "resourceBucketRead",
            "internal/service/s3/bucket.go",
        )];
        let conv = GoConventions;

        // Function name not present at all.
        assert!(conv
            .resolve_symbol("s3.resourceBucketWrite", &nodes)
            .is_err());
        // Right function, wrong package.
        assert!(conv
            .resolve_symbol("ec2.resourceBucketRead", &nodes)
            .is_err());
    }
}
