//! Terraform variable resolver.
//!
//! Resolves `var.xxx` references and `"${var.xxx}"` interpolations in
//! Terraform resource attributes by reading:
//! 1. `variable` block defaults from `.tf` files
//! 2. Overrides from `terraform.tfvars` and `*.auto.tfvars`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;

use super::{AttributeValue, TerraformResources};

/// Regex matching `${var.xxx}` interpolation placeholders.
static VAR_INTERPOLATION_RE: OnceLock<Regex> = OnceLock::new();

fn var_interpolation_regex() -> &'static Regex {
    VAR_INTERPOLATION_RE.get_or_init(|| Regex::new(r"\$\{var\.([^}]+)\}").expect("valid regex"))
}

/// A map of variable name → resolved string value.
#[derive(Debug, Clone, Default)]
pub(crate) struct VariableContext {
    vars: HashMap<String, String>,
}

impl VariableContext {
    /// Build a variable context from explicit `.tf` and `.tfvars` file paths (no directory).
    ///
    /// Used when `--terraform-file` + `--tfvars` are provided without `--terraform-dir`.
    ///
    /// 1. Extracts `variable` block defaults from the given `.tf` files
    /// 2. Applies explicit `.tfvars` overrides in order
    pub(crate) fn from_files_and_tfvars(tf_files: &[PathBuf], tfvars_paths: &[PathBuf]) -> Self {
        let mut ctx = Self::default();

        // Step 1: Extract variable defaults from .tf files
        for tf_file in tf_files {
            match std::fs::read_to_string(tf_file) {
                Ok(content) => ctx.extract_variable_defaults(&content, tf_file),
                Err(e) => log::warn!("Failed to read {}: {e}", tf_file.display()),
            }
        }

        // Step 2: Apply explicit tfvars overrides
        for tfvars_file in tfvars_paths {
            match std::fs::read_to_string(tfvars_file) {
                Ok(content) => ctx.apply_tfvars(&content),
                Err(e) => log::warn!("Failed to read tfvars file {}: {e}", tfvars_file.display()),
            }
        }

        log::debug!(
            "Resolved {} Terraform variables from {} tf + {} tfvars files",
            ctx.vars.len(),
            tf_files.len(),
            tfvars_paths.len()
        );
        ctx
    }

    /// Build a variable context from a Terraform directory, with optional explicit
    /// `.tfvars` file paths.
    ///
    /// When `explicit_tfvars` is non-empty, these files are applied **after** all
    /// auto-discovered sources, giving them highest precedence. This matches the
    /// Terraform CLI behavior of `-var-file=` flags.
    ///
    /// Precedence order (later overrides earlier):
    /// 1. `variable` block defaults from `.tf` files (lexicographic order)
    /// 2. `terraform.tfvars` (if present in directory)
    /// 3. `*.auto.tfvars` files (lexicographic order)
    /// 4. Explicit tfvars files (in the order provided)
    pub(crate) fn from_directory_with_explicit_tfvars(
        dir: &Path,
        explicit_tfvars: &[PathBuf],
    ) -> Result<Self> {
        let mut ctx = Self::default();

        // Step 1: Extract defaults from variable blocks in .tf files
        let tf_files = super::hcl_parser::discover_tf_files(dir);
        for tf_file in &tf_files {
            match std::fs::read_to_string(tf_file) {
                Ok(content) => ctx.extract_variable_defaults(&content, tf_file),
                Err(e) => log::warn!("Failed to read {}: {e}", tf_file.display()),
            }
        }

        // Step 2: Read terraform.tfvars (overrides defaults)
        let tfvars_path = dir.join("terraform.tfvars");
        if tfvars_path.exists() {
            let content = std::fs::read_to_string(&tfvars_path)
                .with_context(|| format!("reading {}", tfvars_path.display()))?;
            ctx.apply_tfvars(&content);
        }

        // Step 3: Read *.auto.tfvars (overrides defaults, applied after terraform.tfvars)
        let mut auto_tfvars: Vec<_> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(std::result::Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.ends_with(".auto.tfvars"))
                })
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                log::warn!("Failed to read directory {}: {e}", dir.display());
                Vec::new()
            }
        };
        auto_tfvars.sort(); // Deterministic lexicographic order (matches Terraform behavior)

        for auto_path in &auto_tfvars {
            if let Ok(content) = std::fs::read_to_string(auto_path) {
                ctx.apply_tfvars(&content);
            }
        }

        // Step 4: Apply explicit --tfvars files (highest precedence, in order)
        for tfvars_file in explicit_tfvars {
            match std::fs::read_to_string(tfvars_file) {
                Ok(content) => ctx.apply_tfvars(&content),
                Err(e) => log::warn!(
                    "Failed to read explicit tfvars file {}: {e}",
                    tfvars_file.display()
                ),
            }
        }

        log::debug!("Resolved {} Terraform variables", ctx.vars.len());
        Ok(ctx)
    }

    /// Extract `variable` block defaults from HCL content.
    ///
    /// Looks for patterns like:
    /// ```hcl
    /// variable "bucket_name" {
    ///   default = "my-bucket"
    /// }
    /// ```
    ///
    /// Complex default values (lists, maps, objects) are silently skipped
    /// because only scalar string values are useful for ARN construction.
    fn extract_variable_defaults(&mut self, content: &str, source: &Path) {
        let Ok(body) = hcl::from_str::<hcl::Body>(content) else {
            log::warn!(
                "Failed to parse HCL in {} while extracting variable defaults",
                source.display()
            );
            return;
        };

        for structure in body {
            if let hcl::Structure::Block(block) = structure {
                if block.identifier.as_str() == "variable" {
                    if let Some(var_name) = block.labels.first().map(|l| l.as_str().to_string()) {
                        // Look for a "default" attribute in the block body
                        for inner in &block.body {
                            if let hcl::Structure::Attribute(attr) = inner {
                                if attr.key.as_str() == "default" {
                                    if let Some(val) = expr_to_string(&attr.expr) {
                                        self.vars.insert(var_name.clone(), val);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply variable assignments from a `.tfvars` file.
    ///
    /// Format: `key = "value"` (one per line, HCL syntax)
    fn apply_tfvars(&mut self, content: &str) {
        let Ok(body) = hcl::from_str::<hcl::Body>(content) else {
            log::warn!("Failed to parse tfvars content");
            return;
        };

        for structure in body {
            if let hcl::Structure::Attribute(attr) = structure {
                if let Some(val) = expr_to_string(&attr.expr) {
                    self.vars.insert(attr.key.as_str().to_string(), val);
                }
            }
        }
    }

    /// Resolve variable references in a parse result's resource attributes.
    ///
    /// For each resource attribute that is an `Expression`:
    /// - `var.xxx` → look up `xxx` in the context, convert to `Literal` if found
    /// - `"${var.xxx}"` → substitute the variable value in the interpolation
    /// - `"${var.prefix}-suffix"` → substitute and produce a `Literal` if all vars resolve
    pub fn resolve_attributes(&self, result: &mut TerraformResources) {
        for resource in result.values_mut() {
            self.resolve_map(&mut resource.attributes);
        }
    }

    /// Resolve variable references in an attribute map.
    fn resolve_map(&self, attrs: &mut HashMap<String, AttributeValue>) {
        for value in attrs.values_mut() {
            if let Some(resolved) = self.try_resolve(value) {
                *value = resolved;
            }
        }
    }

    /// Try to resolve a single attribute value.
    /// Returns `Some(Literal(...))` if fully resolved, `None` if unresolvable.
    pub(crate) fn try_resolve(&self, value: &AttributeValue) -> Option<AttributeValue> {
        match value {
            AttributeValue::Literal(_) => None, // Already resolved
            AttributeValue::Expression(expr) => {
                let trimmed = expr.trim();

                // Case 1: bare variable reference like `var.bucket_name`
                if let Some(var_name) = trimmed.strip_prefix("var.") {
                    // Handle potential trailing content (shouldn't happen for bare refs)
                    let var_name = var_name.trim();
                    return self
                        .vars
                        .get(var_name)
                        .map(|v| AttributeValue::Literal(v.clone()));
                }

                // Case 2: string interpolation like `"${var.prefix}-bucket"`
                if trimmed.contains("${var.") {
                    let resolved = self.resolve_interpolation(trimmed);
                    if !resolved.contains("${var.") {
                        // All variable references resolved
                        return Some(AttributeValue::Literal(resolved));
                    }
                    // Partially resolved — still an expression
                    return Some(AttributeValue::Expression(resolved));
                }

                // Case 3: local reference like `local.xxx` — not supported yet
                None
            }
        }
    }

    /// Resolve `${var.xxx}` interpolations in a string.
    ///
    /// Uses regex to find each `${var.xxx}` placeholder, looks up only the
    /// referenced variables in the map, and substitutes their values.
    /// Unresolvable placeholders are left as-is.
    fn resolve_interpolation(&self, s: &str) -> String {
        let re = var_interpolation_regex();
        // caps[0] = entire match (e.g. "${var.bucket_name}")
        // caps[1] = capture group: the variable name (e.g. "bucket_name")
        // Group 1 is always present when the regex matches (mandatory group),
        // but we handle None defensively via and_then.
        re.replace_all(s, |caps: &regex::Captures| {
            caps.get(1)
                .map(|m| m.as_str())
                .and_then(|var_name| self.vars.get(var_name).cloned())
                .unwrap_or_else(|| caps[0].to_string()) // Leave unresolvable as-is
        })
        .into_owned()
    }

    /// Get a variable value by name (for testing).
    #[cfg(test)]
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }
}

/// Try to extract a scalar string value from an HCL expression.
///
/// Returns `Some` for strings, numbers, and bools (converted to string).
/// Returns `None` for complex types (lists, maps, objects, function calls,
/// variable references) — these are not useful for ARN construction.
///
/// NOTE: We intentionally don't use `hcl::Expression`'s `Display` impl here
/// because it formats *all* variants (e.g., function call `upper(var.name)` → `"upper(var.name)"`),
/// producing nonsensical ARN values. This function acts as both a filter and extractor.
fn expr_to_string(expr: &hcl::Expression) -> Option<String> {
    match expr {
        hcl::Expression::String(s) => Some(s.clone()),
        hcl::Expression::Number(n) => Some(n.to_string()),
        hcl::Expression::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::io::Write;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helper: build a VariableContext from (name, value) pairs
    // -----------------------------------------------------------------------

    fn ctx_from(vars: &[(&str, &str)]) -> VariableContext {
        let mut ctx = VariableContext::default();
        for (k, v) in vars {
            ctx.vars.insert((*k).to_string(), (*v).to_string());
        }
        ctx
    }

    // -----------------------------------------------------------------------
    // Parameterized try_resolve tests
    // -----------------------------------------------------------------------

    #[rstest]
    // Bare variable reference → Literal
    #[case("bare_var",        &[("bucket_name", "my-bucket")],     "var.bucket_name",                        Some(AttributeValue::Literal("my-bucket".into())))]
    // Single interpolation → Literal
    #[case("interpolation",   &[("prefix", "myapp")],              "${var.prefix}-data-bucket",               Some(AttributeValue::Literal("myapp-data-bucket".into())))]
    // Multiple interpolations → Literal
    #[case("multi_interp",    &[("env", "prod"), ("app", "myapp")], "${var.app}-${var.env}-bucket",           Some(AttributeValue::Literal("myapp-prod-bucket".into())))]
    // Mixed literal text + interpolations → Literal
    #[case("mixed_literal",   &[("env1", "staging"), ("env2", "us-east")], "NAME_In__${var.env1}__${var.env2}", Some(AttributeValue::Literal("NAME_In__staging__us-east".into())))]
    // Partial resolution (one var missing) → Expression
    #[case("partial_interp",  &[("env1", "staging")],              "NAME_In__${var.env1}__${var.env2}",      Some(AttributeValue::Expression("NAME_In__staging__${var.env2}".into())))]
    // Partial resolution (suffix missing) → Expression
    #[case("partial_suffix",  &[("prefix", "myapp")],              "${var.prefix}-${var.suffix}-bucket",     Some(AttributeValue::Expression("myapp-${var.suffix}-bucket".into())))]
    // Unknown bare var → None
    #[case("unknown_bare",    &[],                                  "var.unknown",                            None)]
    fn test_try_resolve(
        #[case] _name: &str,
        #[case] vars: &[(&str, &str)],
        #[case] input: &str,
        #[case] expected: Option<AttributeValue>,
    ) {
        let ctx = ctx_from(vars);
        let expr = AttributeValue::Expression(input.to_string());
        let result = ctx.try_resolve(&expr);
        assert_eq!(result, expected, "try_resolve mismatch for case '{_name}'");
    }

    // -----------------------------------------------------------------------
    // extract_variable_defaults + apply_tfvars tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for variable extraction and override tests.
    ///
    /// Applies HCL defaults, then optional tfvars overrides, and asserts
    /// each expected variable value.
    fn assert_variable_extraction(
        hcl_content: &str,
        tfvars_content: Option<&str>,
        expected: &[(&str, Option<&str>)],
    ) {
        let mut ctx = VariableContext::default();
        ctx.extract_variable_defaults(hcl_content, Path::new("variables.tf"));
        if let Some(tfvars) = tfvars_content {
            ctx.apply_tfvars(tfvars);
        }
        for &(var_name, expected_value) in expected {
            assert_eq!(
                ctx.get(var_name),
                expected_value,
                "variable '{var_name}' mismatch"
            );
        }
    }

    #[rstest]
    // Scalar defaults extracted, type-only variables skipped
    #[case(
        "scalar_defaults",
        "variable \"bucket_name\" {\n  default = \"my-default-bucket\"\n}\nvariable \"region\" {\n  default = \"us-east-1\"\n}\nvariable \"no_default\" {\n  type = string\n}",
        None,
        &[("bucket_name", Some("my-default-bucket")), ("region", Some("us-east-1")), ("no_default", None)]
    )]
    // tfvars override defaults
    #[case(
        "tfvars_override",
        "variable \"bucket_name\" {\n  default = \"default-bucket\"\n}",
        Some("bucket_name = \"override-bucket\""),
        &[("bucket_name", Some("override-bucket"))]
    )]
    // Complex types (maps, lists) skipped, scalar kept
    #[case(
        "complex_defaults_skipped",
        "variable \"tags\" {\n  default = {\n    env = \"prod\"\n    team = \"platform\"\n  }\n}\nvariable \"allowed_cidrs\" {\n  default = [\"10.0.0.0/8\", \"172.16.0.0/12\"]\n}\nvariable \"simple\" {\n  default = \"kept\"\n}",
        None,
        &[("tags", None), ("allowed_cidrs", None), ("simple", Some("kept"))]
    )]
    fn test_variable_extraction(
        #[case] _name: &str,
        #[case] hcl_content: &str,
        #[case] tfvars_content: Option<&str>,
        #[case] expected: &[(&str, Option<&str>)],
    ) {
        assert_variable_extraction(hcl_content, tfvars_content, expected);
    }

    // -----------------------------------------------------------------------
    // resolve_attributes + try_resolve edge case tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for resolve_attributes tests.
    fn assert_resolve_attributes(
        vars: &[(&str, &str)],
        input_attr_key: &str,
        input_attr_value: AttributeValue,
        expected_attr_value: AttributeValue,
    ) {
        let ctx = ctx_from(vars);
        let resource = super::super::TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "b".to_string(),
            attributes: HashMap::from([(input_attr_key.to_string(), input_attr_value)]),
            location: crate::Location::new(std::path::PathBuf::from("main.tf"), (1, 1), (1, 1)),
        };
        let mut result = TerraformResources::default();
        result.insert(resource);
        ctx.resolve_attributes(&mut result);
        let r = result.values().next().unwrap();
        assert_eq!(
            r.attributes.get(input_attr_key),
            Some(&expected_attr_value),
            "attribute '{input_attr_key}' mismatch"
        );
    }

    #[rstest]
    // Variable expression resolved to literal
    #[case(
        "expression_resolved",
        &[("bucket_name", "resolved-bucket")],
        "bucket",
        AttributeValue::Expression("var.bucket_name".to_string()),
        AttributeValue::Literal("resolved-bucket".to_string())
    )]
    // Literal not modified
    #[case(
        "literal_not_modified",
        &[],
        "bucket",
        AttributeValue::Literal("already-resolved".to_string()),
        AttributeValue::Literal("already-resolved".to_string())
    )]
    fn test_resolve_attributes(
        #[case] _name: &str,
        #[case] vars: &[(&str, &str)],
        #[case] attr_key: &str,
        #[case] input: AttributeValue,
        #[case] expected: AttributeValue,
    ) {
        assert_resolve_attributes(vars, attr_key, input, expected);
    }

    // -----------------------------------------------------------------------
    // Directory-based tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for directory-based variable resolution tests.
    ///
    /// Creates a temp directory, writes the given files, builds a
    /// `VariableContext`, and asserts each expected variable value.
    fn assert_directory_resolution(files: &[(&str, &str)], expected: &[(&str, Option<&str>)]) {
        let tmp = TempDir::new().expect("create temp dir");

        for (filename, content) in files {
            let path = tmp.path().join(filename);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent dir");
            }
            let mut f = std::fs::File::create(&path).expect("create file");
            writeln!(f, "{content}").expect("write file");
        }

        let ctx =
            VariableContext::from_directory_with_explicit_tfvars(tmp.path(), &[]).expect("resolve");

        for &(var_name, expected_value) in expected {
            assert_eq!(
                ctx.get(var_name),
                expected_value,
                "variable '{var_name}' resolution mismatch"
            );
        }
    }

    #[rstest]
    // tfvars overrides default, non-overridden default preserved
    #[case(
        "tfvars_override",
        &[
            ("variables.tf", "variable \"bucket_name\" {\n  default = \"default-bucket\"\n}\nvariable \"table_name\" {\n  default = \"default-table\"\n}"),
            ("terraform.tfvars", "bucket_name = \"prod-bucket\""),
        ],
        &[("bucket_name", Some("prod-bucket")), ("table_name", Some("default-table"))]
    )]
    // auto.tfvars overrides default
    #[case(
        "auto_tfvars_override",
        &[
            ("variables.tf", "variable \"env\" {\n  default = \"dev\"\n}"),
            ("prod.auto.tfvars", "env = \"prod\""),
        ],
        &[("env", Some("prod"))]
    )]
    // Later auto.tfvars wins (lexicographic ordering)
    #[case(
        "auto_tfvars_lexicographic_ordering",
        &[
            ("variables.tf", "variable \"env\" {\n  default = \"dev\"\n}"),
            ("a.auto.tfvars", "env = \"from-a\""),
            ("b.auto.tfvars", "env = \"from-b\""),
        ],
        &[("env", Some("from-b"))]
    )]
    // Same variable default in multiple .tf files — lexicographically later file wins
    #[case(
        "tf_file_clash_lexicographic",
        &[
            ("a_vars.tf", "variable \"region\" {\n  default = \"us-west-1\"\n}"),
            ("b_vars.tf", "variable \"region\" {\n  default = \"us-east-1\"\n}"),
        ],
        &[("region", Some("us-east-1"))]
    )]
    fn test_directory_resolution(
        #[case] _name: &str,
        #[case] files: &[(&str, &str)],
        #[case] expected: &[(&str, Option<&str>)],
    ) {
        assert_directory_resolution(files, expected);
    }
}
