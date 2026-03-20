//! Terraform state file (`terraform.tfstate`) parser.
//!
//! Reads the v4 JSON format and extracts deployed AWS resource ARNs.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::Json;
use serde::Deserialize;

use crate::Location;

/// A resource instance extracted from `terraform.tfstate`.
///
/// Identity is based on `(resource_type, name, arn)` — the location is
/// metadata for diagnostics and does not affect equality or hashing.
#[derive(Debug, Clone)]
pub struct StateResource {
    /// Terraform resource type (e.g., `"aws_s3_bucket"`)
    pub resource_type: String,
    /// Local name in Terraform config (e.g., `"data_bucket"`)
    pub name: String,
    /// Full ARN if present in state attributes
    pub arn: Option<String>,
    /// Location of the ARN value in the state file (if found)
    pub arn_location: Option<Location>,
}

impl PartialEq for StateResource {
    fn eq(&self, other: &Self) -> bool {
        self.resource_type == other.resource_type
            && self.name == other.name
            && self.arn == other.arn
    }
}

impl Eq for StateResource {}

impl std::hash::Hash for StateResource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.resource_type.hash(state);
        self.name.hash(state);
        self.arn.hash(state);
    }
}

/// A collection of resource instances extracted from `terraform.tfstate` files.
///
/// Resources are keyed internally by `(resource_type, local_name)`.
/// Multiple instances (from `count` / `for_each`) share the same key.
#[derive(Debug, Clone)]
pub struct TerraformStateResources {
    resources: HashMap<(String, String), HashSet<StateResource>>,
}

impl TerraformStateResources {
    /// Create an empty collection.
    #[must_use]
    pub fn default() -> Self {
        Self {
            resources: HashMap::new(),
        }
    }

    /// Parse a single `terraform.tfstate` file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading state file: {}", path.display()))?;

        parse_terraform_state_content(&content, path)
    }

    /// Parse multiple `terraform.tfstate` files and merge them.
    pub fn from_files(paths: &[std::path::PathBuf]) -> Result<Self> {
        let mut result = Self::default();
        for path in paths {
            result.merge(Self::from_file(path)?);
        }
        Ok(result)
    }

    /// Look up state resources by `(resource_type, local_name)`.
    /// Look up state resources by `(resource_type, local_name)`.
    ///
    /// Returns a reference to the set of resources, or `None` if not present.
    #[must_use]
    pub fn get(&self, resource_type: &str, local_name: &str) -> Option<&HashSet<StateResource>> {
        self.resources
            .get(&(resource_type.to_string(), local_name.to_string()))
    }

    /// Number of distinct `(resource_type, local_name)` groups.
    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Returns `true` if the given key is present.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn contains_key(&self, resource_type: &str, local_name: &str) -> bool {
        self.resources
            .contains_key(&(resource_type.to_string(), local_name.to_string()))
    }

    /// Merge another collection into this one.
    /// Merge another collection into this one.
    ///
    /// Resources from `other` are added. Duplicates (same identity) are
    /// automatically deduplicated by the `HashSet`.
    pub fn merge(&mut self, other: Self) {
        for (key, resources) in other.resources {
            self.resources.entry(key).or_default().extend(resources);
        }
    }

    /// Insert a resource instance into the collection.
    ///
    /// Duplicates are automatically ignored by the `HashSet`.
    pub(crate) fn push(&mut self, resource: StateResource) {
        let key = (resource.resource_type.clone(), resource.name.clone());
        self.resources.entry(key).or_default().insert(resource);
    }
}

/// Parse tfstate JSON content (useful for testing without files).
fn parse_terraform_state_content(
    content: &str,
    source_path: &Path,
) -> Result<TerraformStateResources> {
    let state: RawState =
        serde_json::from_str(content).context("parsing terraform.tfstate JSON")?;

    match state.version {
        Some(v) if v >= 4 => {} // OK
        Some(v) => anyhow::bail!(
            "unsupported terraform state version {v}: only version 4 or later is supported"
        ),
        None => anyhow::bail!("terraform state file is missing the 'version' field"),
    }

    let mut result = TerraformStateResources::default();

    let resources = state.resources.unwrap_or_default();
    for raw_resource in &resources {
        // Only process AWS resources
        if !raw_resource
            .resource_type
            .starts_with(super::AWS_RESOURCE_PREFIX)
        {
            continue;
        }

        // Skip data sources in state (mode == "data")
        if raw_resource.mode.as_deref() == Some("data") {
            continue;
        }

        for instance in &raw_resource.instances {
            let arn = instance
                .attributes
                .get("arn")
                .and_then(|v| v.as_str())
                .map(String::from);

            let arn_location = arn
                .as_deref()
                .and_then(|arn_val| find_string_location(content, source_path, arn_val));

            result.push(StateResource {
                resource_type: raw_resource.resource_type.clone(),
                name: raw_resource.name.clone(),
                arn,
                arn_location,
            });
        }
    }

    Ok(result)
}

/// Find the location of an ARN value in JSON content using ast-grep tree-sitter parsing.
///
/// Parses the content with ast-grep's JSON language support and walks the tree-sitter
/// AST to find `pair` nodes where the key is `"arn"`. Matches by comparing the value
/// text (stripped of quotes) against the provided `value`.
///
/// Returns a `Location` pointing at the ARN value node (the string literal including quotes).
fn find_string_location(content: &str, source_path: &Path, value: &str) -> Option<Location> {
    let ast_grep = Json.ast_grep(content);
    let root = ast_grep.root();

    // Walk all nodes via DFS to find "pair" nodes with key "arn"
    find_arn_value_in_tree(&root, source_path, value)
}

/// Recursively walk the AST tree to find a JSON pair node with key `"arn"` whose
/// value matches the given string. Returns the `Location` of the value node.
fn find_arn_value_in_tree<D: ast_grep_core::Doc>(
    node: &ast_grep_core::Node<D>,
    source_path: &Path,
    value: &str,
) -> Option<Location> {
    if node.kind() == "pair" {
        // A JSON pair node has children: key (string), ":", value
        // Check if the key is "arn"
        let mut children = node.children();
        if let Some(key_node) = children.next() {
            let key_text = key_node.text();
            if key_text.trim_matches('"') == "arn" {
                // Find the value child (skip the ":" separator)
                for child in children {
                    if child.kind() != ":" {
                        let child_text = child.text();
                        let unquoted = child_text.trim_matches('"');
                        if unquoted == value {
                            let start = child.start_pos();
                            let end = child.end_pos();
                            return Some(Location::new(
                                source_path.to_path_buf(),
                                (start.line() + 1, start.column(&child) + 1),
                                (end.line() + 1, end.column(&child) + 1),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Recurse into children
    for child in node.children() {
        if let Some(loc) = find_arn_value_in_tree(&child, source_path, value) {
            return Some(loc);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Raw deserialization types for terraform.tfstate v4 format
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawState {
    version: Option<u64>,
    resources: Option<Vec<RawResource>>,
}

#[derive(Deserialize)]
struct RawResource {
    #[serde(rename = "type")]
    resource_type: String,
    name: String,
    mode: Option<String>,
    #[serde(default)]
    instances: Vec<RawInstance>,
}

#[derive(Deserialize)]
struct RawInstance {
    #[serde(default)]
    attributes: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    fn sample_state_json() -> &'static str {
        r#"{
  "version": 4,
  "terraform_version": "1.5.0",
  "resources": [
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "data_bucket",
      "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
      "instances": [
        {
          "attributes": {
            "arn": "arn:aws:s3:::my-app-data-bucket",
            "bucket": "my-app-data-bucket",
            "id": "my-app-data-bucket"
          }
        }
      ]
    },
    {
      "mode": "managed",
      "type": "aws_dynamodb_table",
      "name": "users_table",
      "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
      "instances": [
        {
          "attributes": {
            "arn": "arn:aws:dynamodb:us-east-1:123456789012:table/users-table",
            "name": "users-table",
            "id": "users-table"
          }
        }
      ]
    },
    {
      "mode": "managed",
      "type": "aws_sqs_queue",
      "name": "task_queue",
      "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
      "instances": [
        {
          "attributes": {
            "arn": "arn:aws:sqs:us-east-1:123456789012:task-processing-queue",
            "name": "task-processing-queue",
            "id": "https://sqs.us-east-1.amazonaws.com/123456789012/task-processing-queue"
          }
        }
      ]
    },
    {
      "mode": "data",
      "type": "aws_caller_identity",
      "name": "current",
      "instances": [
        {
          "attributes": {
            "account_id": "123456789012",
            "id": "123456789012"
          }
        }
      ]
    }
  ]
}"#
    }

    // -----------------------------------------------------------------------
    // Parameterized version validation tests
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("version_3_rejected", r#"{"version": 3, "resources": []}"#, true)]
    #[case("missing_version_rejected", r#"{"resources": []}"#, true)]
    #[case("malformed_json_rejected", "not json", true)]
    #[case("version_4_accepted", r#"{"version": 4, "resources": []}"#, false)]
    #[case("version_5_accepted", r#"{"version": 5, "resources": []}"#, false)]
    #[case("empty_resources", r#"{"version": 4, "resources": []}"#, false)]
    #[case("no_resources_key", r#"{"version": 4}"#, false)]
    fn test_state_parsing_edge_cases(
        #[case] _name: &str,
        #[case] json: &str,
        #[case] is_err: bool,
    ) {
        let result = parse_terraform_state_content(json, Path::new("test.tfstate"));
        assert_eq!(
            result.is_err(),
            is_err,
            "[{_name}] expected is_err={is_err}, got {:?}",
            result.as_ref().err()
        );
    }

    // -----------------------------------------------------------------------
    // State parsing tests (shared harness)
    // -----------------------------------------------------------------------

    /// Expected resource for parameterized state parsing tests.
    struct ExpectedStateResource {
        resource_type: &'static str,
        name: &'static str,
        arn: Option<&'static str>,
        instance_count: usize,
    }

    /// Shared harness for state file content parsing tests.
    fn assert_state_parse(
        json: &str,
        expected_count: usize,
        expected_resources: &[ExpectedStateResource],
        expected_absent: &[(&str, &str)],
    ) {
        let map = parse_terraform_state_content(json, Path::new("terraform.tfstate")).expect("parse");
        assert_eq!(map.len(), expected_count, "resource group count mismatch");

        for exp in expected_resources {
            let resources = map
                .get(exp.resource_type, exp.name)
                .unwrap_or_else(|| panic!("{}.{} not found", exp.resource_type, exp.name));
            assert_eq!(
                resources.len(),
                exp.instance_count,
                "instance count mismatch for {}.{}",
                exp.resource_type,
                exp.name
            );
            if let Some(expected_arn) = exp.arn {
                let r = resources.iter().next().unwrap();
                assert_eq!(
                    r.arn.as_deref(),
                    Some(expected_arn),
                    "ARN mismatch for {}.{}",
                    exp.resource_type,
                    exp.name
                );
            }
        }

        for (rtype, name) in expected_absent {
            assert!(
                !map.contains_key(rtype, name),
                "{rtype}.{name} should be absent"
            );
        }
    }

    #[rstest]
    // Sample state: 3 managed resources, data source skipped, ARNs extracted
    #[case(
        "sample_state",
        None, // use sample_state_json()
        3,
        vec![
            ExpectedStateResource { resource_type: "aws_s3_bucket", name: "data_bucket", arn: Some("arn:aws:s3:::my-app-data-bucket"), instance_count: 1 },
            ExpectedStateResource { resource_type: "aws_dynamodb_table", name: "users_table", arn: Some("arn:aws:dynamodb:us-east-1:123456789012:table/users-table"), instance_count: 1 },
            ExpectedStateResource { resource_type: "aws_sqs_queue", name: "task_queue", arn: Some("arn:aws:sqs:us-east-1:123456789012:task-processing-queue"), instance_count: 1 },
        ],
        vec![("aws_caller_identity", "current")]
    )]
    // Multiple instances grouped under one key
    #[case(
        "multiple_instances",
        Some(r#"{"version": 4, "resources": [{"mode": "managed", "type": "aws_s3_bucket", "name": "data", "instances": [{"attributes": {"arn": "arn:aws:s3:::bucket-0"}}, {"attributes": {"arn": "arn:aws:s3:::bucket-1"}}]}]}"#),
        1,
        vec![ExpectedStateResource { resource_type: "aws_s3_bucket", name: "data", arn: None, instance_count: 2 }],
        vec![]
    )]
    // Non-AWS resources skipped
    #[case(
        "non_aws_skipped",
        Some(r#"{"version": 4, "resources": [{"mode": "managed", "type": "google_storage_bucket", "name": "gcs", "instances": [{"attributes": {"name": "my-gcs-bucket"}}]}, {"mode": "managed", "type": "aws_s3_bucket", "name": "s3", "instances": [{"attributes": {"arn": "arn:aws:s3:::my-s3-bucket"}}]}]}"#),
        1,
        vec![ExpectedStateResource { resource_type: "aws_s3_bucket", name: "s3", arn: Some("arn:aws:s3:::my-s3-bucket"), instance_count: 1 }],
        vec![]
    )]
    // Resource without ARN
    #[case(
        "resource_without_arn",
        Some(r#"{"version": 4, "resources": [{"mode": "managed", "type": "aws_s3_bucket", "name": "no_arn", "instances": [{"attributes": {"bucket": "my-bucket", "id": "my-bucket"}}]}]}"#),
        1,
        vec![ExpectedStateResource { resource_type: "aws_s3_bucket", name: "no_arn", arn: None, instance_count: 1 }],
        vec![]
    )]
    fn test_state_content_parsing(
        #[case] _name: &str,
        #[case] json_override: Option<&str>,
        #[case] expected_count: usize,
        #[case] expected_resources: Vec<ExpectedStateResource>,
        #[case] expected_absent: Vec<(&str, &str)>,
    ) {
        let json = json_override.unwrap_or_else(|| sample_state_json());
        assert_state_parse(json, expected_count, &expected_resources, &expected_absent);
    }

    // -----------------------------------------------------------------------
    // ARN location tracking tests (shared harness)
    // -----------------------------------------------------------------------

    /// Shared harness for ARN location tests.
    fn assert_arn_location(
        json: &str,
        file_name: &str,
        resource_type: &str,
        name: &str,
        expected_line: usize,
        expected_start_col: usize,
        expected_end_col: usize,
    ) {
        let map = parse_terraform_state_content(json, Path::new(file_name)).expect("parse");
        let r = map
            .get(resource_type, name)
            .unwrap_or_else(|| panic!("{resource_type}.{name} not found"))
            .iter()
            .next()
            .unwrap();
        let loc = r.arn_location.as_ref().expect("should have location");
        assert_eq!(loc.file_path, PathBuf::from(file_name), "file_path mismatch");
        assert_eq!(loc.start_line(), expected_line, "start_line mismatch");
        assert_eq!(loc.start_col(), expected_start_col, "start_col mismatch");
        assert_eq!(loc.end_col(), expected_end_col, "end_col mismatch");
    }

    #[rstest]
    // S3 bucket in sample state: line 13, col 20-53
    #[case(
        "sample_s3",
        None,
        "terraform.tfstate", "aws_s3_bucket", "data_bucket",
        13, 20, 53
    )]
    // DynamoDB in sample state: line 28
    #[case(
        "sample_dynamodb",
        None,
        "terraform.tfstate", "aws_dynamodb_table", "users_table",
        28, 20, 79
    )]
    // Minimal state with exact positions
    #[case(
        "minimal_exact_positions",
        Some("{\n  \"version\": 4,\n  \"resources\": [\n    {\n      \"mode\": \"managed\",\n      \"type\": \"aws_s3_bucket\",\n      \"name\": \"b\",\n      \"instances\": [\n        {\n          \"attributes\": {\n            \"arn\": \"arn:aws:s3:::test-bucket\"\n          }\n        }\n      ]\n    }\n  ]\n}"),
        "state.tfstate", "aws_s3_bucket", "b",
        11, 20, 46
    )]
    fn test_arn_location(
        #[case] _name: &str,
        #[case] json_override: Option<&str>,
        #[case] file_name: &str,
        #[case] resource_type: &str,
        #[case] name: &str,
        #[case] expected_line: usize,
        #[case] expected_start_col: usize,
        #[case] expected_end_col: usize,
    ) {
        let json = json_override.unwrap_or_else(|| sample_state_json());
        assert_arn_location(json, file_name, resource_type, name, expected_line, expected_start_col, expected_end_col);
    }

    // -----------------------------------------------------------------------
    // find_string_location tests (shared harness)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case(
        "arn_pattern",
        r#"{"arn": "arn:aws:s3:::my-bucket"}"#,
        "arn:aws:s3:::my-bucket",
        Some((1, 9))
    )]
    #[case(
        "not_found",
        r#"{"bucket": "my-bucket"}"#,
        "nonexistent",
        None
    )]
    #[case(
        "multiline",
        "{\n  \"name\": \"test\",\n  \"arn\": \"arn:aws:s3:::b\"\n}",
        "arn:aws:s3:::b",
        Some((3, 10))
    )]
    fn test_find_string_location(
        #[case] _name: &str,
        #[case] content: &str,
        #[case] search_value: &str,
        #[case] expected: Option<(usize, usize)>,
    ) {
        let result = find_string_location(content, Path::new("s.json"), search_value);
        match expected {
            None => assert!(result.is_none(), "expected None"),
            Some((line, col)) => {
                let loc = result.expect("expected Some");
                assert_eq!(loc.start_line(), line, "line mismatch");
                assert_eq!(loc.start_col(), col, "col mismatch");
            }
        }
    }
}
