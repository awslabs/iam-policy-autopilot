use anyhow::{Context, Result};
use log::info;

use crate::{
    api::{common::process_source_files, model::ExtractSdkCallsConfig},
    ExtractedMethods,
};

/// Handle the extract-sdk-calls
pub async fn extract_sdk_calls(config: &ExtractSdkCallsConfig) -> Result<ExtractedMethods> {
    info!("Extracting Sdk Calls");

    // Create the extractor
    let extractor = crate::ExtractionEngine::new();

    // Process source files
    process_source_files(&extractor, &config.source_files, config.language.as_deref())
        .await
        .context("Failed to process source files")
}
