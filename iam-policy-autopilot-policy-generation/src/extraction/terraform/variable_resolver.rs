//! Terraform variable resolver.
//!
//! Resolves `var.xxx` references and `"${var.xxx}"` interpolations in
//! Terraform resource attributes by reading:
//! 1. `variable` block defaults from `.tf` files
//! 2. Overrides from `terraform.tfvars` and `*.auto.tfvars`

use std::collections::HashMap;
use std::path::Path;
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
pub struct VariableContext {
    vars: HashMap<String, String>,
}

impl VariableContext {
    /// Build a variable context from a Terraform directory.
    ///
    /// 1. Scans all `.tf` files for `variable` blocks with `default` values
    /// 2. Reads `terraform.tfvars` if present (overrides defaults)
    /// 3. Reads `*.auto.tfvars` files if present (overrides defaults)
    pub fn from_directory(dir: &Path) -> Result<Self> {
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
        re.replace_all(s, |caps: &regex::Captures| {
            let var_name = caps.get(1).map_or("", |m| m.as_str());
            match self.vars.get(var_name) {
                Some(value) => value.clone(),
                None => caps[0].to_string(), // Leave unresolvable as-is
            }
        })
        .into_owned()
    }

    /// Get a variable value by name (for testing).
    #[cfg(test)]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }
}

/// Try to extract a scalar string value from an HCL expression.
///
/// Returns `Some` for strings, numbers, and bools (converted to string).
/// Returns `None` for complex types (lists, maps, objects, function calls,
/// variable references) — these are not useful for ARN construction.
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

    #[test]
    fn test_literal_not_modified() {
        let ctx = VariableContext::default();
        let lit = AttributeValue::Literal("already-resolved".to_string());
        assert!(ctx.try_resolve(&lit).is_none());
    }

    // -----------------------------------------------------------------------
    // Variable extraction + override tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_variable_defaults() {
        let mut ctx = VariableContext::default();
        let hcl = r#"
variable "bucket_name" {
  default = "my-default-bucket"
}

variable "region" {
  default = "us-east-1"
}

variable "no_default" {
  type = string
}
"#;
        ctx.extract_variable_defaults(hcl, Path::new("variables.tf"));

        assert_eq!(ctx.get("bucket_name"), Some("my-default-bucket"));
        assert_eq!(ctx.get("region"), Some("us-east-1"));
        assert!(ctx.get("no_default").is_none());
    }

    #[test]
    fn test_tfvars_override_defaults() {
        let mut ctx = VariableContext::default();
        ctx.extract_variable_defaults(
            r#"
variable "bucket_name" {
  default = "default-bucket"
}
"#,
            Path::new("variables.tf"),
        );
        assert_eq!(ctx.get("bucket_name"), Some("default-bucket"));

        ctx.apply_tfvars(r#"bucket_name = "override-bucket""#);
        assert_eq!(ctx.get("bucket_name"), Some("override-bucket"));
    }

    #[test]
    fn test_complex_default_values_skipped() {
        let mut ctx = VariableContext::default();
        let hcl = r#"
variable "tags" {
  default = {
    env = "prod"
    team = "platform"
  }
}

variable "allowed_cidrs" {
  default = ["10.0.0.0/8", "172.16.0.0/12"]
}

variable "simple" {
  default = "kept"
}
"#;
        ctx.extract_variable_defaults(hcl, Path::new("variables.tf"));
        assert!(ctx.get("tags").is_none(), "Map defaults should be skipped");
        assert!(
            ctx.get("allowed_cidrs").is_none(),
            "List defaults should be skipped"
        );
        assert_eq!(ctx.get("simple"), Some("kept"));
    }

    // -----------------------------------------------------------------------
    // resolve_attributes integration test
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_attributes_in_parse_result() {
        let mut ctx = VariableContext::default();
        ctx.vars
            .insert("bucket_name".to_string(), "resolved-bucket".to_string());

        let resource = super::super::TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "b".to_string(),
            attributes: HashMap::from([(
                "bucket".to_string(),
                AttributeValue::Expression("var.bucket_name".to_string()),
            )]),
            location: crate::Location::new(std::path::PathBuf::from("main.tf"), (1, 1), (1, 1)),
        };
        let mut result = TerraformResources::default();
        result.insert(resource);

        ctx.resolve_attributes(&mut result);

        let r = result.values().next().unwrap();
        assert_eq!(
            r.attributes.get("bucket"),
            Some(&AttributeValue::Literal("resolved-bucket".to_string()))
        );
    }

    // -----------------------------------------------------------------------
    // Directory-based tests (require temp dirs)
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_directory_with_tfvars() {
        let tmp = TempDir::new().expect("tmp");

        let mut vars_tf = std::fs::File::create(tmp.path().join("variables.tf")).expect("create");
        writeln!(
            vars_tf,
            r#"
variable "bucket_name" {{
  default = "default-bucket"
}}
variable "table_name" {{
  default = "default-table"
}}
"#
        )
        .expect("write");

        let mut tfvars =
            std::fs::File::create(tmp.path().join("terraform.tfvars")).expect("create");
        writeln!(tfvars, r#"bucket_name = "prod-bucket""#).expect("write");

        let ctx = VariableContext::from_directory(tmp.path()).expect("resolve");

        assert_eq!(ctx.get("bucket_name"), Some("prod-bucket"));
        assert_eq!(ctx.get("table_name"), Some("default-table"));
    }

    #[test]
    fn test_from_directory_with_auto_tfvars() {
        let tmp = TempDir::new().expect("tmp");

        let mut vars_tf = std::fs::File::create(tmp.path().join("variables.tf")).expect("create");
        writeln!(
            vars_tf,
            r#"
variable "env" {{
  default = "dev"
}}
"#
        )
        .expect("write");

        let mut auto = std::fs::File::create(tmp.path().join("prod.auto.tfvars")).expect("create");
        writeln!(auto, r#"env = "prod""#).expect("write");

        let ctx = VariableContext::from_directory(tmp.path()).expect("resolve");
        assert_eq!(ctx.get("env"), Some("prod"));
    }

    #[test]
    fn test_auto_tfvars_lexicographic_ordering() {
        let tmp = TempDir::new().expect("tmp");

        let mut vars_tf = std::fs::File::create(tmp.path().join("variables.tf")).expect("create");
        writeln!(
            vars_tf,
            r#"
variable "env" {{
  default = "dev"
}}
"#
        )
        .expect("write");

        let mut a = std::fs::File::create(tmp.path().join("a.auto.tfvars")).expect("create");
        writeln!(a, r#"env = "from-a""#).expect("write");

        let mut b = std::fs::File::create(tmp.path().join("b.auto.tfvars")).expect("create");
        writeln!(b, r#"env = "from-b""#).expect("write");

        let ctx = VariableContext::from_directory(tmp.path()).expect("resolve");
        assert_eq!(
            ctx.get("env"),
            Some("from-b"),
            "Later auto.tfvars should win"
        );
    }
}
