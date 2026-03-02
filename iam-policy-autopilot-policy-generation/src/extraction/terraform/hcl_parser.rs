//! Terraform HCL parser for extracting AWS resource definitions from `.tf` files.
//!
//! This module walks a directory recursively, parses each `.tf` file using `hcl-rs`,
//! and extracts `resource` blocks whose type starts with `aws_`.
//! 
//! This parser is not to convert a resource definition in HCL to corresponding SDK call,
//! but to extract resource arn definition for resource block refinement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::{AttributeValue, TerraformParseResult, TerraformResource};

/// Parse all `.tf` files in a directory recursively.
///
/// Discovers every file ending in `.tf`, parses it with `hcl-rs`, and collects
/// AWS resource blocks. Files with syntax errors are recorded as warnings and skipped.
pub fn parse_terraform_directory(dir: &Path) -> Result<TerraformParseResult> {
    let mut result = TerraformParseResult::empty();

    let tf_files = discover_tf_files(dir);

    if tf_files.is_empty() {
        log::debug!("No .tf files found in {}", dir.display());
        return Ok(result);
    }

    log::debug!("Found {} .tf files in {}", tf_files.len(), dir.display());

    for tf_file in &tf_files {
        match parse_terraform_file(tf_file) {
            Ok(file_result) => {
                result.resources.extend(file_result.resources);
                result.warnings.extend(file_result.warnings);
            }
            Err(e) => {
                let warning = format!("{}: {e}", tf_file.display());
                log::warn!("Failed to parse Terraform file: {warning}");
                result.warnings.push(warning);
            }
        }
    }

    log::debug!(
        "Parsed {} resources from {} files",
        result.resources.len(),
        tf_files.len()
    );

    Ok(result)
}

/// Parse a single `.tf` file and extract AWS resource blocks.
fn parse_terraform_file(path: &Path) -> Result<TerraformParseResult> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    parse_terraform_content(&content, path)
}

/// Parse HCL content from a string (useful for testing without files).
fn parse_terraform_content(content: &str, source_path: &Path) -> Result<TerraformParseResult> {
    let body: hcl::Body =
        hcl::from_str(content).with_context(|| format!("parsing HCL in {}", source_path.display()))?;

    let mut result = TerraformParseResult::empty();

    for structure in body.into_iter() {
        if let hcl::Structure::Block(block) = structure {
            if block.identifier.as_str() == "resource" {
                if let Some(resource) = extract_resource_block(&block, source_path, content) {
                    let key = (resource.resource_type.clone(), resource.local_name.clone());
                    result.resources.insert(key, resource);
                }
            }
        }
    }

    Ok(result)
}

/// Extract a `TerraformResource` from an HCL `resource` block.
///
/// A resource block has the form: `resource "type" "name" { ... }`
/// We only extract blocks whose type starts with `aws_`.
/// The raw `content` is used to resolve the 1-based line number via text search.
fn extract_resource_block(block: &hcl::Block, source_path: &Path, content: &str) -> Option<TerraformResource> {
    let labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();
    if labels.len() < 2 {
        return None;
    }

    let resource_type = labels[0];
    let local_name = labels[1];

    // Only extract AWS provider resources
    if !resource_type.starts_with("aws_") {
        return None;
    }

    let attributes = extract_block_attributes(&block.body);
    let line_number = find_block_line(content, "resource", resource_type, local_name);

    Some(TerraformResource {
        resource_type: resource_type.to_string(),
        local_name: local_name.to_string(),
        attributes,
        source_file: source_path.to_path_buf(),
        line_number,
    })
}

/// Extract top-level attributes from an HCL block body.
///
/// Only flat `key = value` attributes are extracted. Nested blocks (e.g.,
/// `server_side_encryption_configuration`, `versioning`, `lifecycle_rule`)
/// are skipped because they don't contain naming attributes relevant for
/// ARN construction. Skipped blocks are logged at `trace` level for
/// diagnostics.
fn extract_block_attributes(body: &hcl::Body) -> HashMap<String, AttributeValue> {
    let mut attrs = HashMap::new();

    for structure in body.iter() {
        match structure {
            hcl::Structure::Attribute(attr) => {
                let key = attr.key.as_str().to_string();
                let value = expression_to_attribute_value(&attr.expr);
                attrs.insert(key, value);
            }
            hcl::Structure::Block(block) => {
                log::trace!(
                    "Skipping nested block '{}' (not a flat attribute)",
                    block.identifier.as_str()
                );
            }
        }
    }

    attrs
}

/// Convert an HCL expression to an `AttributeValue`.
///
/// Plain string literals become `Literal`; everything else becomes `Expression`.
fn expression_to_attribute_value(expr: &hcl::Expression) -> AttributeValue {
    match expr {
        hcl::Expression::String(s) => {
            // Check if the string contains interpolation markers
            if s.contains("${") || s.contains("%{") {
                AttributeValue::Expression(s.clone())
            } else {
                AttributeValue::Literal(s.clone())
            }
        }
        // Numbers, bools — store as literal with string representation
        hcl::Expression::Number(n) => AttributeValue::Literal(n.to_string()),
        hcl::Expression::Bool(b) => AttributeValue::Literal(b.to_string()),
        // Everything else is an expression we can't resolve
        other => {
            let s = format!("{other}");
            // Strip surrounding quotes that hcl-rs adds to template expressions
            let s = s.strip_prefix('"').unwrap_or(&s);
            let s = s.strip_suffix('"').unwrap_or(s);
            AttributeValue::Expression(s.to_string())
        }
    }
}

/// Find the 1-based line number where a block like `resource "type" "name"` starts.
///
/// Searches the raw file content for a pattern with flexible whitespace:
/// `keyword  "type_name"  "local_name"`. Skips lines inside comments
/// (`#`, `//` line comments, and `/* ... */` block comments).
///
/// **Limitation:** All three tokens (keyword, type, name) must appear on the
/// same line. Multi-line declarations (e.g., type and name on separate lines)
/// are not matched and will return `None`. In practice this is not an issue
/// because `terraform fmt` always places the full declaration on one line,
/// and this is the universal convention. When `None` is returned, the caller
/// falls back to a filename-only location string — no data is lost.
///
/// Returns `None` if not found.
fn find_block_line(content: &str, keyword: &str, type_name: &str, local_name: &str) -> Option<usize> {
    // Build a regex that tolerates any amount of whitespace between tokens.
    // `regex::escape` ensures type_name/local_name are treated literally.
    let pattern = format!(
        r#"^{}\s+"{}"\s+"{}""#,
        regex::escape(keyword),
        regex::escape(type_name),
        regex::escape(local_name),
    );
    let re = regex::Regex::new(&pattern).ok()?;

    let mut in_block_comment = false;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Track /* ... */ block comments
        if in_block_comment {
            if let Some(pos) = trimmed.find("*/") {
                // Block comment ends on this line; check remainder after `*/`
                in_block_comment = false;
                let remainder = &trimmed[pos + 2..];
                if re.is_match(remainder.trim()) {
                    return Some(i + 1);
                }
            }
            continue;
        }

        // Skip line comments
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        // Detect block comment start
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }

        if re.is_match(trimmed) {
            return Some(i + 1); // 1-based
        }
    }
    None
}

/// Discover all `.tf` files in a directory recursively.
///
/// Skips `.terraform/` directories which contain provider plugins and
/// downloaded module sources that would produce confusing duplicates.
pub(super) fn discover_tf_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.file_type().is_dir() && entry.file_name() == ".terraform")
        })
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "tf")
        })
        .map(|entry| entry.into_path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_parse_simple_s3_bucket() {
        let hcl = r#"
resource "aws_s3_bucket" "data_bucket" {
  bucket = "my-app-data"
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 1);
        let resource = &result.resources[&(String::from("aws_s3_bucket"), String::from("data_bucket"))];
        assert_eq!(resource.resource_type, "aws_s3_bucket");
        assert_eq!(resource.local_name, "data_bucket");
        assert_eq!(
            resource.attributes.get("bucket"),
            Some(&AttributeValue::Literal("my-app-data".to_string()))
        );
    }

    #[test]
    fn test_parse_multiple_resources() {
        let hcl = r#"
resource "aws_s3_bucket" "bucket1" {
  bucket = "first-bucket"
}

resource "aws_dynamodb_table" "table1" {
  name = "my-table"
  hash_key = "id"
}

resource "aws_lambda_function" "func1" {
  function_name = "my-function"
  handler       = "index.handler"
  runtime       = "python3.12"
  filename      = "lambda.zip"
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 3);
        assert!(result.resources.contains_key(&(String::from("aws_s3_bucket"), String::from("bucket1"))));
        assert!(result.resources.contains_key(&(String::from("aws_dynamodb_table"), String::from("table1"))));
        assert!(result.resources.contains_key(&(String::from("aws_lambda_function"), String::from("func1"))));
    }

    #[test]
    fn test_non_aws_resources_ignored() {
        let hcl = r#"
resource "google_storage_bucket" "gcs" {
  name = "my-gcs-bucket"
}

resource "aws_s3_bucket" "s3" {
  bucket = "my-s3-bucket"
}

resource "azurerm_storage_account" "azure" {
  name = "myazurestorage"
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 1);
        let s3 = &result.resources[&(String::from("aws_s3_bucket"), String::from("s3"))];
        assert_eq!(s3.resource_type, "aws_s3_bucket");
    }

    #[test]
    fn test_expression_preservation() {
        let hcl = r#"
resource "aws_s3_bucket" "dynamic_bucket" {
  bucket = "${var.prefix}-data-bucket"
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 1);
        let r = &result.resources[&(String::from("aws_s3_bucket"), String::from("dynamic_bucket"))];
        let bucket_attr = r.attributes.get("bucket").unwrap();
        assert!(
            !bucket_attr.is_literal(),
            "Interpolated string should be Expression, got: {bucket_attr:?}"
        );
        assert!(bucket_attr.as_str().contains("var.prefix"));
    }

    #[test]
    fn test_variable_reference_is_expression() {
        let hcl = r#"
resource "aws_s3_bucket" "var_bucket" {
  bucket = var.bucket_name
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 1);
        let r = &result.resources[&(String::from("aws_s3_bucket"), String::from("var_bucket"))];
        let bucket_attr = r.attributes.get("bucket").unwrap();
        assert!(
            !bucket_attr.is_literal(),
            "Variable reference should be Expression, got: {bucket_attr:?}"
        );
    }

    #[test]
    fn test_data_and_other_blocks_ignored() {
        let hcl = r#"
resource "aws_s3_bucket" "bucket" {
  bucket = "my-bucket"
}

data "aws_iam_policy_document" "policy" {
  statement {
    actions = ["s3:GetObject"]
  }
}

variable "region" {
  default = "us-east-1"
}

output "bucket_arn" {
  value = aws_s3_bucket.bucket.arn
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        assert_eq!(result.resources.len(), 1);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_parse_directory_with_tf_files() {
        let tmp = TempDir::new().expect("create temp dir");

        let main_tf = tmp.path().join("main.tf");
        let mut f = std::fs::File::create(&main_tf).expect("create file");
        writeln!(
            f,
            r#"resource "aws_s3_bucket" "b1" {{ bucket = "bucket-one" }}"#
        )
        .expect("write");

        let sub = tmp.path().join("modules");
        std::fs::create_dir_all(&sub).expect("create subdir");
        let sub_tf = sub.join("storage.tf");
        let mut f2 = std::fs::File::create(&sub_tf).expect("create file");
        writeln!(
            f2,
            r#"resource "aws_dynamodb_table" "t1" {{ name = "table-one" }}"#
        )
        .expect("write");

        let txt = tmp.path().join("readme.md");
        std::fs::write(&txt, "# readme").expect("write");

        let result = parse_terraform_directory(tmp.path()).expect("should parse dir");

        assert_eq!(result.resources.len(), 2);
        let types: Vec<&str> = result
            .resources
            .values()
            .map(|r| r.resource_type.as_str())
            .collect();
        assert!(types.contains(&"aws_s3_bucket"));
        assert!(types.contains(&"aws_dynamodb_table"));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_parse_directory_no_tf_files() {
        let tmp = TempDir::new().expect("create temp dir");
        std::fs::write(tmp.path().join("readme.md"), "hello").expect("write");

        let result = parse_terraform_directory(tmp.path()).expect("should parse");
        assert!(result.resources.is_empty());
    }

    #[test]
    fn test_parse_directory_with_syntax_error() {
        let tmp = TempDir::new().expect("create temp dir");

        let good = tmp.path().join("good.tf");
        let mut f = std::fs::File::create(&good).expect("create");
        writeln!(
            f,
            r#"resource "aws_s3_bucket" "ok" {{ bucket = "good-bucket" }}"#
        )
        .expect("write");

        let bad = tmp.path().join("bad.tf");
        std::fs::write(&bad, "this is not valid HCL {{{{").expect("write");

        let result = parse_terraform_directory(tmp.path()).expect("should parse dir");

        assert_eq!(result.resources.len(), 1);
        assert!(!result.warnings.is_empty());
        assert!(result.warnings[0].contains("bad.tf"));
    }

    #[test]
    fn test_source_file_path_preserved() {
        let hcl = r#"
resource "aws_s3_bucket" "b" {
  bucket = "test"
}
"#;
        let path = Path::new("infra/main.tf");
        let result = parse_terraform_content(hcl, path).expect("should parse");

        let r = &result.resources[&(String::from("aws_s3_bucket"), String::from("b"))];
        assert_eq!(r.source_file, PathBuf::from("infra/main.tf"));
    }

    #[test]
    fn test_numeric_and_bool_attributes() {
        let hcl = r#"
resource "aws_dynamodb_table" "t" {
  name           = "my-table"
  read_capacity  = 5
  write_capacity = 10
  stream_enabled = true
}
"#;
        let result = parse_terraform_content(hcl, Path::new("main.tf")).expect("should parse");

        let r = &result.resources[&(String::from("aws_dynamodb_table"), String::from("t"))];
        let attrs = &r.attributes;
        assert_eq!(
            attrs.get("read_capacity"),
            Some(&AttributeValue::Literal("5".to_string()))
        );
        assert_eq!(
            attrs.get("stream_enabled"),
            Some(&AttributeValue::Literal("true".to_string()))
        );
    }

    #[test]
    fn test_line_number_tracking() {
        let hcl = r#"
resource "aws_s3_bucket" "first" {
  bucket = "first-bucket"
}

resource "aws_s3_bucket" "second" {
  bucket = "second-bucket"
}
"#;
        let result = parse_terraform_content(hcl, Path::new("main.tf")).expect("parse");

        let first = &result.resources[&(String::from("aws_s3_bucket"), String::from("first"))];
        let second = &result.resources[&(String::from("aws_s3_bucket"), String::from("second"))];
        assert_eq!(first.line_number, Some(2));
        assert_eq!(second.line_number, Some(6));
    }

    #[test]
    fn test_discover_skips_dot_terraform_dir() {
        let tmp = TempDir::new().expect("create temp dir");

        let main_tf = tmp.path().join("main.tf");
        std::fs::write(&main_tf, r#"resource "aws_s3_bucket" "b" {}"#).expect("write");

        let dot_tf = tmp.path().join(".terraform/providers/main.tf");
        std::fs::create_dir_all(dot_tf.parent().unwrap()).expect("mkdir");
        std::fs::write(&dot_tf, r#"resource "aws_s3_bucket" "cached" {}"#).expect("write");

        let files = discover_tf_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], main_tf);
    }

    #[test]
    fn test_find_block_line_skips_comments() {
        let hcl = r#"
# resource "aws_s3_bucket" "b" {
//   bucket = "commented-out"
// }

resource "aws_s3_bucket" "b" {
  bucket = "real-bucket"
}
"#;
        let result = parse_terraform_content(hcl, Path::new("main.tf")).expect("parse");
        let r = &result.resources[&(String::from("aws_s3_bucket"), String::from("b"))];
        assert_eq!(r.line_number, Some(6), "Should skip commented-out lines");
    }

    #[test]
    fn test_resource_with_empty_body() {
        let hcl = r#"
resource "aws_s3_bucket" "empty" {}
"#;
        let result = parse_terraform_content(hcl, Path::new("main.tf")).expect("parse");
        assert_eq!(result.resources.len(), 1);
        let r = &result.resources[&(String::from("aws_s3_bucket"), String::from("empty"))];
        assert_eq!(r.resource_type, "aws_s3_bucket");
        assert_eq!(r.local_name, "empty");
        assert!(r.attributes.is_empty());
    }
}
