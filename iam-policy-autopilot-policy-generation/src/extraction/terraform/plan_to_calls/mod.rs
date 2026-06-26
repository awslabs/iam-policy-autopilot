//! Terraform plan → SDK method calls.
//!
//! This module turns a Terraform plan (the JSON produced by
//! `terraform show -json <plan>`) into the `SdkMethodCall`s the existing
//! enrichment + policy-generation pipeline already consumes. It is **not** a
//! parallel policy path: it produces the same `SdkMethodCall` currency the
//! source-code extractors emit, and hands them off unchanged.
//!
//! The mapping is driven by two committed, embedded artifacts (under
//! `resources/config/terraform/`):
//!
//! - **`terraform-crud-map.json`** — `resource_type` → its four CRUD handler
//!   symbols (`create`/`read`/`update`/`delete`, full Go import paths; SDKv2
//!   handler funcs or Plugin Framework `(*Resource).Create` methods).
//! - **`terraform-model.json`** — an [`ExternalLibraryModel`] whose
//!   `call_patterns` map a handler `(module_path, class_name, function_name)`
//!   to the AWS SDK operations it invokes.
//!
//! Pipeline:
//! ```text
//! plan resource_change.type ─► crud-map entry
//!   ─► per applicable CRUD slot: handler symbol ─► (module_path, class?, func)
//!   ─► model call_pattern ─► sdk_operations
//!   ─► SdkMethodCall { name: operation, possible_services: [service] }
//! ```

use std::path::Path;

use anyhow::Result;
use rust_embed::RustEmbed;

pub(crate) mod crud_map;
pub(crate) mod go_symbol;
pub(crate) mod mapper;
pub(crate) mod model_index;
pub(crate) mod plan_reader;

pub(crate) use mapper::MappedPlan;
pub(crate) use plan_reader::PlannedResource;

/// Derive SDK method calls from a `terraform show -json` plan file.
///
/// Loads the embedded CRUD map and model, reads the plan, and maps each
/// managed resource's exercised CRUD slots to the SDK operations its handlers
/// invoke. The returned [`MappedPlan`] carries the calls plus any non-fatal
/// warnings (unmodelable resource types, handlers absent from the model).
///
/// The returned [`PlannedResource`] list is also provided so callers can use
/// the `name_prefix` signal for ARN scoping (§5.1).
pub(crate) fn plan_to_sdk_calls(plan_path: &Path) -> Result<(MappedPlan, Vec<PlannedResource>)> {
    let crud_map = crud_map::CrudMap::load()?;
    let model = model_index::ModelIndex::load()?;
    let resources = plan_reader::read_plan(plan_path)?;
    let mapped = mapper::map_plan(&resources, &crud_map, &model);
    Ok((mapped, resources))
}

/// The terraform-provider-aws version tag the embedded model was built against
/// (e.g. `v6.34.0`), for surfacing in `--version --verbose`.
pub(crate) fn model_version() -> Result<Option<String>> {
    Ok(model_index::ModelIndex::load()?
        .version()
        .map(str::to_string))
}

/// The library name recorded in the embedded `terraform-model.json`.
pub(crate) const TERRAFORM_LIBRARY_NAME: &str = "terraform-provider-aws";

/// Embedded Terraform model artifacts.
///
/// Both files are committed under `resources/config/terraform/` and regenerated
/// weekly from the terraform-provider-aws submodule (see
/// `.github/workflows/weekly_terraform_model_update.yml`). They are embedded at
/// compile time following the same `rust-embed` pattern used for the external
/// library models and botocore data.
#[derive(RustEmbed)]
#[folder = "resources/config/terraform"]
#[include = "terraform-crud-map.json"]
#[include = "terraform-model.json"]
struct TerraformArtifacts;

impl TerraformArtifacts {
    /// Raw bytes of the committed `terraform-crud-map.json`.
    fn crud_map_bytes() -> std::borrow::Cow<'static, [u8]> {
        Self::get("terraform-crud-map.json")
            .expect("terraform-crud-map.json not embedded")
            .data
    }

    /// Raw bytes of the committed `terraform-model.json`.
    fn model_bytes() -> std::borrow::Cow<'static, [u8]> {
        Self::get("terraform-model.json")
            .expect("terraform-model.json not embedded")
            .data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_artifacts_are_embedded() {
        assert!(!TerraformArtifacts::crud_map_bytes().is_empty());
        assert!(!TerraformArtifacts::model_bytes().is_empty());
    }
}
