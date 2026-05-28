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
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Hcl;
use walkdir::WalkDir;

use crate::Location;

use super::{AttributeValue, TerraformResource, TerraformResources};

impl TerraformResources {
    /// Parse all `.tf` files in a directory recursively and add them to this collection.
    ///
    /// Discovers every file ending in `.tf`, parses it with `hcl-rs`, and extends
    /// `self` in-place. Files with syntax errors are recorded as warnings and skipped.
    pub(crate) fn parse_directory(&mut self, dir: &Path) -> Result<()> {
        let tf_files = discover_tf_files(dir);

        if tf_files.is_empty() {
            log::debug!("No .tf files found in {}", dir.display());
            return Ok(());
        }

        log::debug!("Found {} .tf files in {}", tf_files.len(), dir.display());
        let count_before = self.len();

        for tf_file in &tf_files {
            match Self::parse_file(tf_file) {
                Ok(file_result) => self.merge(file_result),
                Err(e) => {
                    let warning = format!("{}: {e}", tf_file.display());
                    log::warn!("Failed to parse Terraform file: {warning}");
                    self.add_warning(warning);
                }
            }
        }

        log::debug!(
            "Parsed {} resources from {} files",
            self.len() - count_before,
            tf_files.len()
        );

        Ok(())
    }

    /// Parse a list of individual `.tf` files and add them to this collection.
    ///
    /// Extends `self` in-place. Files with syntax errors are recorded as
    /// warnings and skipped.
    pub(crate) fn parse_files(&mut self, files: &[PathBuf]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }

        log::debug!("Parsing {} individual .tf files", files.len());
        let count_before = self.len();

        for tf_file in files {
            match Self::parse_file(tf_file) {
                Ok(file_result) => self.merge(file_result),
                Err(e) => {
                    let warning = format!("{}: {e}", tf_file.display());
                    log::warn!("Failed to parse Terraform file: {warning}");
                    self.add_warning(warning);
                }
            }
        }

        log::debug!(
            "Parsed {} resources from {} individual files",
            self.len() - count_before,
            files.len()
        );

        Ok(())
    }

    /// Parse a single `.tf` file and extract AWS resource blocks.
    fn parse_file(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

        parse_terraform_content(&content, path)
    }
}

/// Parse HCL content from a string (useful for testing without files).
fn parse_terraform_content(content: &str, source_path: &Path) -> Result<TerraformResources> {
    let body: hcl::Body = hcl::from_str(content)
        .with_context(|| format!("parsing HCL in {}", source_path.display()))?;

    let mut result = TerraformResources::default();

    for structure in body {
        if let hcl::Structure::Block(block) = structure {
            if block.identifier.as_str() == "resource" {
                if let Some(resource) = extract_resource_block(&block, source_path, content) {
                    result.insert(resource);
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
fn extract_resource_block(
    block: &hcl::Block,
    source_path: &Path,
    content: &str,
) -> Option<TerraformResource> {
    let labels: Vec<&str> = block.labels.iter().map(hcl::BlockLabel::as_str).collect();
    if labels.len() < 2 {
        return None;
    }

    let resource_type = labels[0];
    let local_name = labels[1];

    // Only extract AWS provider resources
    if !resource_type.starts_with(super::AWS_RESOURCE_PREFIX) {
        return None;
    }

    let attributes = extract_block_attributes(&block.body);
    let location = find_block_location(content, source_path, resource_type, local_name)
        .unwrap_or_else(|| Location::new(source_path.to_path_buf(), (1, 1), (1, 1)));

    Some(TerraformResource {
        resource_type: resource_type.to_string(),
        local_name: local_name.to_string(),
        attributes,
        location,
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

    for structure in body {
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

/// Locate a Terraform resource block in the source content and return a [`Location`]
/// with accurate line and column information using ast-grep tree-sitter parsing.
///
/// Parses the content with ast-grep's HCL language support and uses the pattern
/// `resource $TYPE $NAME $BODY` to find resource blocks. Matches by checking the
/// `$TYPE` and `$NAME` metavariable values against the provided `type_name` and
/// `local_name`.
///
/// The returned `Location` spans the entire resource block (from `resource` keyword
/// to the closing `}`), providing precise AST-level positioning.
///
/// Returns `None` if not found; the caller falls back to a (1, 1) location.
fn find_block_location(
    content: &str,
    source_path: &Path,
    resource_type: &str,
    local_name: &str,
) -> Option<Location> {
    let ast_grep = Hcl.ast_grep(content);
    let root = ast_grep.root();

    let pattern = "resource $TYPE $NAME $BODY";

    for node_match in root.find_all(pattern) {
        let env = node_match.get_env();

        let matched_type = env.get_match("TYPE")?;
        let matched_name = env.get_match("NAME")?;

        // Strip surrounding quotes for comparison
        let type_text = matched_type.text();
        let name_text = matched_name.text();
        let matched_type_text = type_text.trim_matches('"');
        let matched_name_text = name_text.trim_matches('"');

        if matched_type_text == resource_type && matched_name_text == local_name {
            let node = node_match.get_node();
            return Some(Location::from_node(source_path.to_path_buf(), node));
        }
    }
    None
}

/// Discover all `.tf` files in a directory recursively.
///
/// Skips `.terraform/` directories which contain provider plugins and
/// downloaded module sources that would produce confusing duplicates.
///
/// Results are sorted lexicographically to match Terraform's processing
/// order and ensure deterministic behavior when multiple files define the
/// same variable defaults.
pub(super) fn discover_tf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| !(entry.file_type().is_dir() && entry.file_name() == ".terraform"))
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "tf")
        })
        .map(walkdir::DirEntry::into_path)
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Expected resource for parameterized content-parsing tests
    // -----------------------------------------------------------------------

    /// Describes one expected resource from HCL parsing.
    struct ExpectedResource {
        resource_type: &'static str,
        local_name: &'static str,
        /// (attr_key, expected_value) pairs to assert. If empty, no attribute checks.
        attributes: Vec<(&'static str, AttributeValue)>,
        /// Expected location (None = don't check).
        /// When provided, validates line and column positions.
        expected_location: Option<(usize, usize, usize, usize)>, // (start_line, start_col, end_line, end_col)
    }

    /// Shared assertion: parse HCL content and compare against expected resources.
    fn assert_parse(
        hcl: &str,
        source_path: &str,
        expected_count: usize,
        expected_resources: &[ExpectedResource],
    ) {
        let result = parse_terraform_content(hcl, Path::new(source_path)).expect("should parse");
        assert_eq!(
            result.len(),
            expected_count,
            "resource count mismatch for HCL input"
        );

        for expected in expected_resources {
            let resource = result
                .get(expected.resource_type, expected.local_name)
                .unwrap_or_else(|| {
                    panic!(
                        "expected resource {}.{} not found",
                        expected.resource_type, expected.local_name
                    )
                });
            assert_eq!(resource.resource_type, expected.resource_type);
            assert_eq!(resource.local_name, expected.local_name);

            for (attr_key, expected_value) in &expected.attributes {
                let actual = resource.attributes.get(*attr_key).unwrap_or_else(|| {
                    panic!(
                        "attribute '{}' not found on {}.{}",
                        attr_key, expected.resource_type, expected.local_name
                    )
                });
                assert_eq!(
                    actual, expected_value,
                    "attribute '{}' mismatch on {}.{}",
                    attr_key, expected.resource_type, expected.local_name
                );
            }

            // Always verify file_path is preserved from source_path
            assert_eq!(
                resource.location.file_path,
                PathBuf::from(source_path),
                "file_path mismatch for {}.{}",
                expected.resource_type,
                expected.local_name
            );

            if let Some((start_line, start_col, end_line, end_col)) = expected.expected_location {
                let loc = &resource.location;
                assert_eq!(
                    loc.start_line(),
                    start_line,
                    "start_line mismatch for {}.{}",
                    expected.resource_type,
                    expected.local_name
                );
                assert_eq!(
                    loc.start_col(),
                    start_col,
                    "start_col mismatch for {}.{}",
                    expected.resource_type,
                    expected.local_name
                );
                assert_eq!(
                    loc.end_line(),
                    end_line,
                    "end_line mismatch for {}.{}",
                    expected.resource_type,
                    expected.local_name
                );
                assert_eq!(
                    loc.end_col(),
                    end_col,
                    "end_col mismatch for {}.{}",
                    expected.resource_type,
                    expected.local_name
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Parameterized HCL content parsing tests
    // -----------------------------------------------------------------------

    #[rstest]
    // Simple S3 bucket with literal attribute
    #[case(
        "simple_s3_bucket",
        "main.tf",
        r#"
resource "aws_s3_bucket" "data_bucket" {
  bucket = "my-app-data"
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket",
            local_name: "data_bucket",
            attributes: vec![("bucket", AttributeValue::Literal("my-app-data".into()))],
            // ast-grep spans entire block: line 2 col 1 to line 4 col 2 (after `}`)
            expected_location: Some((2, 1, 4, 2)),
        }],
    )]
    // Multiple resources: S3 + DynamoDB + Lambda
    #[case(
        "multiple_resources",
        "main.tf",
        r#"
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
"#,
        3,
        vec![
            ExpectedResource {
                resource_type: "aws_s3_bucket", local_name: "bucket1",
                attributes: vec![("bucket", AttributeValue::Literal("first-bucket".into()))],
                expected_location: Some((2, 1, 4, 2)),
            },
            ExpectedResource {
                resource_type: "aws_dynamodb_table", local_name: "table1",
                attributes: vec![("name", AttributeValue::Literal("my-table".into()))],
                expected_location: Some((6, 1, 9, 2)),
            },
            ExpectedResource {
                resource_type: "aws_lambda_function", local_name: "func1",
                attributes: vec![("function_name", AttributeValue::Literal("my-function".into()))],
                expected_location: Some((11, 1, 16, 2)),
            },
        ],
    )]
    // Non-AWS resources are ignored
    #[case(
        "non_aws_ignored",
        "main.tf",
        r#"
resource "google_storage_bucket" "gcs" { name = "my-gcs-bucket" }
resource "aws_s3_bucket" "s3" { bucket = "my-s3-bucket" }
resource "azurerm_storage_account" "azure" { name = "myazurestorage" }
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "s3",
            attributes: vec![("bucket", AttributeValue::Literal("my-s3-bucket".into()))],
            // Single-line block: ast-grep spans to end of `}`
            expected_location: Some((3, 1, 3, 58)),
        }],
    )]
    // Interpolation preserved as Expression
    #[case(
        "expression_interpolation",
        "main.tf",
        r#"
resource "aws_s3_bucket" "dynamic_bucket" {
  bucket = "${var.prefix}-data-bucket"
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "dynamic_bucket",
            attributes: vec![("bucket", AttributeValue::Expression("${var.prefix}-data-bucket".into()))],
            // ast-grep spans entire block
            expected_location: Some((2, 1, 4, 2)),
        }],
    )]
    // Bare variable reference preserved as Expression
    #[case(
        "expression_bare_var",
        "main.tf",
        r#"
resource "aws_s3_bucket" "var_bucket" {
  bucket = var.bucket_name
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "var_bucket",
            attributes: vec![("bucket", AttributeValue::Expression("var.bucket_name".into()))],
            // ast-grep spans entire block
            expected_location: Some((2, 1, 4, 2)),
        }],
    )]
    // Data, variable, output blocks are ignored
    #[case(
        "non_resource_blocks_ignored",
        "main.tf",
        r#"
resource "aws_s3_bucket" "bucket" { bucket = "my-bucket" }
data "aws_iam_policy_document" "policy" {
  statement {
    actions = ["s3:GetObject"]
  }
}
variable "region" { default = "us-east-1" }
output "bucket_arn" { value = aws_s3_bucket.bucket.arn }
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "bucket",
            attributes: vec![("bucket", AttributeValue::Literal("my-bucket".into()))],
            // Single-line block: `resource "aws_s3_bucket" "bucket" { bucket = "my-bucket" }` = 59 chars
            expected_location: Some((2, 1, 2, 59)),
        }],
    )]
    // Numeric and bool attributes stored as Literal strings
    #[case(
        "numeric_and_bool_attrs",
        "main.tf",
        r#"
resource "aws_dynamodb_table" "t" {
  name           = "my-table"
  read_capacity  = 5
  write_capacity = 10
  stream_enabled = true
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_dynamodb_table", local_name: "t",
            attributes: vec![
                ("name", AttributeValue::Literal("my-table".into())),
                ("read_capacity", AttributeValue::Literal("5".into())),
                ("stream_enabled", AttributeValue::Literal("true".into())),
            ],
            // ast-grep spans entire block
            expected_location: Some((2, 1, 7, 2)),
        }],
    )]
    // Location tracking with line and column
    #[case(
        "location_tracking",
        "main.tf",
        r#"
resource "aws_s3_bucket" "first" {
  bucket = "first-bucket"
}

resource "aws_s3_bucket" "second" {
  bucket = "second-bucket"
}
"#,
        2,
        vec![
            ExpectedResource {
                resource_type: "aws_s3_bucket", local_name: "first",
                attributes: vec![],
                // ast-grep spans entire block
                expected_location: Some((2, 1, 4, 2)),
            },
            ExpectedResource {
                resource_type: "aws_s3_bucket", local_name: "second",
                attributes: vec![],
                expected_location: Some((6, 1, 8, 2)),
            },
        ],
    )]
    // Comments skipped for location resolution
    #[case(
        "comments_skipped",
        "main.tf",
        r#"
# resource "aws_s3_bucket" "b" {
//   bucket = "commented-out"
// }

resource "aws_s3_bucket" "b" {
  bucket = "real-bucket"
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "b",
            attributes: vec![("bucket", AttributeValue::Literal("real-bucket".into()))],
            // ast-grep spans entire block
            expected_location: Some((6, 1, 8, 2)),
        }],
    )]
    // Empty body resource
    #[case(
        "empty_body",
        "main.tf",
        r#"
resource "aws_s3_bucket" "empty" {}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "empty",
            attributes: vec![],
            // Single-line: `resource "aws_s3_bucket" "empty" {}` = 35 chars → end col 36
            expected_location: Some((2, 1, 2, 36)),
        }],
    )]
    // Source file path preserved in location (non-default path)
    #[case(
        "source_file_path_preserved",
        "infra/main.tf",
        r#"
resource "aws_s3_bucket" "b" {
  bucket = "test"
}
"#,
        1,
        vec![ExpectedResource {
            resource_type: "aws_s3_bucket", local_name: "b",
            attributes: vec![("bucket", AttributeValue::Literal("test".into()))],
            expected_location: Some((2, 1, 4, 2)),
        }],
    )]
    fn test_parse_hcl_content(
        #[case] _name: &str,
        #[case] source_path: &str,
        #[case] hcl: &str,
        #[case] expected_count: usize,
        #[case] expected_resources: Vec<ExpectedResource>,
    ) {
        assert_parse(hcl, source_path, expected_count, &expected_resources);
    }

    // -----------------------------------------------------------------------
    // Directory-based tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for directory-based HCL parsing tests.
    ///
    /// Creates a temp directory, writes the given files (supporting nested paths),
    /// parses the directory, and asserts resource count, expected resource types,
    /// and optional warning content.
    fn assert_directory_parse(
        files: &[(&str, &str)],
        expected_count: usize,
        expected_types: &[&str],
        expected_warning_contains: Option<&str>,
    ) {
        let tmp = TempDir::new().expect("create temp dir");

        for (rel_path, content) in files {
            let path = tmp.path().join(rel_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent dirs");
            }
            std::fs::write(&path, content).expect("write file");
        }

        let mut result = TerraformResources::default();
        result
            .parse_directory(tmp.path())
            .expect("should parse dir");

        assert_eq!(result.len(), expected_count, "resource count mismatch");

        let types: Vec<&str> = result.values().map(|r| r.resource_type.as_str()).collect();
        for expected_type in expected_types {
            assert!(
                types.contains(expected_type),
                "expected resource type '{expected_type}' not found in {types:?}"
            );
        }

        match expected_warning_contains {
            None => assert!(result.warnings().is_empty(), "expected no warnings"),
            Some(substr) => {
                assert!(
                    !result.warnings().is_empty(),
                    "expected warnings but got none"
                );
                assert!(
                    result.warnings().iter().any(|w| w.contains(substr)),
                    "no warning contains '{substr}', got: {:?}",
                    result.warnings()
                );
            }
        }
    }

    #[rstest]
    // Multiple .tf files across subdirectories, non-tf files ignored
    #[case(
        "multiple_tf_files",
        &[
            ("main.tf", "resource \"aws_s3_bucket\" \"b1\" { bucket = \"bucket-one\" }"),
            ("modules/storage.tf", "resource \"aws_dynamodb_table\" \"t1\" { name = \"table-one\" }"),
            ("readme.md", "# readme"),
        ],
        2,
        &["aws_s3_bucket", "aws_dynamodb_table"],
        None
    )]
    // No .tf files → empty result
    #[case(
        "no_tf_files",
        &[("readme.md", "hello")],
        0,
        &[],
        None
    )]
    // Syntax error in one file → good file parsed, warning for bad file
    #[case(
        "syntax_error_partial_parse",
        &[
            ("good.tf", "resource \"aws_s3_bucket\" \"ok\" { bucket = \"good-bucket\" }"),
            ("bad.tf", "this is not valid HCL {{{{"),
        ],
        1,
        &["aws_s3_bucket"],
        Some("bad.tf")
    )]
    // .terraform directory is skipped (only root main.tf found)
    #[case(
        "dot_terraform_skipped",
        &[
            ("main.tf", "resource \"aws_s3_bucket\" \"b\" {}"),
            (".terraform/providers/main.tf", "resource \"aws_s3_bucket\" \"cached\" {}"),
        ],
        1,
        &["aws_s3_bucket"],
        None
    )]
    fn test_parse_directory(
        #[case] _name: &str,
        #[case] files: &[(&str, &str)],
        #[case] expected_count: usize,
        #[case] expected_types: &[&str],
        #[case] expected_warning_contains: Option<&str>,
    ) {
        assert_directory_parse(
            files,
            expected_count,
            expected_types,
            expected_warning_contains,
        );
    }

    // -----------------------------------------------------------------------
    // find_block_location unit tests
    // -----------------------------------------------------------------------

    #[rstest]
    #[case(
        "basic_at_line_1",
        "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\"\n}",
        Some((1, 1, 3, 2))
    )]
    #[case(
        "with_leading_blank_line",
        "\nresource \"aws_s3_bucket\" \"b\" {\n}",
        Some((2, 1, 3, 2))
    )]
    #[case(
        "indented_2_spaces",
        "  resource \"aws_s3_bucket\" \"b\" {\n}",
        Some((1, 3, 2, 2))
    )]
    #[case(
        "indented_4_spaces",
        "    resource \"aws_s3_bucket\" \"b\" {\n}",
        Some((1, 5, 2, 2))
    )]
    #[case(
        "after_comment_lines",
        "# comment\n// another comment\nresource \"aws_s3_bucket\" \"b\" {\n}",
        Some((3, 1, 4, 2))
    )]
    #[case(
        "after_block_comment",
        "/* block\ncomment */\nresource \"aws_s3_bucket\" \"b\" {\n}",
        Some((3, 1, 4, 2))
    )]
    #[case(
        "extra_whitespace_between_tokens",
        "resource   \"aws_s3_bucket\"   \"b\" {\n}",
        Some((1, 1, 2, 2))
    )]
    // Block not found → None
    #[case(
        "not_found_different_type",
        "resource \"aws_dynamodb_table\" \"t\" {\n}",
        None
    )]
    // Inside comment → not matched
    #[case(
        "inside_comment_not_matched",
        "# resource \"aws_s3_bucket\" \"b\" {\n# }",
        None
    )]
    fn test_find_block_location(
        #[case] _name: &str,
        #[case] content: &str,
        #[case] expected: Option<(usize, usize, usize, usize)>,
    ) {
        let result = find_block_location(content, Path::new("test.tf"), "aws_s3_bucket", "b");
        match expected {
            None => assert!(result.is_none(), "[{_name}] expected None"),
            Some((start_line, start_col, end_line, end_col)) => {
                let loc =
                    result.unwrap_or_else(|| panic!("[{_name}] expected location to be found"));
                assert_eq!(loc.start_line(), start_line, "[{_name}] start_line");
                assert_eq!(loc.start_col(), start_col, "[{_name}] start_col");
                assert_eq!(loc.end_line(), end_line, "[{_name}] end_line");
                assert_eq!(loc.end_col(), end_col, "[{_name}] end_col");
                assert_eq!(
                    loc.file_path,
                    PathBuf::from("test.tf"),
                    "[{_name}] file_path"
                );
            }
        }
    }
}
