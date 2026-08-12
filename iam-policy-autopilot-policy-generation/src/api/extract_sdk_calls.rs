use anyhow::{Context, Result};
use log::{debug, info, warn};

use crate::{
    api::{
        common::process_source_files,
        input_kind::{classify_inputs, ClassifiedInputs, IacFormat},
        model::ExtractSdkCallsConfig,
    },
    extraction::{terraform::plan_to_calls, ExtractionMetadata},
    ExtractedMethods, Language,
};

/// Extract SDK method calls from a run's inputs.
///
/// The inputs are classified once ([`classify_inputs`]) into a single coherent
/// kind and resolved to the same `SdkMethodCall` currency:
/// - **application source files** → tree-sitter extraction (per detected/overridden language);
/// - **one or more Terraform plans** → resource changes mapped through the embedded
///   CRUD map + model.
///
/// Mixing the two kinds in one run is rejected by the classifier. This is the
/// shared front-end for both `extract-sdk-calls` and `generate_policies`; the
/// returned [`ExtractedMethods::sdk`] tells callers which SDK dialect the calls
/// belong to (the plan path has no `SourceFile`s to infer it from).
pub async fn extract_sdk_calls(config: &ExtractSdkCallsConfig) -> Result<ExtractedMethods> {
    info!("Extracting Sdk Calls");

    match classify_inputs(&config.source_files).context("Failed to classify inputs")? {
        ClassifiedInputs::Iac(IacFormat::TerraformPlan, plan_paths) => {
            // Report the input kind on the same telemetry dimension the source
            // path uses for the detected language (recorded in
            // `process_source_files`). A Terraform plan has no source language;
            // `"terraform_plan"` marks the run as plan-derived on that dimension.
            iam_policy_autopilot_common::telemetry::span::record_result_str(
                "detected_language",
                "terraform_plan",
            );
            debug!(
                "Deriving actions from {} Terraform plan(s)",
                plan_paths.len()
            );
            let (mapped, _resources) = plan_to_calls::plan_to_sdk_calls(&plan_paths)
                .context("Failed to derive SDK calls from Terraform plan(s)")?;
            for warning in &mapped.warnings {
                warn!("{warning}");
            }
            debug!(
                "Derived {} SDK calls from Terraform plan(s) ({} warning(s))",
                mapped.calls.len(),
                mapped.warnings.len()
            );
            let mut metadata = ExtractionMetadata::new(Vec::new(), mapped.warnings);
            metadata.update_method_count(mapped.calls.len());
            Ok(ExtractedMethods {
                methods: mapped.calls,
                metadata,
                // The model emits botocore service ids in `possible_services`,
                // exactly as the Go source extractor does, so the dialect is Go.
                sdk: Language::Go.sdk_type(),
            })
        }
        ClassifiedInputs::Source(source_files) => {
            // `detected_language` is recorded inside `process_source_files` for
            // the source path.
            let extractor = crate::ExtractionEngine::new();
            process_source_files(
                &extractor,
                &source_files,
                config.language.as_deref(),
                config.service_hints.clone(),
            )
            .await
            .context("Failed to process source files")
        }
    }
}
