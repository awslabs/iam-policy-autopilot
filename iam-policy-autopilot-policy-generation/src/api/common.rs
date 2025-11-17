use std::path::{Path, PathBuf};

use log::{info, trace, warn};

use crate::{ExtractedMethods, ExtractionEngine, Language, SourceFile};

use anyhow::{Context, Result};

/// Process source files and extract SDK method calls
pub(crate) async fn process_source_files(
    extractor: &ExtractionEngine,
    source_files: &[PathBuf],
    language_override: Option<&str>,
) -> Result<ExtractedMethods> {
    trace!("Processing {} source files", source_files.len());

    // Log the files being processed
    for (i, file) in source_files.iter().enumerate() {
        trace!("Source file {}: {}", i + 1, file.display());
    }

    // Convert PathBuf to &Path for language detection
    let source_file_paths: Vec<&Path> = source_files.iter().map(|p| p.as_path()).collect();

    // Determine the programming language to use
    let language = if let Some(override_lang) = language_override {
        info!("Using language override: {}", override_lang);
        override_lang.to_string()
    } else {
        // Detect and validate language consistency across all files
        let detected_language = extractor
            .detect_and_validate_language(&source_file_paths)
            .context("Failed to detect or validate programming language consistency")?;

        info!("Detected programming language: {}", detected_language);
        detected_language.to_string()
    };

    let language = Language::try_from_str(&language)?;

    // Load all source files into SourceFile objects
    let mut loaded_source_files = Vec::new();
    for file_path in source_files {
        let content = std::fs::read_to_string(file_path).context(format!(
            "Failed to read source file: {}",
            file_path.display()
        ))?;

        let source_file = SourceFile::with_language(file_path.clone(), content, language);
        loaded_source_files.push(source_file);
    }

    // Extract SDK method calls from the loaded source files
    let results = extractor
        .extract_sdk_method_calls(language, loaded_source_files)
        .await
        .context("Failed to extract SDK method calls from source files")?;

    info!(
        "Extraction completed: {} SDK method calls found from {} source files",
        results.methods.len(),
        results.metadata.source_files.len()
    );

    // Log warnings if any
    for warning in &results.metadata.warnings {
        warn!("{}", warning);
    }

    Ok(results)
}
