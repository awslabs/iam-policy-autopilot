//! Terraform extraction module for parsing HCL files, resolving variables,
//! and parsing state files.
//!
//! This module provides the extraction-phase functionality for Terraform integration:
//! - Parse Terraform HCL configuration files to extract AWS resource definitions
//! - Resolve `var.xxx` references from defaults and `.tfvars` files
//! - Parse `terraform.tfstate` files for deployed resource ARNs

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod hcl_parser;
pub mod state_parser;

/// Terraform AWS provider resource type prefix. Used to filter resources
/// from multi-provider configurations — only types starting with this
/// prefix are processed for IAM ARN binding.
pub const AWS_RESOURCE_PREFIX: &str = "aws_";
pub mod variable_resolver;

/// Represents a Terraform attribute value, which may be a literal or an unresolvable expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AttributeValue {
    /// A resolved literal string value (e.g., `"my-app-bucket"`)
    Literal(String),
    /// An unresolvable expression preserved as-is (e.g., `"${var.prefix}-bucket"`)
    Expression(String),
}

impl AttributeValue {
    /// Returns the string value regardless of variant.
    /// For `Literal`, returns the literal value.
    /// For `Expression`, returns the raw expression text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Literal(s) | Self::Expression(s) => s,
        }
    }

    /// Returns `true` if this is a `Literal` value.
    #[must_use]
    pub fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }
}

/// A parsed Terraform resource block (e.g., `resource "aws_s3_bucket" "my_bucket" { ... }`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerraformResource {
    /// Terraform resource type (e.g., `"aws_s3_bucket"`, `"aws_dynamodb_table"`)
    pub resource_type: String,
    /// Local name in Terraform config (e.g., `"my_bucket"`)
    pub local_name: String,
    /// Key attributes relevant for ARN construction
    /// (e.g., `{"bucket": Literal("my-app-data")}`)
    pub attributes: HashMap<String, AttributeValue>,
    /// Source `.tf` file path where this resource is defined
    pub source_file: PathBuf,
    /// Line number in the source file where the resource block starts (1-based)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_number: Option<usize>,
}

/// Key for resource maps: `(resource_type, local_name)`.
pub type TerraformBlockKey = (String, String);

/// Map of Terraform resource blocks keyed by `(resource_type, local_name)`.
pub type TerraformResourceMap = HashMap<TerraformBlockKey, TerraformResource>;

/// Result of parsing Terraform configuration files in a directory.
///
/// Resources are keyed by `(type, local_name)` for O(1) lookup.
/// Custom serde is used because tuple-keyed HashMaps don't serialize to JSON natively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerraformParseResult {
    /// Discovered AWS resource blocks keyed by `(resource_type, local_name)`
    pub resources: TerraformResourceMap,
    /// Warnings encountered during parsing (e.g., syntax errors in individual files)
    pub warnings: Vec<String>,
}

impl TerraformParseResult {
    /// Create an empty parse result
    #[must_use]
    pub fn empty() -> Self {
        Self {
            resources: TerraformResourceMap::new(),
            warnings: Vec::new(),
        }
    }
}

// Custom serde: serialize resources as Vec for JSON compatibility.
impl Serialize for TerraformParseResult {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TerraformParseResult", 2)?;
        let resources: Vec<&TerraformResource> = self.resources.values().collect();
        state.serialize_field("resources", &resources)?;
        state.serialize_field("warnings", &self.warnings)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TerraformParseResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            resources: Vec<TerraformResource>,
            warnings: Vec<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let mut result = Self::empty();
        result.warnings = raw.warnings;
        for r in raw.resources {
            let key = (r.resource_type.clone(), r.local_name.clone());
            result.resources.insert(key, r);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attribute_value_literal() {
        let val = AttributeValue::Literal("my-bucket".to_string());
        assert_eq!(val.as_str(), "my-bucket");
        assert!(val.is_literal());
    }

    #[test]
    fn test_attribute_value_expression() {
        let val = AttributeValue::Expression("${var.prefix}-bucket".to_string());
        assert_eq!(val.as_str(), "${var.prefix}-bucket");
        assert!(!val.is_literal());
    }

    #[test]
    fn test_terraform_resource_creation() {
        let resource = TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "data_bucket".to_string(),
            attributes: HashMap::from([(
                "bucket".to_string(),
                AttributeValue::Literal("my-app-data".to_string()),
            )]),
            source_file: PathBuf::from("main.tf"),
            line_number: None,
        };
        assert_eq!(resource.resource_type, "aws_s3_bucket");
        assert_eq!(resource.local_name, "data_bucket");
        assert_eq!(
            resource.attributes.get("bucket"),
            Some(&AttributeValue::Literal("my-app-data".to_string()))
        );
    }

    #[test]
    fn test_terraform_parse_result_empty() {
        let result = TerraformParseResult::empty();
        assert!(result.resources.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let resource = TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "test".to_string(),
            attributes: HashMap::from([
                (
                    "bucket".to_string(),
                    AttributeValue::Literal("my-bucket".to_string()),
                ),
                (
                    "tags".to_string(),
                    AttributeValue::Expression("var.tags".to_string()),
                ),
            ]),
            source_file: PathBuf::from("main.tf"),
            line_number: None,
        };

        let json = serde_json::to_string(&resource).unwrap();
        let deserialized: TerraformResource = serde_json::from_str(&json).unwrap();
        assert_eq!(resource, deserialized);
    }

    #[test]
    fn test_parse_result_serialize_deserialize_roundtrip() {
        let mut result = TerraformParseResult::empty();
        let resource = TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "bucket".to_string(),
            attributes: HashMap::from([(
                "bucket".to_string(),
                AttributeValue::Literal("test-bucket".to_string()),
            )]),
            source_file: PathBuf::from("main.tf"),
            line_number: None,
        };
        result.resources.insert(
            (resource.resource_type.clone(), resource.local_name.clone()),
            resource,
        );
        result.warnings.push("some warning".to_string());

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: TerraformParseResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deserialized);
    }
}
