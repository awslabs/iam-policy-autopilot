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

use std::path::PathBuf;

use anyhow::Result;
use rust_embed::RustEmbed;

pub(crate) mod crud_map;
pub(crate) mod go_symbol;
pub(crate) mod mapper;
pub(crate) mod model_index;
pub(crate) mod plan_reader;

pub(crate) use mapper::MappedPlan;
pub(crate) use plan_reader::{file_looks_like_plan, PlannedResource};

/// Derive SDK method calls from one or more `terraform show -json` plan files.
///
/// Loads the embedded CRUD map and model once, reads every plan, **unions**
/// their resource changes, and maps each managed resource's exercised CRUD
/// slots to the SDK operations its handlers invoke. Multiple plans are
/// additive: the mapper dedups identical `(service, operation)` calls, so a
/// resource appearing in more than one plan contributes its actions once.
///
/// The returned [`MappedPlan`] carries the calls plus any non-fatal warnings
/// (unmodelable resource types, handlers absent from the model). The combined
/// [`PlannedResource`] list is also provided so callers can use the
/// `name_prefix` signal for ARN scoping (§5.1).
pub(crate) fn plan_to_sdk_calls(
    plan_paths: &[PathBuf],
) -> Result<(MappedPlan, Vec<PlannedResource>)> {
    let crud_map = crud_map::CrudMap::load()?;
    let model = model_index::ModelIndex::load()?;

    let mut resources = Vec::new();
    for plan_path in plan_paths {
        resources.extend(plan_reader::read_plan(plan_path)?);
    }

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

    /// Exercises the consumer (`go_symbol::handler_key`) against the two real
    /// committed artifacts: every key in `terraform-model.json` must be
    /// reproducible by `handler_key` from some `terraform-crud-map.json` handler
    /// symbol.
    ///
    /// The model is built from the CRUD map, so each model key came from some
    /// handler symbol. If `handler_key` cannot reproduce a key, the consumer
    /// parses that symbol differently than the model was keyed on, and the
    /// handler would silently resolve to no actions (under-scoping). A no-op
    /// handler produces no model key, so it cannot cause a spurious miss.
    ///
    /// Scope: this pins the consumer to the shipped model. It does not run the
    /// model builder, so it cannot catch the builder and consumer drifting
    /// together across a regeneration.
    #[test]
    fn every_model_key_is_reproducible_from_a_crud_map_symbol() {
        use std::collections::HashSet;

        let crud = crud_map::CrudMap::load().unwrap();
        let model = model_index::ModelIndex::load().unwrap();

        // Keys the consumer reproduces from every CRUD-map handler symbol.
        let reachable: HashSet<_> = crud
            .entries()
            .flat_map(|entry| entry.handler_symbols())
            .filter_map(go_symbol::handler_key)
            .collect();

        let unreachable: Vec<_> = model.keys().filter(|k| !reachable.contains(k)).collect();

        assert!(
            unreachable.is_empty(),
            "{} model key(s) are not reproducible from any CRUD-map handler symbol — \
             go_symbol::handler_key parses these differently than the model was keyed on, \
             so these handlers would silently resolve to no actions:\n{unreachable:#?}",
            unreachable.len(),
        );
    }
}
