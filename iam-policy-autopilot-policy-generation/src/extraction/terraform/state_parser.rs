//! Terraform state file (`terraform.tfstate`) parser.
//!
//! Reads the v4 JSON format and extracts deployed AWS resource ARNs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::Location;

/// A resource instance extracted from `terraform.tfstate`.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Indexed map of state resources keyed by `(resource_type, local_name)`.
/// Multiple instances (from `count` / `for_each`) share the same key.
pub type StateResourceMap = HashMap<(String, String), Vec<StateResource>>;

/// Parse a `terraform.tfstate` file and extract AWS resource instances.
pub fn parse_terraform_state(path: &Path) -> Result<StateResourceMap> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading state file: {}", path.display()))?;

    parse_terraform_state_content(&content, path)
}

/// Parse tfstate JSON content (useful for testing without files).
fn parse_terraform_state_content(content: &str, source_path: &Path) -> Result<StateResourceMap> {
    let state: RawState =
        serde_json::from_str(content).context("parsing terraform.tfstate JSON")?;

    match state.version {
        Some(v) if v >= 4 => {} // OK
        Some(v) => anyhow::bail!(
            "unsupported terraform state version {v}: only version 4 or later is supported"
        ),
        None => anyhow::bail!("terraform state file is missing the 'version' field"),
    }

    let mut resources_map = StateResourceMap::new();

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

        let key = (
            raw_resource.resource_type.clone(),
            raw_resource.name.clone(),
        );

        for instance in &raw_resource.instances {
            let arn = instance
                .attributes
                .get("arn")
                .and_then(|v| v.as_str())
                .map(String::from);

            let arn_location = arn
                .as_deref()
                .and_then(|arn_val| find_string_location(content, source_path, arn_val));

            resources_map
                .entry(key.clone())
                .or_default()
                .push(StateResource {
                    resource_type: key.0.clone(),
                    name: key.1.clone(),
                    arn,
                    arn_location,
                });
        }
    }

    Ok(resources_map)
}

/// Find the location (line, column) of a string value in JSON content.
///
/// Searches for the pattern `"arn": "<value>"` in the raw content and returns
/// a `Location` pointing at the value (including surrounding quotes).
/// Falls back to searching for the bare value string if the key pattern isn't found.
fn find_string_location(content: &str, source_path: &Path, value: &str) -> Option<Location> {
    // Search for "arn": "value" pattern first (more precise)
    let arn_pattern = format!(r#""arn": "{value}""#);
    let search_str = content.find(&arn_pattern).or_else(|| {
        // Try without space after colon
        let alt_pattern = format!(r#""arn":"{value}""#);
        content.find(&alt_pattern)
    });

    if let Some(byte_offset) = search_str {
        // Find the start of the value (the quote before the ARN)
        let value_offset = content[byte_offset..]
            .find(value)
            .map(|off| byte_offset + off)?;
        return Some(byte_offset_to_location(content, source_path, value_offset, value.len()));
    }

    // Fallback: search for the bare quoted value
    let quoted = format!(r#""{value}""#);
    let byte_offset = content.find(&quoted)?;
    Some(byte_offset_to_location(content, source_path, byte_offset, quoted.len()))
}

/// Convert a byte offset in content to a `Location` with line/column numbers.
fn byte_offset_to_location(
    content: &str,
    source_path: &Path,
    offset: usize,
    length: usize,
) -> Location {
    let before = &content[..offset];
    let line = before.matches('\n').count() + 1;
    let col = offset - before.rfind('\n').map_or(0, |pos| pos + 1) + 1;
    let end_col = col + length;
    Location::new(
        source_path.to_path_buf(),
        (line, col),
        (line, end_col),
    )
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

        let s3 = &map[&("aws_s3_bucket".into(), "data_bucket".into())][0];
        assert_eq!(s3.arn.as_deref(), Some("arn:aws:s3:::my-app-data-bucket"));
        assert_eq!(s3.name, "data_bucket");

        let ddb = &map[&("aws_dynamodb_table".into(), "users_table".into())][0];
        assert_eq!(
            ddb.arn.as_deref(),
            Some("arn:aws:dynamodb:us-east-1:123456789012:table/users-table")
        );
    }

    #[test]
    fn test_parse_state_skips_data_sources() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");
        assert!(
            !map.contains_key(&("aws_caller_identity".into(), "current".into())),
            "Data sources should be skipped"
        );
    }

    #[test]
    fn test_resources_keyed_by_type_and_name() {
        let map = parse_terraform_state_content(sample_state_json(), Path::new("terraform.tfstate")).expect("parse");
        assert!(map.contains_key(&("aws_s3_bucket".into(), "data_bucket".into())));
        assert!(map.contains_key(&("aws_dynamodb_table".into(), "users_table".into())));
        assert!(map.contains_key(&("aws_sqs_queue".into(), "task_queue".into())));
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
        let buckets = &map[&("aws_s3_bucket".into(), "data".into())];
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].arn.as_deref(), Some("arn:aws:s3:::bucket-0"));
        assert_eq!(buckets[1].arn.as_deref(), Some("arn:aws:s3:::bucket-1"));
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
        assert!(map.contains_key(&("aws_s3_bucket".into(), "s3".into())));
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
        let resources = &map[&("aws_s3_bucket".into(), "no_arn".into())];
        assert_eq!(resources.len(), 1);
        assert!(resources[0].arn.is_none());
        assert!(resources[0].arn_location.is_none());
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
        // Value starts at col 21 (after 12 spaces + `"arn": "`), length 31
        let s3 = &map[&("aws_s3_bucket".into(), "data_bucket".into())][0];
        let loc = s3.arn_location.as_ref().expect("S3 ARN should have location");
        assert_eq!(loc.file_path, PathBuf::from("terraform.tfstate"));
        assert_eq!(loc.start_line(), 13);
        assert_eq!(loc.start_col(), 21);
        assert_eq!(loc.end_col(), 52); // 21 + 31

        // DynamoDB ARN on line 28
        let ddb = &map[&("aws_dynamodb_table".into(), "users_table".into())][0];
        let ddb_loc = ddb.arn_location.as_ref().expect("DDB ARN should have location");
        assert_eq!(ddb_loc.start_line(), 28);
    }

    #[test]
    fn test_arn_location_line_and_col_accuracy() {
        // Minimal JSON where we can count positions exactly
        //                                                  col positions:
        // line 11: `            "arn": "arn:aws:s3:::test-bucket"`
        //          123456789012345678901234567890
        //                                ^col21  = start of value
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
        let s3 = &map[&("aws_s3_bucket".into(), "b".into())][0];
        let loc = s3.arn_location.as_ref().expect("should have location");

        assert_eq!(loc.file_path, PathBuf::from("state.tfstate"));
        assert_eq!(loc.start_line(), 11);
        assert_eq!(loc.start_col(), 21); // after `            "arn": "`
        // "arn:aws:s3:::test-bucket" = 24 chars
        assert_eq!(loc.end_col(), 45); // 21 + 24
    }

    #[test]
    fn test_byte_offset_to_location_basic() {
        let content = "line1\nline2\nline3";
        // "line3" starts at byte offset 12
        let loc = byte_offset_to_location(content, Path::new("f.json"), 12, 5);
        assert_eq!(loc.start_line(), 3);
        assert_eq!(loc.start_col(), 1);
        assert_eq!(loc.end_col(), 6); // 1 + 5
    }

    #[test]
    fn test_byte_offset_to_location_mid_line() {
        let content = "{\n  \"key\": \"value\"\n}";
        // "value" starts at byte offset 11
        let loc = byte_offset_to_location(content, Path::new("f.json"), 11, 5);
        assert_eq!(loc.start_line(), 2);
        assert_eq!(loc.start_col(), 10); // after `  "key": "`
        assert_eq!(loc.end_col(), 15);
    }

    #[test]
    fn test_find_string_location_arn_pattern() {
        let content = r#"{"arn": "arn:aws:s3:::my-bucket"}"#;
        let loc = find_string_location(content, Path::new("s.json"), "arn:aws:s3:::my-bucket")
            .expect("should find");
        assert_eq!(loc.start_line(), 1);
        // "arn:aws:s3:::my-bucket" starts at byte offset 9, col = 10 (1-based)
        assert_eq!(loc.start_col(), 10);
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
