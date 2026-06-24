//! Map a Terraform plan to `SdkMethodCall`s via the CRUD map + model.
//!
//! For each planned resource: look up its CRUD entry, take the handler symbol
//! for each exercised CRUD slot, resolve that symbol to a model join key, and
//! emit one `SdkMethodCall` per modeled SDK operation. `metadata` is `None` —
//! the action set is what drives IAM action resolution, and ARN refinement is
//! handled separately by the Terraform resource binder.

use std::collections::HashSet;

use crate::extraction::SdkMethodCall;

use super::crud_map::CrudMap;
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
            let Some(symbol) = entry.handler(*slot) else {
                continue;
            };
            let Some(key) = handler_key(symbol) else {
                // The model builder skips non-service handlers too, so this is
                // benign; note it at debug level via a warning the caller can
                // log if desired.
                continue;
            };
            let Some(operations) = model.operations(&key) else {
                continue;
            };

            for op in operations {
                if seen.insert((op.service.clone(), op.operation.clone())) {
                    calls.push(SdkMethodCall {
                        name: op.operation.clone(),
                        possible_services: vec![op.service.clone()],
                        metadata: None,
                    });
                }
                emitted_any = true;
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
                "create_without_timeout": "{PFX}{create}",
                "read_without_timeout": "{PFX}{read}"
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
}
