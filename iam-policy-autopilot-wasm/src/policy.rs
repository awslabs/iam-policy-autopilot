//! WASM bindings for the policy generation pipeline.
//!
//! Extraction is performed in JavaScript using web-tree-sitter.
//! This module accepts pre-extracted SDK method calls as JSON and runs
//! the validation, enrichment, and policy generation pipeline in Rust.

use iam_policy_autopilot_policy_generation::api::model::AwsContext;
use iam_policy_autopilot_policy_generation::api::GenerateFromSdkCallsConfig;
use iam_policy_autopilot_policy_generation::extraction::SdkMethodCall;
use iam_policy_autopilot_policy_generation::Language;
use wasm_bindgen::prelude::*;

/// Resolve a `Language` from a string identifier.
fn resolve_language(language: &str) -> Result<Language, JsError> {
    Language::try_from_str(language).map_err(|e| JsError::new(&format!("{e}")))
}

/// Validate pre-extracted SDK calls against the AWS SDK model and generate
/// IAM policies.
///
/// # Arguments
/// * `sdk_calls_json` — JSON array of `SdkMethodCall` objects (PascalCase keys:
///   `Name`, `PossibleServices`). Typically produced by the JS extractor.
/// * `language` — Source language identifier (`python`, `go`, `javascript`, `typescript`).
/// * `region` — AWS region for ARN generation (e.g. `us-east-1`).
/// * `account` — AWS account ID (e.g. `123456789012`).
///
/// # Returns
/// JSON string of the generated IAM policies.
#[wasm_bindgen(js_name = "validateAndGeneratePolicies")]
pub async fn validate_and_generate_policies(
    sdk_calls_json: &str,
    language: &str,
    region: &str,
    account: &str,
) -> Result<String, JsError> {
    let lang = resolve_language(language)?;

    let sdk_calls: Vec<SdkMethodCall> =
        serde_json::from_str(sdk_calls_json).map_err(|e| JsError::new(&format!("{e}")))?;

    let aws_context =
        AwsContext::new(region.to_string(), account.to_string())
            .map_err(|e| JsError::new(&format!("{e}")))?;

    let config = GenerateFromSdkCallsConfig {
        sdk_calls,
        language: lang,
        aws_context,
        minimize_policy_size: false,
    };

    let result =
        iam_policy_autopilot_policy_generation::api::generate_policies_from_sdk_calls(&config)
            .await
            .map_err(|e| JsError::new(&format!("{e}")))?;

    serde_json::to_string_pretty(&result).map_err(|e| JsError::new(&format!("{e}")))
}
