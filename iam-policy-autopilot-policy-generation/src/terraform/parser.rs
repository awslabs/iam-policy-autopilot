//! Terraform HCL parser for extracting AWS resource definitions from `.tf` files.
//!
//! This module walks a directory recursively, parses each `.tf` file using `hcl-rs`,
//! and extracts `resource` and `data` blocks whose type starts with `aws_`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::{AttributeValue, TerraformDataSource, TerraformParseResult, TerraformResource};

/// Parse all `.tf` files in a directory recursively.
///
/// Discovers every file ending in `.tf`, parses it with `hcl-rs`, and collects
/// AWS resource and data source blocks. Files with syntax errors are recorded as
/// warnings and skipped.
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
                result.data_sources.extend(file_result.data_sources);
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
        "Parsed {} resources and {} data sources from {} files",
        result.resources.len(),
        result.data_sources.len(),
        tf_files.len()
    );

    Ok(result)
}

/// Parse a single `.tf` file and extract AWS resource and data source blocks.
pub fn parse_terraform_file(path: &Path) -> Result<TerraformParseResult> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    parse_terraform_content(&content, path)
}

/// Parse HCL content from a string (useful for testing without files).
pub fn parse_terraform_content(content: &str, source_path: &Path) -> Result<TerraformParseResult> {
    let body: hcl::Body =
        hcl::from_str(content).with_context(|| format!("parsing HCL in {}", source_path.display()))?;

    let mut result = TerraformParseResult::empty();

    for structure in body.into_iter() {
        if let hcl::Structure::Block(block) = structure {
            match block.identifier.as_str() {
                "resource" => {
                    if let Some(resource) = extract_resource_block(&block, source_path, content) {
                        result.resources.push(resource);
                    }
                }
                "data" => {
                    if let Some(data_source) = extract_data_block(&block, source_path, content) {
                        result.data_sources.push(data_source);
                    }
                }
                _ => {}
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

/// Extract a `TerraformDataSource` from an HCL `data` block.
///
/// A data block has the form: `data "type" "name" { ... }`
/// We only extract blocks whose type starts with `aws_`.
/// The raw `content` is used to resolve the 1-based line number via text search.
fn extract_data_block(block: &hcl::Block, source_path: &Path, content: &str) -> Option<TerraformDataSource> {
    let labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();
    if labels.len() < 2 {
        return None;
    }

    let data_type = labels[0];
    let local_name = labels[1];

    if !data_type.starts_with("aws_") {
        return None;
    }

    let attributes = extract_block_attributes(&block.body);
    let line_number = find_block_line(content, "data", data_type, local_name);

    Some(TerraformDataSource {
        data_type: data_type.to_string(),
        local_name: local_name.to_string(),
        attributes,
        source_file: source_path.to_path_buf(),
        line_number,
    })
}

/// Extract top-level attributes from an HCL block body.
///
/// String literals are stored as `AttributeValue::Literal`.
/// Everything else (variable references, function calls, interpolations,
/// object/list expressions) is stored as `AttributeValue::Expression` with
/// the original text preserved.
fn extract_block_attributes(body: &hcl::Body) -> HashMap<String, AttributeValue> {
    let mut attrs = HashMap::new();

    for structure in body.iter() {
        if let hcl::Structure::Attribute(attr) = structure {
            let key = attr.key.as_str().to_string();
            let value = expression_to_attribute_value(&attr.expr);
            attrs.insert(key, value);
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
/// Searches the raw file content for the pattern. Comment lines (`#` and `//`)
/// are skipped to avoid matching commented-out blocks before the real one.
/// Returns `None` if not found.
fn find_block_line(content: &str, keyword: &str, type_name: &str, local_name: &str) -> Option<usize> {
    // Match patterns like: resource "aws_s3_bucket" "data_bucket"
    let pattern = format!("{keyword} \"{type_name}\" \"{local_name}\"");
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with(&pattern) {
            return Some(i + 1); // 1-based
        }
    }
    None
}

/// Discover all `.tf` files in a directory recursively.
///
/// Skips `.terraform/` directories which contain provider plugins and
/// downloaded module sources that would produce confusing duplicates.
pub(crate) fn discover_tf_files(dir: &Path) -> Vec<PathBuf> {
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
        let resource = &result.resources[0];
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
        assert_eq!(result.resources[0].resource_type, "aws_s3_bucket");
        assert_eq!(result.resources[1].resource_type, "aws_dynamodb_table");
        assert_eq!(result.resources[2].resource_type, "aws_lambda_function");

        // Check DynamoDB attributes
        assert_eq!(
            result.resources[1].attributes.get("name"),
            Some(&AttributeValue::Literal("my-table".to_string()))
        );

        // Check Lambda attributes
        assert_eq!(
            result.resources[2].attributes.get("function_name"),
            Some(&AttributeValue::Literal("my-function".to_string()))
        );
        assert_eq!(
            result.resources[2].attributes.get("handler"),
            Some(&AttributeValue::Literal("index.handler".to_string()))
        );
    }

    #[test]
    fn test_parse_data_source() {
        let hcl = r#"
data "aws_iam_role" "existing_role" {
  name = "my-existing-role"
}
"#;
        let result =
            parse_terraform_content(hcl, Path::new("data.tf")).expect("should parse");

        assert_eq!(result.data_sources.len(), 1);
        let ds = &result.data_sources[0];
        assert_eq!(ds.data_type, "aws_iam_role");
        assert_eq!(ds.local_name, "existing_role");
        assert_eq!(
            ds.attributes.get("name"),
            Some(&AttributeValue::Literal("my-existing-role".to_string()))
        );
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
        assert_eq!(result.resources[0].resource_type, "aws_s3_bucket");
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
        let bucket_attr = result.resources[0].attributes.get("bucket").unwrap();
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
        let bucket_attr = result.resources[0].attributes.get("bucket").unwrap();
        assert!(
            !bucket_attr.is_literal(),
            "Variable reference should be Expression, got: {bucket_attr:?}"
        );
    }

    #[test]
    fn test_mixed_resources_and_data() {
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
        // data "aws_iam_policy_document" has nested blocks which we handle gracefully
        assert_eq!(result.data_sources.len(), 1);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_parse_directory_with_tf_files() {
        let tmp = TempDir::new().expect("create temp dir");

        // Create main.tf
        let main_tf = tmp.path().join("main.tf");
        let mut f = std::fs::File::create(&main_tf).expect("create file");
        writeln!(
            f,
            r#"resource "aws_s3_bucket" "b1" {{ bucket = "bucket-one" }}"#
        )
        .expect("write");

        // Create a subdirectory with another .tf file
        let sub = tmp.path().join("modules");
        std::fs::create_dir_all(&sub).expect("create subdir");
        let sub_tf = sub.join("storage.tf");
        let mut f2 = std::fs::File::create(&sub_tf).expect("create file");
        writeln!(
            f2,
            r#"resource "aws_dynamodb_table" "t1" {{ name = "table-one" }}"#
        )
        .expect("write");

        // Create a non-tf file that should be ignored
        let txt = tmp.path().join("readme.md");
        std::fs::write(&txt, "# readme").expect("write");

        let result = parse_terraform_directory(tmp.path()).expect("should parse dir");

        assert_eq!(result.resources.len(), 2);
        let types: Vec<&str> = result
            .resources
            .iter()
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
        assert!(result.data_sources.is_empty());
    }

    #[test]
    fn test_parse_directory_with_syntax_error() {
        let tmp = TempDir::new().expect("create temp dir");

        // Valid file
        let good = tmp.path().join("good.tf");
        let mut f = std::fs::File::create(&good).expect("create");
        writeln!(
            f,
            r#"resource "aws_s3_bucket" "ok" {{ bucket = "good-bucket" }}"#
        )
        .expect("write");

        // Invalid file
        let bad = tmp.path().join("bad.tf");
        std::fs::write(&bad, "this is not valid HCL {{{{").expect("write");

        let result = parse_terraform_directory(tmp.path()).expect("should parse dir");

        // Good file should still be parsed
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].local_name, "ok");

        // Bad file should produce a warning
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

        assert_eq!(result.resources[0].source_file, PathBuf::from("infra/main.tf"));
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

        let attrs = &result.resources[0].attributes;
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

        assert_eq!(result.resources[0].line_number, Some(2));
        assert_eq!(result.resources[1].line_number, Some(6));
    }

    #[test]
    fn test_find_block_line_not_found() {
        let content = r#"resource "aws_s3_bucket" "exists" {}"#;
        assert!(find_block_line(content, "resource", "aws_s3_bucket", "missing").is_none());
    }

    #[test]
    fn test_find_block_line_skips_hash_comments() {
        let content = r#"
# resource "aws_s3_bucket" "b" {}
resource "aws_s3_bucket" "b" {
  bucket = "real"
}
"#;
        assert_eq!(
            find_block_line(content, "resource", "aws_s3_bucket", "b"),
            Some(3)
        );
    }

    #[test]
    fn test_find_block_line_skips_double_slash_comments() {
        let content = r#"
// resource "aws_s3_bucket" "b" {}
resource "aws_s3_bucket" "b" {
  bucket = "real"
}
"#;
        assert_eq!(
            find_block_line(content, "resource", "aws_s3_bucket", "b"),
            Some(3)
        );
    }

    #[test]
    fn test_discover_skips_dot_terraform_dir() {
        let tmp = TempDir::new().expect("create temp dir");

        // Real .tf file
        let main_tf = tmp.path().join("main.tf");
        std::fs::write(&main_tf, r#"resource "aws_s3_bucket" "b" {}"#).expect("write");

        // .terraform/ directory with a .tf file that should be ignored
        let dot_tf = tmp.path().join(".terraform/providers/main.tf");
        std::fs::create_dir_all(dot_tf.parent().unwrap()).expect("mkdir");
        std::fs::write(&dot_tf, r#"resource "aws_s3_bucket" "cached" {}"#).expect("write");

        let files = discover_tf_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], main_tf);
    }
}
