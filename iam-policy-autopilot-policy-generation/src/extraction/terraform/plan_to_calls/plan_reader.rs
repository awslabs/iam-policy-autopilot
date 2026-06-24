//! Reader for `terraform show -json <plan>` output.
//!
//! Parses the documented Terraform plan JSON into the minimal shape the
//! mapper needs: each managed resource change's `type`, `address`, the planned
//! `actions`, and the `after` attributes (used to detect `name_prefix` for ARN
//! scoping, per the design doc §5.1).
//!
//! We deliberately consume the JSON produced by `terraform show -json`, not a
//! binary `.tfplan` — the binary format is version- and backend-specific, so
//! the user runs `terraform show -json plan.tfplan > plan.json` and passes the
//! JSON.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::extraction::terraform::AWS_RESOURCE_PREFIX;

use super::crud_map::CrudSlot;

/// Top-level `terraform show -json` document (only the fields we use).
#[derive(Debug, Deserialize)]
struct PlanDocument {
    #[serde(default)]
    resource_changes: Vec<RawResourceChange>,
}

#[derive(Debug, Deserialize)]
struct RawResourceChange {
    address: String,
    #[serde(rename = "type")]
    type_: String,
    /// `"managed"` for resources, `"data"` for data sources. We only model
    /// managed resources (data sources read existing infra, not part of the
    /// apply's write surface).
    #[serde(default)]
    mode: Option<String>,
    change: RawChange,
}

#[derive(Debug, Deserialize)]
struct RawChange {
    #[serde(default)]
    actions: Vec<String>,
    /// Planned post-apply attribute values. `null` keys (e.g. `name` when a
    /// `name_prefix` is used and the final name is known-after-apply) are
    /// preserved so the mapper can detect the `name_prefix` ARN-scoping case.
    #[serde(default)]
    after: Value,
}

/// A single managed-resource change extracted from the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedResource {
    /// Full resource address, e.g. `aws_s3_bucket.example`.
    pub(crate) address: String,
    /// Resource type, e.g. `aws_s3_bucket`.
    pub(crate) resource_type: String,
    /// The CRUD slots this change exercises (always includes `Read`).
    pub(crate) slots: BTreeSet<CrudSlot>,
    /// `name_prefix` value set in the planned attributes, when the resource
    /// uses a prefix instead of an explicit `name`. Captured for future
    /// prefix-glob ARN scoping (§5.1); not yet consumed — ARN binding currently
    /// comes from `.tf`/`.tfstate` via the existing resource binder.
    pub(crate) name_prefix: Option<String>,
}

/// Parse the CRUD slots a Terraform `actions` array implies.
///
/// Per the design doc §3, `Read` is always included (the provider reads state
/// back on every apply), and a replace (`create`+`delete`) is the union of
/// both write slots.
///
/// - `["create"]`            → Create, Read
/// - `["update"]`            → Update, Read
/// - `["delete"]`            → Delete, Read
/// - `["create","delete"]` / `["delete","create"]` → Create, Delete, Read
/// - `["no-op"]`, `["read"]` → Read
fn slots_for_actions(actions: &[String]) -> BTreeSet<CrudSlot> {
    let mut slots = BTreeSet::new();
    slots.insert(CrudSlot::Read);
    for action in actions {
        match action.as_str() {
            "create" => {
                slots.insert(CrudSlot::Create);
            }
            "update" => {
                slots.insert(CrudSlot::Update);
            }
            "delete" => {
                slots.insert(CrudSlot::Delete);
            }
            // "no-op" and "read" contribute only the always-on Read slot.
            _ => {}
        }
    }
    slots
}

/// Extract a `name_prefix` attribute from the planned `after` object, if the
/// resource sets one and does *not* set an explicit `name`. Terraform's schema
/// makes `name` and `name_prefix` mutually exclusive, so an explicit `name`
/// means the final identifier is known and prefix-globbing is unnecessary.
fn extract_name_prefix(after: &Value) -> Option<String> {
    let obj = after.as_object()?;
    // An explicit, non-null `name` wins — no prefix scoping needed.
    if obj.get("name").is_some_and(|v| !v.is_null()) {
        return None;
    }
    match obj.get("name_prefix") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Returns `true` if `bytes` look like `terraform show -json` plan output.
///
/// A Terraform plan is a JSON object carrying a `format_version` and a
/// `resource_changes` array — the combination is specific to plan output and
/// does not collide with application JSON config or `.tfstate` (which has
/// `format_version` but no `resource_changes`). This lets a plan be passed as a
/// positional input and auto-detected, the same way other inputs are inferred,
/// without a dedicated flag.
pub(crate) fn looks_like_plan(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("format_version") && obj.get("resource_changes").is_some_and(Value::is_array)
}

/// Read a file and report whether it is a Terraform plan JSON. Returns `false`
/// for unreadable files (they are simply not treated as plans).
pub(crate) fn file_looks_like_plan(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| looks_like_plan(&bytes))
}

/// Parse a plan document from raw `terraform show -json` bytes.
fn from_slice(bytes: &[u8]) -> Result<Vec<PlannedResource>> {
    let doc: PlanDocument =
        serde_json::from_slice(bytes).context("Failed to parse terraform plan JSON")?;

    let resources = doc
        .resource_changes
        .into_iter()
        // Only managed AWS resources participate in IAM action derivation.
        .filter(|rc| rc.mode.as_deref() != Some("data"))
        .filter(|rc| rc.type_.starts_with(AWS_RESOURCE_PREFIX))
        .map(|rc| PlannedResource {
            address: rc.address,
            resource_type: rc.type_,
            slots: slots_for_actions(&rc.change.actions),
            name_prefix: extract_name_prefix(&rc.change.after),
        })
        .collect();

    Ok(resources)
}

/// Read and parse a `terraform show -json` plan file from disk.
pub(crate) fn read_plan(path: &Path) -> Result<Vec<PlannedResource>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read Terraform plan JSON at {}", path.display()))?;
    from_slice(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn slots(items: &[CrudSlot]) -> BTreeSet<CrudSlot> {
        items.iter().copied().collect()
    }

    #[rstest]
    #[case(&["create"], &[CrudSlot::Read, CrudSlot::Create])]
    #[case(&["update"], &[CrudSlot::Read, CrudSlot::Update])]
    #[case(&["delete"], &[CrudSlot::Read, CrudSlot::Delete])]
    #[case(&["create", "delete"], &[CrudSlot::Read, CrudSlot::Create, CrudSlot::Delete])]
    #[case(&["delete", "create"], &[CrudSlot::Read, CrudSlot::Create, CrudSlot::Delete])]
    #[case(&["no-op"], &[CrudSlot::Read])]
    #[case(&["read"], &[CrudSlot::Read])]
    fn slots_for_actions_maps_each_action_set(
        #[case] actions: &[&str],
        #[case] expected: &[CrudSlot],
    ) {
        let actions: Vec<String> = actions.iter().map(|s| s.to_string()).collect();
        assert_eq!(slots_for_actions(&actions), slots(expected));
    }

    #[test]
    fn parses_managed_resource_change() {
        let json = r#"{
            "resource_changes": [
                {
                    "address": "aws_s3_bucket.example",
                    "type": "aws_s3_bucket",
                    "mode": "managed",
                    "change": { "actions": ["create"], "after": { "name": "my-bucket" } }
                }
            ]
        }"#;
        let resources = from_slice(json.as_bytes()).unwrap();
        assert_eq!(
            resources,
            vec![PlannedResource {
                address: "aws_s3_bucket.example".to_string(),
                resource_type: "aws_s3_bucket".to_string(),
                slots: slots(&[CrudSlot::Read, CrudSlot::Create]),
                name_prefix: None,
            }]
        );
    }

    #[test]
    fn skips_data_sources_and_non_aws_resources() {
        let json = r#"{
            "resource_changes": [
                { "address": "data.aws_ami.x", "type": "aws_ami", "mode": "data",
                  "change": { "actions": ["read"], "after": {} } },
                { "address": "random_id.x", "type": "random_id", "mode": "managed",
                  "change": { "actions": ["create"], "after": {} } },
                { "address": "aws_s3_bucket.x", "type": "aws_s3_bucket", "mode": "managed",
                  "change": { "actions": ["create"], "after": {} } }
            ]
        }"#;
        let resources = from_slice(json.as_bytes()).unwrap();
        let types: Vec<&str> = resources.iter().map(|r| r.resource_type.as_str()).collect();
        assert_eq!(types, vec!["aws_s3_bucket"]);
    }

    #[rstest]
    // name_prefix set, no explicit name → captured.
    #[case(r#"{ "name_prefix": "my-bucket-" }"#, Some("my-bucket-"))]
    // explicit name set → prefix ignored even if present.
    #[case(r#"{ "name": "final-name", "name_prefix": "my-bucket-" }"#, None)]
    // name null, name_prefix set → captured (the known-after-apply case).
    #[case(r#"{ "name": null, "name_prefix": "my-bucket-" }"#, Some("my-bucket-"))]
    // neither set → none.
    #[case(r#"{ "other": "x" }"#, None)]
    fn extracts_name_prefix(#[case] after_json: &str, #[case] expected: Option<&str>) {
        let after: Value = serde_json::from_str(after_json).unwrap();
        assert_eq!(extract_name_prefix(&after), expected.map(str::to_string));
    }

    #[test]
    fn empty_plan_yields_no_resources() {
        let resources = from_slice(br#"{ "resource_changes": [] }"#).unwrap();
        assert_eq!(resources, vec![]);
    }

    #[rstest]
    // A real plan: has both format_version and a resource_changes array.
    #[case(r#"{ "format_version": "1.2", "resource_changes": [] }"#, true)]
    #[case(
        r#"{ "format_version": "1.2", "resource_changes": [{"type":"aws_s3_bucket"}] }"#,
        true
    )]
    // .tfstate has format_version but no resource_changes → not a plan.
    #[case(r#"{ "format_version": "4", "values": {} }"#, false)]
    // resource_changes without format_version → not a plan (be specific).
    #[case(r#"{ "resource_changes": [] }"#, false)]
    // resource_changes present but not an array → not a plan.
    #[case(r#"{ "format_version": "1.2", "resource_changes": {} }"#, false)]
    // Arbitrary application JSON → not a plan.
    #[case(r#"{ "name": "my-config", "settings": {} }"#, false)]
    // Not even JSON → not a plan.
    #[case(r#"package main"#, false)]
    fn looks_like_plan_detects_plan_json(#[case] contents: &str, #[case] expected: bool) {
        assert_eq!(looks_like_plan(contents.as_bytes()), expected);
    }
}
