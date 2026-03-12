//! Terraform state file (`terraform.tfstate`) parser.
//!
//! Reads the v4 JSON format and extracts deployed AWS resource ARNs.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// A resource instance extracted from `terraform.tfstate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateResource {
    /// Terraform resource type (e.g., `"aws_s3_bucket"`)
    pub resource_type: String,
    /// Local name in Terraform config (e.g., `"data_bucket"`)
    pub name: String,
    /// Full ARN if present in state attributes
    pub arn: Option<String>,
}

/// Indexed map of state resources keyed by `(resource_type, local_name)`.
/// Multiple instances (from `count` / `for_each`) share the same key.
pub type StateResourceMap = HashMap<(String, String), Vec<StateResource>>;

/// Parse a `terraform.tfstate` file and extract AWS resource instances.
pub fn parse_terraform_state(path: &Path) -> Result<StateResourceMap> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading state file: {}", path.display()))?;

    parse_terraform_state_content(&content)
}

/// Parse tfstate JSON content (useful for testing without files).
fn parse_terraform_state_content(content: &str) -> Result<StateResourceMap> {
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

            resources_map
                .entry(key.clone())
                .or_default()
                .push(StateResource {
                    resource_type: key.0.clone(),
                    name: key.1.clone(),
                    arn,
                });
        }
    }

    Ok(resources_map)
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
        let result = parse_terraform_state_content(json);
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
        let map = parse_terraform_state_content(sample_state_json()).expect("parse");
        assert_eq!(map.len(), 3, "3 managed resources (data source skipped)");
    }

    #[test]
    fn test_parse_state_extracts_arns() {
        let map = parse_terraform_state_content(sample_state_json()).expect("parse");

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
        let map = parse_terraform_state_content(sample_state_json()).expect("parse");
        assert!(
            !map.contains_key(&("aws_caller_identity".into(), "current".into())),
            "Data sources should be skipped"
        );
    }

    #[test]
    fn test_resources_keyed_by_type_and_name() {
        let map = parse_terraform_state_content(sample_state_json()).expect("parse");
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
        let map = parse_terraform_state_content(json).expect("parse");
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
        let map = parse_terraform_state_content(json).expect("parse");
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
        let map = parse_terraform_state_content(json).expect("parse");
        let resources = &map[&("aws_s3_bucket".into(), "no_arn".into())];
        assert_eq!(resources.len(), 1);
        assert!(resources[0].arn.is_none());
    }
}
