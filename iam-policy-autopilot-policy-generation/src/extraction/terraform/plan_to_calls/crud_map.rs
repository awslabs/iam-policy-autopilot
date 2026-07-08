//! The Terraform CRUD map: `resource_type` → its four CRUD handler symbols.
//!
//! Parses the committed `terraform-crud-map.json` into a lookup keyed by
//! resource type. Each entry carries the four CRUD handler symbols
//! (`create`/`read`/`update`/`delete`, full Go import paths) — for SDKv2
//! resources the `*_without_timeout` handler funcs (e.g.
//! `.../internal/service/s3.resourceBucketCreate`), for Plugin Framework
//! resources the `Create`/`Read`/`Update`/`Delete` methods (e.g.
//! `.../internal/service/appsync.(*apiResource).Create`).
//! A slot is `None` when the provider has no handler for it — most commonly
//! `update` on immutable resources (~219 of ~1600 resource types).

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
    pub(crate) create: Option<String>,
    #[serde(default)]
    pub(crate) read: Option<String>,
    #[serde(default)]
    pub(crate) update: Option<String>,
    #[serde(default)]
    pub(crate) delete: Option<String>,
    /// Transparent-tagging entry points, present only for `@Tags` resources
    /// whose service package implements the tagging interface. References the
    /// model's `ListTags`/`UpdateTags` `call_pattern`s (the tag SDK calls are
    /// invoked by the provider's interceptor, outside the CRUD handlers).
    #[serde(default)]
    pub(crate) tags: Option<TagsInfo>,
}

/// Tagging entry-point symbols for a resource (mirrors the Go extractor's
/// `tags` block). Strictly references — the SDK operations live in the model,
/// keyed by these symbols, never duplicated here.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct TagsInfo {
    /// `(*servicePackage).ListTags` symbol (tag read), if a lister exists.
    #[serde(default)]
    pub(crate) list_tags_symbol: Option<String>,
    /// `(*servicePackage).UpdateTags` symbol (tag write), if an updater exists.
    #[serde(default)]
    pub(crate) update_tags_symbol: Option<String>,
}

impl ResourceEntry {
    /// The handler symbol for a given CRUD slot, if the provider defines one.
    pub(crate) fn handler(&self, slot: CrudSlot) -> Option<&str> {
        match slot {
            CrudSlot::Create => self.create.as_deref(),
            CrudSlot::Read => self.read.as_deref(),
            CrudSlot::Update => self.update.as_deref(),
            CrudSlot::Delete => self.delete.as_deref(),
        }
    }

    /// The `ListTags` (tag-read) symbol, if this resource is tag-managed.
    pub(crate) fn list_tags_symbol(&self) -> Option<&str> {
        self.tags.as_ref()?.list_tags_symbol.as_deref()
    }

    /// The `UpdateTags` (tag-write) symbol, if this resource is tag-managed.
    pub(crate) fn update_tags_symbol(&self) -> Option<&str> {
        self.tags.as_ref()?.update_tags_symbol.as_deref()
    }

    /// Every handler symbol this entry carries: the four CRUD slots plus, for
    /// `@Tags` resources, the `ListTags`/`UpdateTags` methods. Mirrors the model
    /// builder's `ResourceEntry::handler_symbols` (xtask) — the set of symbols
    /// that must each resolve to a model `call_pattern`.
    #[cfg(test)]
    pub(crate) fn handler_symbols(&self) -> impl Iterator<Item = &str> {
        [
            self.create.as_deref(),
            self.read.as_deref(),
            self.update.as_deref(),
            self.delete.as_deref(),
            self.list_tags_symbol(),
            self.update_tags_symbol(),
        ]
        .into_iter()
        .flatten()
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

    /// Every resource entry in the map.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.by_type.values()
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
            "create": "pkg/internal/service/s3.resourceBucketCreate",
            "read": "pkg/internal/service/s3.resourceBucketRead",
            "update": "pkg/internal/service/s3.resourceBucketUpdate",
            "delete": "pkg/internal/service/s3.resourceBucketDelete"
        },
        {
            "resource_type": "aws_immutable_thing",
            "create": "pkg/internal/service/x.resourceThingCreate",
            "read": "pkg/internal/service/x.resourceThingRead",
            "delete": "pkg/internal/service/x.resourceThingDelete"
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
    fn untagged_entry_has_no_tag_symbols() {
        // SAMPLE's entries carry no `tags` block.
        let map = CrudMap::from_slice(SAMPLE.as_bytes()).unwrap();
        let entry = map.get("aws_s3_bucket").unwrap();
        assert_eq!(entry.tags, None);
        assert_eq!(entry.list_tags_symbol(), None);
        assert_eq!(entry.update_tags_symbol(), None);
    }

    #[test]
    fn tagged_entry_exposes_tag_symbols() {
        let json = r#"[{
            "resource_type": "aws_bucket_like",
            "read": "pkg/internal/service/s3.resourceBucketRead",
            "tags": {
                "resource_type": "Bucket",
                "identifier_attribute": "bucket",
                "list_tags_symbol": "pkg/internal/service/s3.(*servicePackage).ListTags",
                "update_tags_symbol": "pkg/internal/service/s3.(*servicePackage).UpdateTags"
            }
        }]"#;
        let map = CrudMap::from_slice(json.as_bytes()).unwrap();
        let entry = map.get("aws_bucket_like").unwrap();
        assert_eq!(
            entry.list_tags_symbol(),
            Some("pkg/internal/service/s3.(*servicePackage).ListTags")
        );
        assert_eq!(
            entry.update_tags_symbol(),
            Some("pkg/internal/service/s3.(*servicePackage).UpdateTags")
        );
    }

    #[test]
    fn embedded_crud_map_parses_and_contains_known_resource() {
        let map = CrudMap::load().unwrap();
        let entry = map.get("aws_s3_bucket").unwrap();
        assert_eq!(entry.resource_type, "aws_s3_bucket");
    }
}
