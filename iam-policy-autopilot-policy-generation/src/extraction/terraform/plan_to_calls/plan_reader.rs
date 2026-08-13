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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::extraction::terraform::{
    AttributeValue, ChangeSide, TerraformResource, AWS_RESOURCE_PREFIX,
};
use crate::Location;

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
    /// Pre-change (currently deployed) attribute values. `null` for a create.
    /// For a delete this is the only place the concrete identity lives (`after`
    /// is `null`); for a replace it is the *old* identity being destroyed.
    #[serde(default)]
    before: Value,
    /// Planned post-apply attribute values. `null` keys (e.g. `name` when a
    /// `name_prefix` is used and the final name is known-after-apply) are
    /// preserved so the mapper can detect the `name_prefix` ARN-scoping case.
    /// `null` for a delete.
    #[serde(default)]
    after: Value,
}

/// One concrete resource identity resolved from a plan change: which side of the
/// diff it came from, plus the known (non-null, string) attribute values the
/// resource binder turns into a scoped ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedIdentity {
    /// The change side this identity was read from (`after` for create/update,
    /// `before` for delete, and both for a replace).
    pub(crate) side: ChangeSide,
    /// Known, resolved top-level string attributes (e.g. `{"name": "my-table"}`).
    pub(crate) attributes: BTreeMap<String, String>,
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
    /// Concrete resource identities resolved from the plan's `before`/`after`
    /// attributes, which the resource binder turns into scoped ARNs.
    ///
    /// Usually one identity. A **replace** (`create`+`delete`) contributes two
    /// — the new (`After`) and old (`Before`) identities — because the apply
    /// creates one ARN and deletes the other; both belong in the policy.
    pub(crate) identities: Vec<PlannedIdentity>,
    /// The plan file this resource was read from.
    pub(crate) source_plan: PathBuf,
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

impl PlannedResource {
    /// Represent this planned resource's concrete identities as parsed
    /// HCL-style resources, so the shared resource binder can derive ARNs from
    /// their resolved attributes exactly as it does for `.tf`-sourced resources.
    ///
    /// Emits one [`TerraformResource`] per identity (see [`Self::identities`]),
    /// each tagged with the [`ChangeSide`] it came from.
    pub(crate) fn to_terraform_resources(&self) -> Vec<TerraformResource> {
        let local_name = local_name_from_address(&self.address);
        self.identities
            .iter()
            .map(|identity| {
                let attributes = identity
                    .attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), AttributeValue::Literal(v.clone())))
                    .collect();
                TerraformResource {
                    resource_type: self.resource_type.clone(),
                    local_name: local_name.clone(),
                    attributes,
                    location: Location::new(self.source_plan.clone(), (0, 0), (0, 0)),
                    change_side: Some(identity.side),
                }
            })
            .collect()
    }
}

/// Derive a resource's local name from its plan address (the last dotted
/// segment, with any `[index]`/`["key"]` suffix stripped). E.g.
/// `aws_s3_bucket.data` → `data`
fn local_name_from_address(address: &str) -> String {
    let last = address.rsplit('.').next().unwrap_or(address);
    match last.split_once('[') {
        Some((name, _)) => name.to_string(),
        None => last.to_string(),
    }
}

/// Collect the resolved, non-null, top-level string attributes from a plan
/// `before`/`after` object.
fn known_string_attributes(value: &Value) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if let Value::String(s) = val {
                attrs.insert(key.clone(), s.clone());
            }
        }
    }
    attrs
}

/// Resolve a change's concrete identities from its `before`/`after` values.
///
/// - create → `After`
/// - delete → `Before` (`after` is null)
/// - update → `After` (identity is stable in place)
/// - replace (`create`+`delete`) → both `After` (new) and `Before` (old) —
///   the apply creates one ARN and deletes the other
///
/// Identities with no known attributes (all values known-after-apply) are
/// dropped so those resources fall back to wildcard ARNs, as is a `Before` whose
/// attributes duplicate the `After` (an in-place replace that didn't rename).
fn resolve_identities(
    slots: &BTreeSet<CrudSlot>,
    before: &Value,
    after: &Value,
) -> Vec<PlannedIdentity> {
    let after_known = known_string_attributes(after);
    let before_known = known_string_attributes(before);
    let is_replace = slots.contains(&CrudSlot::Create) && slots.contains(&CrudSlot::Delete);

    let candidates = if is_replace {
        vec![
            (ChangeSide::After, after_known),
            (ChangeSide::Before, before_known),
        ]
    } else if !after_known.is_empty() {
        vec![(ChangeSide::After, after_known)]
    } else {
        vec![(ChangeSide::Before, before_known)]
    };

    let mut identities: Vec<PlannedIdentity> = Vec::new();
    for (side, attributes) in candidates {
        if attributes.is_empty() {
            continue;
        }
        // Skip a Before whose attributes exactly match an already-kept identity
        // (a replace that didn't change the naming attributes → one ARN).
        if identities.iter().any(|id| id.attributes == attributes) {
            continue;
        }
        identities.push(PlannedIdentity { side, attributes });
    }
    identities
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

/// Read and parse a `terraform show -json` plan file from disk. Each resource
/// records `path` as its `source_plan` for use as the binding's source location.
pub(crate) fn read_plan(path: &Path) -> Result<Vec<PlannedResource>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read Terraform plan JSON at {}", path.display()))?;
    let doc: PlanDocument =
        serde_json::from_slice(&bytes).context("Failed to parse terraform plan JSON")?;

    let resources = doc
        .resource_changes
        .into_iter()
        // Only managed AWS resources participate in IAM action derivation.
        .filter(|rc| rc.mode.as_deref() != Some("data"))
        .filter(|rc| rc.type_.starts_with(AWS_RESOURCE_PREFIX))
        .map(|rc| {
            let slots = slots_for_actions(&rc.change.actions);
            let identities = resolve_identities(&slots, &rc.change.before, &rc.change.after);
            PlannedResource {
                address: rc.address,
                resource_type: rc.type_,
                slots,
                name_prefix: extract_name_prefix(&rc.change.after),
                identities,
                source_plan: path.to_path_buf(),
            }
        })
        .collect();

    Ok(resources)
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

    /// Write `json` to a temp file and return the handle (kept alive by the
    /// caller). `read_plan` needs a real file; `.path()` is the recorded
    /// `source_plan`.
    fn write_plan(json: &str) -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().expect("create temp plan");
        file.write_all(json.as_bytes()).expect("write temp plan");
        file
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
        let plan = write_plan(json);
        let resources = read_plan(plan.path()).unwrap();
        assert_eq!(
            resources,
            vec![PlannedResource {
                address: "aws_s3_bucket.example".to_string(),
                resource_type: "aws_s3_bucket".to_string(),
                slots: slots(&[CrudSlot::Read, CrudSlot::Create]),
                name_prefix: None,
                identities: vec![ident(ChangeSide::After, &[("name", "my-bucket")])],
                source_plan: plan.path().to_path_buf(),
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
        let plan = write_plan(json);
        let resources = read_plan(plan.path()).unwrap();
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
        let plan = write_plan(r#"{ "resource_changes": [] }"#);
        let resources = read_plan(plan.path()).unwrap();
        assert_eq!(resources, vec![]);
    }

    /// Build a `BTreeMap` of attributes from `(key, value)` pairs.
    fn btm(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Build a `PlannedIdentity` for the given side from `(key, value)` pairs.
    fn ident(side: ChangeSide, pairs: &[(&str, &str)]) -> PlannedIdentity {
        PlannedIdentity {
            side,
            attributes: btm(pairs),
        }
    }

    #[rstest]
    // create: known values come from `after`.
    #[case(&["create"], serde_json::json!(null), serde_json::json!({"name": "a"}), vec![ident(ChangeSide::After, &[("name", "a")])])]
    // delete: `after` is null; the concrete identity is in `before`.
    #[case(&["delete"], serde_json::json!({"name": "a"}), serde_json::json!(null), vec![ident(ChangeSide::Before, &[("name", "a")])])]
    // update in place: identity is stable; `after` is used.
    #[case(&["update"], serde_json::json!({"name": "a"}), serde_json::json!({"name": "a"}), vec![ident(ChangeSide::After, &[("name", "a")])])]
    // replace with rename: BOTH the new (after) and old (before) identities, after first.
    #[case(&["delete", "create"], serde_json::json!({"name": "old"}), serde_json::json!({"name": "new"}),
        vec![ident(ChangeSide::After, &[("name", "new")]), ident(ChangeSide::Before, &[("name", "old")])])]
    // replace, name unchanged: the two identities collapse to the After one.
    #[case(&["create", "delete"], serde_json::json!({"name": "a"}), serde_json::json!({"name": "a"}), vec![ident(ChangeSide::After, &[("name", "a")])])]
    // create with a known-after-apply name: no known attributes → no identity (wildcard fallback).
    #[case(&["create"], serde_json::json!(null), serde_json::json!({}), Vec::new())]
    fn resolve_identities_by_action_set(
        #[case] actions: &[&str],
        #[case] before: Value,
        #[case] after: Value,
        #[case] expected: Vec<PlannedIdentity>,
    ) {
        let actions: Vec<String> = actions.iter().map(|s| s.to_string()).collect();
        let slots = slots_for_actions(&actions);
        assert_eq!(resolve_identities(&slots, &before, &after), expected);
    }

    #[test]
    fn known_string_attributes_keeps_only_scalar_strings() {
        let value = serde_json::json!({
            "name": "my-table",
            "read_capacity": 5,            // number → skipped
            "tags": { "env": "prod" },     // nested object → skipped
            "attribute": [{ "name": "id" }] // array → skipped
        });
        assert_eq!(
            known_string_attributes(&value),
            btm(&[("name", "my-table")])
        );
    }

    #[test]
    fn to_terraform_resources_tags_each_identity_with_its_side() {
        // A replace-with-rename resource: two identities → two TerraformResources
        // that share the real local name but carry distinct change sides, so the
        // binder keeps them apart (and only state-matches the Before one) while
        // both contribute an ARN.
        let planned = PlannedResource {
            address: "aws_dynamodb_table.app".to_string(),
            resource_type: "aws_dynamodb_table".to_string(),
            slots: slots(&[CrudSlot::Read, CrudSlot::Create, CrudSlot::Delete]),
            name_prefix: None,
            identities: vec![
                ident(ChangeSide::After, &[("name", "new")]),
                ident(ChangeSide::Before, &[("name", "old")]),
            ],
            source_plan: PathBuf::from("plan.json"),
        };
        let resources = planned.to_terraform_resources();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].resource_type, "aws_dynamodb_table");
        assert_eq!(resources[0].local_name, "app");
        assert_eq!(resources[0].change_side, Some(ChangeSide::After));
        // Location points at the source plan file.
        assert_eq!(resources[0].location.file_path, PathBuf::from("plan.json"));
        assert_eq!(
            resources[0].attributes.get("name"),
            Some(&AttributeValue::Literal("new".to_string()))
        );
        // Same real local name — the side, not a munged name, keeps it distinct.
        assert_eq!(resources[1].local_name, "app");
        assert_eq!(resources[1].change_side, Some(ChangeSide::Before));
        assert_eq!(
            resources[1].attributes.get("name"),
            Some(&AttributeValue::Literal("old".to_string()))
        );
    }

    #[rstest]
    #[case("aws_s3_bucket.data", "data")]
    #[case("module.m.aws_sqs_queue.jobs", "jobs")]
    #[case("aws_dynamodb_table.app[0]", "app")]
    #[case("aws_instance.web[\"a\"]", "web")]
    fn local_name_from_address_strips_module_and_index(
        #[case] address: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(local_name_from_address(address), expected);
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
