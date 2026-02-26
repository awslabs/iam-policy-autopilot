//! Terraform state file (`terraform.tfstate`) parser.
//!
//! Reads the v4 JSON format and extracts deployed AWS resource ARNs and attributes.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A resource instance extracted from `terraform.tfstate`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateResource {
    /// Terraform resource type (e.g., `"aws_s3_bucket"`)
    pub resource_type: String,
    /// Local name in Terraform config (e.g., `"data_bucket"`)
    pub name: String,
    /// Full ARN if present in state attributes
    pub arn: Option<String>,
    /// Resource ID if present
    pub id: Option<String>,
    /// Selected attributes (naming attributes relevant for ARN construction)
    pub attributes: HashMap<String, String>,
}

/// Result of parsing a `terraform.tfstate` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerraformStateResult {
    /// Extracted resource instances
    pub resources: Vec<StateResource>,
    /// Warnings encountered during parsing
    pub warnings: Vec<String>,
}

impl TerraformStateResult {
    /// Create an empty result
    #[must_use]
    pub fn empty() -> Self {
        Self {
            resources: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Parse a `terraform.tfstate` file and extract AWS resource instances.
pub fn parse_terraform_state(path: &Path) -> Result<TerraformStateResult> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading state file: {}", path.display()))?;

    parse_terraform_state_content(&content)
}

/// Parse tfstate JSON content (useful for testing without files).
pub fn parse_terraform_state_content(content: &str) -> Result<TerraformStateResult> {
    let state: RawState =
        serde_json::from_str(content).context("parsing terraform.tfstate JSON")?;

    match state.version {
        Some(v) if v >= 4 => {} // OK
        Some(v) => anyhow::bail!(
            "unsupported terraform state version {v}: only version 4 or later is supported"
        ),
        None => anyhow::bail!("terraform state file is missing the 'version' field"),
    }

    let mut result = TerraformStateResult::empty();

    let resources = state.resources.unwrap_or_default();
    for raw_resource in &resources {
        // Only process AWS resources
        if !raw_resource.resource_type.starts_with("aws_") {
            continue;
        }

        // Skip data sources in state (mode == "data")
        if raw_resource.mode.as_deref() == Some("data") {
            continue;
        }

        for instance in &raw_resource.instances {
            let attrs = &instance.attributes;

            let arn = attrs
                .get("arn")
                .and_then(|v| v.as_str())
                .map(String::from);

            let id = attrs
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Keep all string-valued attributes
            let selected_attrs: HashMap<String, String> = attrs
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();

            result.resources.push(StateResource {
                resource_type: raw_resource.resource_type.clone(),
                name: raw_resource.name.clone(),
                arn,
                id,
                attributes: selected_attrs,
            });
        }
    }

    Ok(result)
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

    #[test]
    fn test_parse_state_extracts_resources() {
        let result = parse_terraform_state_content(sample_state_json()).expect("parse");

        // Should have 3 managed resources (data source skipped)
        assert_eq!(result.resources.len(), 3);
    }

    #[test]
    fn test_parse_state_extracts_arns() {
        let result = parse_terraform_state_content(sample_state_json()).expect("parse");

        let s3 = result
            .resources
            .iter()
            .find(|r| r.resource_type == "aws_s3_bucket")
            .expect("s3");
        assert_eq!(s3.arn.as_deref(), Some("arn:aws:s3:::my-app-data-bucket"));
        assert_eq!(s3.name, "data_bucket");

        let ddb = result
            .resources
            .iter()
            .find(|r| r.resource_type == "aws_dynamodb_table")
            .expect("ddb");
        assert_eq!(
            ddb.arn.as_deref(),
            Some("arn:aws:dynamodb:us-east-1:123456789012:table/users-table")
        );
    }

    #[test]
    fn test_parse_state_extracts_all_string_attributes() {
        let result = parse_terraform_state_content(sample_state_json()).expect("parse");

        let s3 = result
            .resources
            .iter()
            .find(|r| r.resource_type == "aws_s3_bucket")
            .expect("s3");
        assert_eq!(s3.attributes.get("bucket").map(String::as_str), Some("my-app-data-bucket"));
        assert_eq!(s3.attributes.get("id").map(String::as_str), Some("my-app-data-bucket"));
    }

    #[test]
    fn test_parse_state_excludes_non_string_attributes() {
        let json = r#"{
  "version": 4,
  "resources": [
    {
      "mode": "managed",
      "type": "aws_s3_bucket",
      "name": "test",
      "instances": [
        {
          "attributes": {
            "bucket": "my-bucket",
            "tags": {"env": "prod"},
            "versioning": [{"enabled": true}],
            "force_destroy": false,
            "count": 42
          }
        }
      ]
    }
  ]
}"#;
        let result = parse_terraform_state_content(json).expect("parse");
        let r = &result.resources[0];
        assert_eq!(r.attributes.get("bucket").map(String::as_str), Some("my-bucket"));
        assert!(!r.attributes.contains_key("tags"), "objects should be excluded");
        assert!(!r.attributes.contains_key("versioning"), "arrays should be excluded");
        assert!(!r.attributes.contains_key("force_destroy"), "bools should be excluded");
        assert!(!r.attributes.contains_key("count"), "numbers should be excluded");
    }

    #[test]
    fn test_parse_state_skips_data_sources() {
        let result = parse_terraform_state_content(sample_state_json()).expect("parse");

        let data_sources: Vec<_> = result
            .resources
            .iter()
            .filter(|r| r.resource_type == "aws_caller_identity")
            .collect();
        assert!(data_sources.is_empty(), "Data sources should be skipped");
    }

    #[test]
    fn test_parse_state_malformed_json() {
        let result = parse_terraform_state_content("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_state_empty_resources() {
        let json = r#"{"version": 4, "resources": []}"#;
        let result = parse_terraform_state_content(json).expect("parse");
        assert!(result.resources.is_empty());
    }

    #[test]
    fn test_parse_state_no_resources_key() {
        let json = r#"{"version": 4}"#;
        let result = parse_terraform_state_content(json).expect("parse");
        assert!(result.resources.is_empty());
    }

    #[test]
    fn test_state_result_roundtrip() {
        let result = parse_terraform_state_content(sample_state_json()).expect("parse");
        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: TerraformStateResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, deserialized);
    }

    #[test]
    fn test_rejects_state_version_below_4() {
        let json = r#"{"version": 3, "resources": []}"#;
        let result = parse_terraform_state_content(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported terraform state version 3"), "{err}");
    }

    #[test]
    fn test_rejects_missing_version_field() {
        let json = r#"{"resources": []}"#;
        let result = parse_terraform_state_content(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing the 'version' field"), "{err}");
    }

    #[test]
    fn test_accepts_state_version_4() {
        let json = r#"{"version": 4, "resources": []}"#;
        let result = parse_terraform_state_content(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_accepts_future_state_version() {
        let json = r#"{"version": 5, "resources": []}"#;
        let result = parse_terraform_state_content(json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resource_without_arn_uses_id() {
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
        let result = parse_terraform_state_content(json).expect("parse");
        assert_eq!(result.resources.len(), 1);
        assert!(result.resources[0].arn.is_none());
        assert_eq!(result.resources[0].id.as_deref(), Some("my-bucket"));
    }
}
