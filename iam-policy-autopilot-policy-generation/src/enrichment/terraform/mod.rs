//! Terraform enrichment module for resolving Terraform resources to IAM metadata
//! and binding concrete ARNs into enriched SDK calls.
//!
//! This module provides the enrichment-phase functionality for Terraform integration:
//! - Map Terraform resource types to IAM service names via `names_data.hcl`
//! - Resolve resources to concrete ARNs (from Terraform attributes and/or terraform.tfstate)
//! - Substitute ARN placeholders in enriched SDK calls with concrete values
//! - Generate binding explanations for policy output

use serde::{Deserialize, Serialize};

use crate::Location;

pub mod resource_binder;
mod service_resolver;

// ---------------------------------------------------------------------------
// Resource Binding Explanation
// ---------------------------------------------------------------------------

/// Describes where a concrete resource ARN in the generated policy came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct ResourceBindingExplanation {
    /// The concrete ARN that was substituted into the policy
    pub arn: String,
    /// Source of the ARN binding
    pub source: BindingSource,
    /// Resource type (e.g., `"aws_s3_bucket"`)
    pub resource_type: String,
    /// Resource local name (e.g., `"data_bucket"`)
    pub resource_name: String,
    /// Location of the resource definition
    pub location: Location,
}

/// Whether the ARN came from Terraform configuration parsing or from a terraform.tfstate file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum BindingSource {
    /// ARN constructed from Terraform resource attributes
    Terraform,
    /// ARN read directly from terraform.tfstate
    TerraformState,
}
