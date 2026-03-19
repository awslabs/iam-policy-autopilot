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

    /// Returns `true` if there are no state resources.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Returns `true` if the given key is present.
    #[must_use]
    pub fn contains_key(&self, resource_type: &str, local_name: &str) -> bool {
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
    // Sample state file tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_state_extracts_resources() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");
        assert_eq!(map.len(), 3, "3 managed resources (data source skipped)");
    }

    #[test]
    fn test_parse_state_extracts_arns() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");

        let s3_set = map.get("aws_s3_bucket", "data_bucket").expect("s3");
        let s3 = s3_set.iter().next().unwrap();
        assert_eq!(s3.arn.as_deref(), Some("arn:aws:s3:::my-app-data-bucket"));
        assert_eq!(s3.name, "data_bucket");

        let ddb = map.get("aws_dynamodb_table", "users_table").expect("ddb").iter().next().unwrap();
        assert_eq!(
            ddb.arn.as_deref(),
            Some("arn:aws:dynamodb:us-east-1:123456789012:table/users-table")
        );
    }

    #[test]
    fn test_parse_state_skips_data_sources() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");
        assert!(
            !map.contains_key("aws_caller_identity", "current"),
            "Data sources should be skipped"
        );
    }

    #[test]
    fn test_resources_keyed_by_type_and_name() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");
        assert!(map.contains_key("aws_s3_bucket", "data_bucket"));
        assert!(map.contains_key("aws_dynamodb_table", "users_table"));
        assert!(map.contains_key("aws_sqs_queue", "task_queue"));
    }

    #[test]
    fn test_multiple_instances_grouped_under_one_key() {
        let json = r#"{
  "version": 4,
  "resources": [
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "data",
      "instances": [
        { "attributes": { "arn": "arn:aws:s3:::bucket-0" } },
        { "attributes": { "arn": "arn:aws:s3:::bucket-1" } }
      ]
    }
  ]
}"#;
        let map = parse_terraform_state_content(json, Path::new("terraform.tfstate")).expect("parse");
        let buckets = map.get("aws_s3_bucket", "data").expect("buckets");
        assert_eq!(buckets.len(), 2);
        let arns: std::collections::BTreeSet<_> = buckets.iter().filter_map(|b| b.arn.as_deref()).collect();
        assert!(arns.contains("arn:aws:s3:::bucket-0"));
        assert!(arns.contains("arn:aws:s3:::bucket-1"));
    }

    #[test]
    fn test_non_aws_resources_skipped() {
        let json = r#"{
  "version": 4,
  "resources": [
    {
      "mode": "managed",
      "type": "google_storage_bucket",
      "name": "gcs",
      "instances": [{ "attributes": { "name": "my-gcs-bucket" } }]
    },
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "s3",
      "instances": [{ "attributes": { "arn": "arn:aws:s3:::my-s3-bucket" } }]
    }
  ]
}"#;
        let map = parse_terraform_state_content(json, Path::new("terraform.tfstate")).expect("parse");
        assert_eq!(map.len(), 1, "Only AWS resources should be included");
        assert!(map.contains_key("aws_s3_bucket", "s3"));
    }

    #[test]
    fn test_resource_without_arn() {
        let json = r#"{
  "version": 4,
  "resources": [
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "no_arn",
      "instances": [
        {
          "attributes": {
            "bucket": "my-bucket",
            "id": "my-bucket"
          }
        }
      ]
    }
  ]
}"#;
        let map = parse_terraform_state_content(json, Path::new("terraform.tfstate")).expect("parse");
        let resources = map.get("aws_s3_bucket", "no_arn").expect("no_arn");
        assert_eq!(resources.len(), 1);
        let r = resources.iter().next().unwrap();
        assert!(r.arn.is_none());
        assert!(r.arn_location.is_none());
    }

    // -----------------------------------------------------------------------
    // ARN location tracking tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_arn_location_computed_for_state_resources() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate"))
            .expect("parse");

        // S3 bucket ARN: "arn:aws:s3:::my-app-data-bucket" on line 13 of sample JSON
        // Line 13: `            "arn": "arn:aws:s3:::my-app-data-bucket",`
        // ast-grep returns the string node including quotes
        // Value node `"arn:aws:s3:::my-app-data-bucket"` starts at col 20 (1-based, at the `"`)
        let s3 = map.get("aws_s3_bucket", "data_bucket").expect("s3").iter().next().unwrap();
        let loc = s3.arn_location.as_ref().expect("S3 ARN should have location");
        assert_eq!(loc.file_path, PathBuf::from("terraform.tfstate"));
        assert_eq!(loc.start_line(), 13);
        assert_eq!(loc.start_col(), 20);
        assert_eq!(loc.end_col(), 53);

        // DynamoDB ARN on line 28
        let ddb = map.get("aws_dynamodb_table", "users_table").expect("ddb").iter().next().unwrap();
        let ddb_loc = ddb.arn_location.as_ref().expect("DDB ARN should have location");
        assert_eq!(ddb_loc.start_line(), 28);
    }

    #[test]
    fn test_arn_location_line_and_col_accuracy() {
        // Minimal JSON where we can count positions exactly
        // line 11: `            "arn": "arn:aws:s3:::test-bucket"`
        //          123456789012345678901234567890
        let json = r#"{
  "version": 4,
  "resources": [
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "b",
      "instances": [
        {
          "attributes": {
            "arn": "arn:aws:s3:::test-bucket"
          }
        }
      ]
    }
  ]
}"#;
        let map = parse_terraform_state_content(json, Path::new("state.tfstate")).expect("parse");
        let s3 = map.get("aws_s3_bucket", "b").expect("b").iter().next().unwrap();
        let loc = s3.arn_location.as_ref().expect("should have location");

        assert_eq!(loc.file_path, PathBuf::from("state.tfstate"));
        assert_eq!(loc.start_line(), 11);
        assert_eq!(loc.start_col(), 20); 
        assert_eq!(loc.end_col(), 46);
    }

    #[test]
    fn test_find_string_location_arn_pattern() {
        let content = r#"{"arn": "arn:aws:s3:::my-bucket"}"#;
        let loc = find_string_location(content, Path::new("s.json"), "arn:aws:s3:::my-bucket")
            .expect("should find");
        assert_eq!(loc.start_line(), 1);
        // `{"arn": "arn:aws:s3:::my-bucket"}` — value starts at col 9 (1-based, at `"`)
        assert_eq!(loc.start_col(), 9);
    }

    #[test]
    fn test_find_string_location_not_found() {
        let content = r#"{"bucket": "my-bucket"}"#;
        assert!(find_string_location(content, Path::new("s.json"), "nonexistent").is_none());
    }

    #[test]
    fn test_find_string_location_multiline() {
        let content = "{\n  \"name\": \"test\",\n  \"arn\": \"arn:aws:s3:::b\"\n}";
        let loc = find_string_location(content, Path::new("s.json"), "arn:aws:s3:::b")
            .expect("should find");
        assert_eq!(loc.start_line(), 3);
    }
}
