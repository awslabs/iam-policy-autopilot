//! Terraform enrichment module for resolving Terraform resources to IAM metadata
//! and binding concrete ARNs into enriched SDK calls.
//!
//! This module provides the enrichment-phase functionality for Terraform integration:
//! - Map Terraform resource types to IAM service names via `names_data.hcl`
//! - Resolve resources to concrete ARNs (from HCL attributes and/or terraform.tfstate)
//! - Substitute ARN placeholders in enriched SDK calls with concrete values
//! - Generate binding explanations for policy output

use serde::{Deserialize, Serialize};

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
    /// Terraform resource type (e.g., `"aws_s3_bucket"`)
    pub terraform_resource_type: String,
    /// Terraform local name (e.g., `"data_bucket"`)
    pub terraform_resource_name: String,
    /// Location in GNU format: `file:line` or just `file` if line unknown
    pub location: String,
}

/// Whether the ARN came from HCL parsing or from a terraform.tfstate file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BindingSource {
    /// ARN constructed from HCL resource attributes
    #[serde(rename = "HCL")]
    Hcl,
    /// ARN read directly from terraform.tfstate
    #[serde(rename = "TerraformState")]
    TerraformState,
}
