//! Map a Terraform plan to `SdkMethodCall`s via the CRUD map + model.
//!
//! For each planned resource: look up its CRUD entry, take the handler symbol
//! for each exercised CRUD slot, resolve that symbol to a model join key, and
//! emit one `SdkMethodCall` per modeled SDK operation. `metadata` is `None` —
//! the action set is what drives IAM action resolution, and ARN refinement is
//! handled separately by the Terraform resource binder.

use std::collections::HashSet;

use crate::extraction::SdkMethodCall;

use super::crud_map::{CrudMap, CrudSlot};
use super::go_symbol::handler_key;
use super::model_index::ModelIndex;
use super::plan_reader::PlannedResource;

/// Result of mapping a plan: the derived SDK calls plus any non-fatal warnings
/// (resource types we cannot model, handlers absent from the model). Warnings
/// are surfaced rather than swallowed so an unmodelable resource never silently
/// produces an under-scoped policy.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MappedPlan {
    pub(crate) calls: Vec<SdkMethodCall>,
    pub(crate) warnings: Vec<String>,
}

/// Map planned resources to SDK method calls.
pub(crate) fn map_plan(
    resources: &[PlannedResource],
    crud_map: &CrudMap,
    model: &ModelIndex,
) -> MappedPlan {
    let mut calls = Vec::new();
    // Dedupe identical (service, operation) calls across resources/slots; the
    // enrichment pipeline would collapse the resulting actions anyway, but a
    // smaller call list keeps downstream work and logs tidy.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut warnings = Vec::new();

    // Resolve a handler/tagging symbol to its modeled ops and append any not yet
    // seen. Returns whether the symbol resolved to at least one operation.
    let emit_symbol = |symbol: &str,
                       calls: &mut Vec<SdkMethodCall>,
                       seen: &mut HashSet<(String, String)>|
     -> bool {
        let Some(key) = handler_key(symbol) else {
            // Non-service handler (e.g. plugin SDK); the model builder skips
            // these too, so there is nothing to attribute. Benign.
            return false;
        };
        let Some(operations) = model.operations(&key) else {
            return false;
        };
        for op in operations {
            if seen.insert((op.service.clone(), op.operation.clone())) {
                calls.push(SdkMethodCall {
                    name: op.operation.clone(),
                    possible_services: vec![op.service.clone()],
                    metadata: None,
                });
            }
        }
        !operations.is_empty()
    };

    for resource in resources {
        let Some(entry) = crud_map.get(&resource.resource_type) else {
            warnings.push(format!(
                "Resource '{}' ({}) has no entry in the Terraform CRUD map; \
                 emitting no actions for it",
                resource.address, resource.resource_type
            ));
            continue;
        };

        let mut emitted_any = false;
        for slot in &resource.slots {
            // A missing slot (e.g. `update` on an immutable resource) is
            // expected and not a warning — those resources simply have no
            // handler for that lifecycle step.
            if let Some(symbol) = entry.handler(*slot) {
                emitted_any |= emit_symbol(symbol, &mut calls, &mut seen);
            }
        }

        // Transparent tagging: the provider's tag interceptor invokes the
        // service package's ListTags/UpdateTags around the CRUD handlers, so
        // those tag SDK calls are NOT reachable from the handler bodies. Apply
        // the interceptor's CRUD-slot => tag-call rule (see
        // sdkv2/tags_interceptor.go):
        //   - Read present              => ListTags  (tags read into state on refresh)
        //   - Create or Update present  => UpdateTags (write) AND ListTags (read-back)
        //   - Delete                    => no tag call
        // Only tag-managed resources carry these symbols.
        let does_create_or_update = resource.slots.contains(&CrudSlot::Create)
            || resource.slots.contains(&CrudSlot::Update);
        let reads_tags = does_create_or_update || resource.slots.contains(&CrudSlot::Read);

        if reads_tags {
            if let Some(symbol) = entry.list_tags_symbol() {
                emitted_any |= emit_symbol(symbol, &mut calls, &mut seen);
            }
        }
        if does_create_or_update {
            if let Some(symbol) = entry.update_tags_symbol() {
                emitted_any |= emit_symbol(symbol, &mut calls, &mut seen);
            }
        }

        if !emitted_any {
            warnings.push(format!(
                "Resource '{}' ({}) is in the CRUD map but no handler resolved to \
                 any modeled SDK operations; emitting no actions for it",
                resource.address, resource.resource_type
            ));
        }
    }

    MappedPlan { calls, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::terraform::plan_to_calls::crud_map::CrudSlot;
    use std::collections::BTreeSet;

    const PFX: &str = "github.com/hashicorp/terraform-provider-aws/internal/service/";

    fn crud_map_json(create: &str, read: &str) -> String {
        format!(
            r#"[{{
                "resource_type": "aws_accessanalyzer_analyzer",
                "create": "{PFX}{create}",
                "read": "{PFX}{read}"
            }}]"#
        )
    }

    const MODEL_JSON: &str = r#"{
        "library_name": "terraform-provider-aws",
        "language": "go",
        "version": "v6.34.0",
        "call_patterns": [
            {
                "module_path": "accessanalyzer",
                "function_name": "resourceAnalyzerCreate",
                "call_type": "function",
                "sdk_operations": [
                    { "service": "accessanalyzer", "operation": "CreateAnalyzer" },
                    { "service": "accessanalyzer", "operation": "GetAnalyzer" }
                ]
            },
            {
                "module_path": "accessanalyzer",
                "function_name": "resourceAnalyzerRead",
                "call_type": "function",
                "sdk_operations": [
                    { "service": "accessanalyzer", "operation": "GetAnalyzer" }
                ]
            }
        ]
    }"#;

    fn build(crud_json: &str) -> (CrudMap, ModelIndex) {
        (
            CrudMap::from_slice_for_test(crud_json.as_bytes()),
            ModelIndex::from_slice_for_test(MODEL_JSON.as_bytes()),
        )
    }

    fn resource(slots: &[CrudSlot]) -> PlannedResource {
        PlannedResource {
            address: "aws_accessanalyzer_analyzer.example".to_string(),
            resource_type: "aws_accessanalyzer_analyzer".to_string(),
            slots: slots.iter().copied().collect::<BTreeSet<_>>(),
            name_prefix: None,
        }
    }

    /// Collect (service, operation) pairs from the mapped calls for exact
    /// comparison without depending on emission order.
    fn pairs(mapped: &MappedPlan) -> BTreeSet<(String, String)> {
        mapped
            .calls
            .iter()
            .map(|c| (c.possible_services[0].clone(), c.name.clone()))
            .collect()
    }

    fn pair(service: &str, op: &str) -> (String, String) {
        (service.to_string(), op.to_string())
    }

    #[test]
    fn create_emits_create_handler_ops_and_dedupes() {
        let crud = crud_map_json(
            "accessanalyzer.resourceAnalyzerCreate",
            "accessanalyzer.resourceAnalyzerRead",
        );
        let (crud_map, model) = build(&crud);
        // Create action → Create + Read slots; both reference GetAnalyzer, which
        // must appear exactly once.
        let mapped = map_plan(
            &[resource(&[CrudSlot::Create, CrudSlot::Read])],
            &crud_map,
            &model,
        );
        assert_eq!(
            pairs(&mapped),
            [
                pair("accessanalyzer", "CreateAnalyzer"),
                pair("accessanalyzer", "GetAnalyzer"),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(mapped.warnings, Vec::<String>::new());
        assert!(mapped.calls.iter().all(|c| c.metadata.is_none()));
    }

    #[test]
    fn read_only_change_emits_read_handler_ops() {
        let crud = crud_map_json(
            "accessanalyzer.resourceAnalyzerCreate",
            "accessanalyzer.resourceAnalyzerRead",
        );
        let (crud_map, model) = build(&crud);
        let mapped = map_plan(&[resource(&[CrudSlot::Read])], &crud_map, &model);
        assert_eq!(
            pairs(&mapped),
            [pair("accessanalyzer", "GetAnalyzer")]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn unknown_resource_type_warns_and_emits_nothing() {
        let crud = crud_map_json(
            "accessanalyzer.resourceAnalyzerCreate",
            "accessanalyzer.resourceAnalyzerRead",
        );
        let (crud_map, model) = build(&crud);
        let unknown = PlannedResource {
            address: "aws_unmodeled_thing.x".to_string(),
            resource_type: "aws_unmodeled_thing".to_string(),
            slots: [CrudSlot::Read, CrudSlot::Create].into_iter().collect(),
            name_prefix: None,
        };
        let mapped = map_plan(&[unknown], &crud_map, &model);
        assert_eq!(mapped.calls, Vec::new());
        assert_eq!(mapped.warnings.len(), 1);
    }

    #[test]
    fn handler_with_no_model_pattern_warns() {
        // Create handler points at a symbol the model does not know.
        let crud = crud_map_json(
            "accessanalyzer.resourceMysteryCreate",
            "accessanalyzer.resourceMysteryRead",
        );
        let (crud_map, model) = build(&crud);
        let r = PlannedResource {
            address: "aws_accessanalyzer_analyzer.x".to_string(),
            resource_type: "aws_accessanalyzer_analyzer".to_string(),
            slots: [CrudSlot::Read, CrudSlot::Create].into_iter().collect(),
            name_prefix: None,
        };
        let mapped = map_plan(&[r], &crud_map, &model);
        assert_eq!(mapped.calls, Vec::new());
        assert_eq!(mapped.warnings.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Transparent tagging: CRUD-slot => tag-call rule
    // -----------------------------------------------------------------------

    use rstest::rstest;

    /// A tagged resource (S3-bucket-like): CRUD handlers with no tag ops of
    /// their own, plus ListTags/UpdateTags tag symbols.
    const TAGGED_CRUD_JSON: &str = r#"[{
        "resource_type": "aws_bucket_like",
        "create": "github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketCreate",
        "read": "github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketRead",
        "delete": "github.com/hashicorp/terraform-provider-aws/internal/service/s3.resourceBucketDelete",
        "tags": {
            "resource_type": "Bucket",
            "identifier_attribute": "bucket",
            "list_tags_symbol": "github.com/hashicorp/terraform-provider-aws/internal/service/s3.(*servicePackage).ListTags",
            "update_tags_symbol": "github.com/hashicorp/terraform-provider-aws/internal/service/s3.(*servicePackage).UpdateTags"
        }
    }]"#;

    /// Model where CRUD handlers carry NO tag ops (mirrors reality: the model
    /// builder can't reach them from the handlers); the tag ops live only under
    /// the ListTags/UpdateTags symbols.
    const TAGGED_MODEL_JSON: &str = r#"{
        "library_name": "terraform-provider-aws",
        "language": "go",
        "version": "v6.34.0",
        "call_patterns": [
            { "module_path": "s3", "function_name": "resourceBucketCreate", "call_type": "function",
              "sdk_operations": [ { "service": "s3", "operation": "CreateBucket" } ] },
            { "module_path": "s3", "function_name": "resourceBucketRead", "call_type": "function",
              "sdk_operations": [ { "service": "s3", "operation": "GetBucketAcl" } ] },
            { "module_path": "s3", "function_name": "resourceBucketDelete", "call_type": "function",
              "sdk_operations": [ { "service": "s3", "operation": "DeleteBucket" } ] },
            { "module_path": "s3", "class_name": "servicePackage", "function_name": "ListTags", "call_type": "instance_method",
              "sdk_operations": [ { "service": "s3", "operation": "GetBucketTagging" } ] },
            { "module_path": "s3", "class_name": "servicePackage", "function_name": "UpdateTags", "call_type": "instance_method",
              "sdk_operations": [ { "service": "s3", "operation": "PutBucketTagging" } ] }
        ]
    }"#;

    fn tagged_resource(slots: &[CrudSlot]) -> PlannedResource {
        PlannedResource {
            address: "aws_bucket_like.example".to_string(),
            resource_type: "aws_bucket_like".to_string(),
            slots: slots.iter().copied().collect::<BTreeSet<_>>(),
            name_prefix: None,
        }
    }

    #[rstest]
    // Read-only refresh (no-op / destroy refresh): Read => ListTags (the bug we
    // fixed — GetBucketTagging must appear), plus the Read handler op.
    #[case(
        &[CrudSlot::Read],
        &[("s3", "GetBucketAcl"), ("s3", "GetBucketTagging")]
    )]
    // Delete plan = Delete + Read slots: Delete contributes no tag call, Read
    // contributes ListTags. GetBucketTagging present; no PutBucketTagging.
    #[case(
        &[CrudSlot::Read, CrudSlot::Delete],
        &[("s3", "GetBucketAcl"), ("s3", "DeleteBucket"), ("s3", "GetBucketTagging")]
    )]
    // Create (+Read): Create => UpdateTags (write) AND ListTags (read-back).
    #[case(
        &[CrudSlot::Create, CrudSlot::Read],
        &[("s3", "CreateBucket"), ("s3", "GetBucketAcl"), ("s3", "GetBucketTagging"), ("s3", "PutBucketTagging")]
    )]
    // Update (+Read): Update => UpdateTags AND ListTags.
    #[case(
        &[CrudSlot::Update, CrudSlot::Read],
        &[("s3", "GetBucketAcl"), ("s3", "GetBucketTagging"), ("s3", "PutBucketTagging")]
    )]
    fn tag_call_rule_per_slot_set(#[case] slots: &[CrudSlot], #[case] expected: &[(&str, &str)]) {
        let crud_map = CrudMap::from_slice_for_test(TAGGED_CRUD_JSON.as_bytes());
        let model = ModelIndex::from_slice_for_test(TAGGED_MODEL_JSON.as_bytes());
        let mapped = map_plan(&[tagged_resource(slots)], &crud_map, &model);
        assert_eq!(
            pairs(&mapped),
            expected
                .iter()
                .map(|(s, o)| pair(s, o))
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(mapped.warnings, Vec::<String>::new());
    }

    #[test]
    fn untagged_resource_emits_no_tag_ops() {
        // No `tags` block => no ListTags/UpdateTags, even on Create+Read.
        let (crud_map, model) = build(&crud_map_json(
            "accessanalyzer.resourceAnalyzerCreate",
            "accessanalyzer.resourceAnalyzerRead",
        ));
        let mapped = map_plan(
            &[resource(&[CrudSlot::Create, CrudSlot::Read])],
            &crud_map,
            &model,
        );
        // Only the handler ops; nothing tag-related.
        assert_eq!(
            pairs(&mapped),
            [
                pair("accessanalyzer", "CreateAnalyzer"),
                pair("accessanalyzer", "GetAnalyzer"),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        );
    }
}
