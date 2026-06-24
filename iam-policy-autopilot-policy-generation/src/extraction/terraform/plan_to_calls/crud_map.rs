//! The Terraform CRUD map: `resource_type` → its four CRUD handler symbols.
//!
//! Parses the committed `terraform-crud-map.json` into a lookup keyed by
//! resource type. Each entry carries the four `*_without_timeout` handler
//! symbols (full Go import paths, e.g.
//! `github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketCreate`).
//! A slot is `None` when the provider has no handler for it — most commonly
//! `update` on immutable resources (~219 of ~1240 resource types).

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::TerraformArtifacts;

/// One of the four CRUD lifecycle slots a Terraform resource handler covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum CrudSlot {
    Create,
    Read,
    Update,
    Delete,
}

/// A single resource type's CRUD handler symbols, as committed in
/// `terraform-crud-map.json`. Field names match the JSON keys exactly.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ResourceEntry {
    pub(crate) resource_type: String,
    #[serde(default)]
    pub(crate) create_without_timeout: Option<String>,
    #[serde(default)]
    pub(crate) read_without_timeout: Option<String>,
    #[serde(default)]
    pub(crate) update_without_timeout: Option<String>,
    #[serde(default)]
    pub(crate) delete_without_timeout: Option<String>,
}

impl ResourceEntry {
    /// The handler symbol for a given CRUD slot, if the provider defines one.
    pub(crate) fn handler(&self, slot: CrudSlot) -> Option<&str> {
        match slot {
            CrudSlot::Create => self.create_without_timeout.as_deref(),
            CrudSlot::Read => self.read_without_timeout.as_deref(),
            CrudSlot::Update => self.update_without_timeout.as_deref(),
            CrudSlot::Delete => self.delete_without_timeout.as_deref(),
        }
    }
}

/// Resource-type → CRUD handler symbols, loaded from the embedded JSON.
pub(crate) struct CrudMap {
    by_type: HashMap<String, ResourceEntry>,
}

impl CrudMap {
    /// Parse the embedded `terraform-crud-map.json`.
    pub(crate) fn load() -> Result<Self> {
        let bytes = TerraformArtifacts::crud_map_bytes();
        Self::from_slice(&bytes)
    }

    /// Parse a CRUD map from raw JSON bytes (a list of [`ResourceEntry`]).
    fn from_slice(bytes: &[u8]) -> Result<Self> {
        let entries: Vec<ResourceEntry> =
            serde_json::from_slice(bytes).context("Failed to parse terraform-crud-map.json")?;
        let by_type = entries
            .into_iter()
            .map(|e| (e.resource_type.clone(), e))
            .collect();
        Ok(Self { by_type })
    }

    /// Look up a resource type's CRUD entry, e.g. `"aws_s3_bucket"`.
    pub(crate) fn get(&self, resource_type: &str) -> Option<&ResourceEntry> {
        self.by_type.get(resource_type)
    }

    /// Build a CRUD map from raw JSON bytes, for cross-module tests.
    #[cfg(test)]
    pub(crate) fn from_slice_for_test(bytes: &[u8]) -> Self {
        Self::from_slice(bytes).expect("valid test CRUD map JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const SAMPLE: &str = r#"[
        {
            "resource_type": "aws_s3_bucket",
            "create_without_timeout": "pkg/internal/service/s3.resourceBucketCreate",
            "read_without_timeout": "pkg/internal/service/s3.resourceBucketRead",
            "update_without_timeout": "pkg/internal/service/s3.resourceBucketUpdate",
            "delete_without_timeout": "pkg/internal/service/s3.resourceBucketDelete"
        },
        {
            "resource_type": "aws_immutable_thing",
            "create_without_timeout": "pkg/internal/service/x.resourceThingCreate",
            "read_without_timeout": "pkg/internal/service/x.resourceThingRead",
            "delete_without_timeout": "pkg/internal/service/x.resourceThingDelete"
        }
    ]"#;

    #[rstest]
    #[case(CrudSlot::Create, Some("pkg/internal/service/s3.resourceBucketCreate"))]
    #[case(CrudSlot::Read, Some("pkg/internal/service/s3.resourceBucketRead"))]
    #[case(CrudSlot::Update, Some("pkg/internal/service/s3.resourceBucketUpdate"))]
    #[case(CrudSlot::Delete, Some("pkg/internal/service/s3.resourceBucketDelete"))]
    fn handler_returns_symbol_for_each_slot(
        #[case] slot: CrudSlot,
        #[case] expected: Option<&str>,
    ) {
        let map = CrudMap::from_slice(SAMPLE.as_bytes()).unwrap();
        let entry = map.get("aws_s3_bucket").unwrap();
        assert_eq!(entry.handler(slot), expected);
    }

    #[test]
    fn missing_update_slot_is_none() {
        let map = CrudMap::from_slice(SAMPLE.as_bytes()).unwrap();
        let entry = map.get("aws_immutable_thing").unwrap();
        assert_eq!(entry.handler(CrudSlot::Update), None);
        assert_eq!(
            entry.handler(CrudSlot::Create),
            Some("pkg/internal/service/x.resourceThingCreate")
        );
    }

    #[test]
    fn unknown_resource_type_is_none() {
        let map = CrudMap::from_slice(SAMPLE.as_bytes()).unwrap();
        assert_eq!(map.get("aws_does_not_exist"), None);
    }

    #[test]
    fn embedded_crud_map_parses_and_contains_known_resource() {
        let map = CrudMap::load().unwrap();
        let entry = map.get("aws_s3_bucket").unwrap();
        assert_eq!(entry.resource_type, "aws_s3_bucket");
    }
}
