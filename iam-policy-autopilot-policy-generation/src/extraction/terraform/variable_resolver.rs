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

use super::{AttributeValue, TerraformParseResult};

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
    pub fn resolve_attributes(&self, result: &mut TerraformParseResult) {
        for resource in result.resources.values_mut() {
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
    use std::io::Write;
    use tempfile::TempDir;

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

        // Set defaults
        ctx.extract_variable_defaults(
            r#"
variable "bucket_name" {
  default = "default-bucket"
}
"#,
            Path::new("variables.tf"),
        );
        assert_eq!(ctx.get("bucket_name"), Some("default-bucket"));

        // Override with tfvars
        ctx.apply_tfvars(r#"bucket_name = "override-bucket""#);
        assert_eq!(ctx.get("bucket_name"), Some("override-bucket"));
    }

    #[test]
    fn test_resolve_bare_var_reference() {
        let mut ctx = VariableContext::default();
        ctx.vars
            .insert("bucket_name".to_string(), "my-bucket".to_string());

        let expr = AttributeValue::Expression("var.bucket_name".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        assert_eq!(resolved, AttributeValue::Literal("my-bucket".to_string()));
    }

    #[test]
    fn test_resolve_interpolation() {
        let mut ctx = VariableContext::default();
        ctx.vars.insert("prefix".to_string(), "myapp".to_string());

        let expr = AttributeValue::Expression("${var.prefix}-data-bucket".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        assert_eq!(
            resolved,
            AttributeValue::Literal("myapp-data-bucket".to_string())
        );
    }

    #[test]
    fn test_resolve_multiple_interpolations() {
        let mut ctx = VariableContext::default();
        ctx.vars.insert("env".to_string(), "prod".to_string());
        ctx.vars.insert("app".to_string(), "myapp".to_string());

        let expr = AttributeValue::Expression("${var.app}-${var.env}-bucket".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        assert_eq!(
            resolved,
            AttributeValue::Literal("myapp-prod-bucket".to_string())
        );
    }

    #[test]
    fn test_resolve_mixed_literal_and_interpolation() {
        let mut ctx = VariableContext::default();
        ctx.vars.insert("env1".to_string(), "staging".to_string());
        ctx.vars.insert("env2".to_string(), "us-east".to_string());

        let expr = AttributeValue::Expression("NAME_In__${var.env1}__${var.env2}".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        assert_eq!(
            resolved,
            AttributeValue::Literal("NAME_In__staging__us-east".to_string())
        );
    }

    #[test]
    fn test_resolve_mixed_partial_only_some_vars_defined() {
        let mut ctx = VariableContext::default();
        ctx.vars.insert("env1".to_string(), "staging".to_string());
        // env2 NOT defined

        let expr = AttributeValue::Expression("NAME_In__${var.env1}__${var.env2}".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        // env1 resolved, env2 not — stays as Expression
        assert_eq!(
            resolved,
            AttributeValue::Expression("NAME_In__staging__${var.env2}".to_string())
        );
    }

    #[test]
    fn test_unresolvable_var_stays_expression() {
        let ctx = VariableContext::default(); // No vars defined

        let expr = AttributeValue::Expression("var.unknown".to_string());
        let resolved = ctx.try_resolve(&expr);
        assert!(resolved.is_none());
    }

    #[test]
    fn test_partial_interpolation_stays_expression() {
        let mut ctx = VariableContext::default();
        ctx.vars.insert("prefix".to_string(), "myapp".to_string());
        // var.suffix is NOT defined

        let expr = AttributeValue::Expression("${var.prefix}-${var.suffix}-bucket".to_string());
        let resolved = ctx.try_resolve(&expr).unwrap();
        // prefix resolved, suffix not — still an expression
        assert_eq!(
            resolved,
            AttributeValue::Expression("myapp-${var.suffix}-bucket".to_string())
        );
    }

    #[test]
    fn test_literal_not_modified() {
        let ctx = VariableContext::default();
        let lit = AttributeValue::Literal("already-resolved".to_string());
        assert!(ctx.try_resolve(&lit).is_none());
    }

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
            source_file: std::path::PathBuf::from("main.tf"),
            line_number: None,
        };
        let mut result = TerraformParseResult::empty();
        result.resources.insert(
            (resource.resource_type.clone(), resource.local_name.clone()),
            resource,
        );

        ctx.resolve_attributes(&mut result);

        let r = result.resources.values().next().unwrap();
        assert_eq!(
            r.attributes.get("bucket"),
            Some(&AttributeValue::Literal("resolved-bucket".to_string()))
        );
    }

    #[test]
    fn test_from_directory_with_tfvars() {
        let tmp = TempDir::new().expect("tmp");

        // variables.tf with defaults
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

        // terraform.tfvars overrides bucket_name
        let mut tfvars =
            std::fs::File::create(tmp.path().join("terraform.tfvars")).expect("create");
        writeln!(tfvars, r#"bucket_name = "prod-bucket""#).expect("write");

        let ctx = VariableContext::from_directory(tmp.path()).expect("resolve");

        assert_eq!(ctx.get("bucket_name"), Some("prod-bucket")); // overridden
        assert_eq!(ctx.get("table_name"), Some("default-table")); // default kept
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

        // Two auto.tfvars — "b" should override "a" due to lexicographic order
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

        // Complex defaults (map, list) should be silently skipped
        assert!(ctx.get("tags").is_none(), "Map defaults should be skipped");
        assert!(
            ctx.get("allowed_cidrs").is_none(),
            "List defaults should be skipped"
        );
        // Scalar string defaults should still be captured
        assert_eq!(ctx.get("simple"), Some("kept"));
    }
}
