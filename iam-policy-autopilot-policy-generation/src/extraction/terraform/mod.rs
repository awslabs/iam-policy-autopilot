//! Terraform extraction module for parsing HCL files, resolving variables,
//! and parsing state files.
//!
//! This module provides the extraction-phase functionality for Terraform integration:
//! - Parse Terraform HCL configuration files to extract AWS resource definitions
//! - Resolve `var.xxx` references from defaults and `.tfvars` files
//! - Parse `terraform.tfstate` files for deployed resource ARNs

use std::collections::HashMap;

use crate::Location;

pub mod hcl_parser;
pub mod state_parser;

/// Terraform AWS provider resource type prefix. Used to filter resources
/// from multi-provider configurations — only types starting with this
/// prefix are processed for IAM ARN binding.
pub const AWS_RESOURCE_PREFIX: &str = "aws_";
pub mod variable_resolver;

/// Represents a Terraform attribute value, which may be a literal or an unresolvable expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttributeValue {
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
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Literal(s) | Self::Expression(s) => s,
        }
    }

    /// Returns `true` if this is a `Literal` value.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }
}

/// A parsed Terraform resource block (e.g., `resource "aws_s3_bucket" "my_bucket" { ... }`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerraformResource {
    /// Terraform resource type (e.g., `"aws_s3_bucket"`, `"aws_dynamodb_table"`)
    pub resource_type: String,
    /// Local name in Terraform config (e.g., `"my_bucket"`)
    pub local_name: String,
    /// Key attributes relevant for ARN construction
    /// (e.g., `{"bucket": Literal("my-app-data")}`)
    pub attributes: HashMap<String, AttributeValue>,
    /// Location of the resource block definition in the source `.tf` file
    pub location: Location,
}

/// A collection of parsed Terraform resource blocks with associated warnings.
///
/// Resources are keyed internally by `(resource_type, local_name)` for O(1) lookup.
/// Use the provided methods to query and mutate instead of accessing the map directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerraformResources {
    /// Discovered AWS resource blocks keyed by `(resource_type, local_name)`
    resources: HashMap<(String, String), TerraformResource>,
    /// Warnings encountered during parsing (e.g., syntax errors in individual files)
    warnings: Vec<String>,
}

impl TerraformResources {
    /// Create an empty collection.
    #[must_use]
    pub(crate) fn default() -> Self {
        Self {
            resources: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Insert a resource, keyed by `(resource_type, local_name)`.
    ///
    /// If a resource with the same key already exists, it is replaced.
    pub(crate) fn insert(&mut self, resource: TerraformResource) {
        let key = (resource.resource_type.clone(), resource.local_name.clone());
        self.resources.insert(key, resource);
    }

    /// Look up a resource by type and local name.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn get(&self, resource_type: &str, local_name: &str) -> Option<&TerraformResource> {
        self.resources
            .get(&(resource_type.to_string(), local_name.to_string()))
    }

    /// Number of resources.
    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Returns `true` if there are no resources.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Iterate over all resources (unordered).
    pub(crate) fn values(&self) -> impl Iterator<Item = &TerraformResource> {
        self.resources.values()
    }

    /// Iterate mutably over all resources (unordered).
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut TerraformResource> {
        self.resources.values_mut()
    }

    /// Access the warnings collected during parsing.
    #[must_use]
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Record a warning.
    pub(crate) fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    /// Take ownership of the warnings, leaving an empty list behind.
    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Merge another collection into this one.
    ///
    /// Resources and warnings from `other` are appended. If both collections
    /// contain a resource with the same `(type, local_name)` key, the one
    /// from `other` wins (last-write-wins).
    pub(crate) fn merge(&mut self, other: Self) {
        self.resources.extend(other.resources);
        self.warnings.extend(other.warnings);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
            location: Location::new(PathBuf::from("main.tf"), (1, 1), (1, 1)),
        };
        assert_eq!(resource.resource_type, "aws_s3_bucket");
        assert_eq!(resource.local_name, "data_bucket");
        assert_eq!(
            resource.attributes.get("bucket"),
            Some(&AttributeValue::Literal("my-app-data".to_string()))
        );
    }

    #[test]
    fn test_parsed_terraform_resources_empty() {
        let result = TerraformResources::default();
        assert!(result.is_empty());
        assert!(result.warnings().is_empty());
    }

    #[test]
    fn test_parsed_terraform_resources_insert_and_get() {
        let mut result = TerraformResources::default();
        let resource = TerraformResource {
            resource_type: "aws_s3_bucket".to_string(),
            local_name: "b".to_string(),
            attributes: HashMap::new(),
            location: Location::new(PathBuf::from("main.tf"), (1, 1), (1, 1)),
        };
        result.insert(resource);
        assert_eq!(result.len(), 1);
        assert!(!result.is_empty());
        assert!(result.get("aws_s3_bucket", "b").is_some());
        assert!(result.get("aws_s3_bucket", "other").is_none());
    }
}
