use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use crate::api::common::process_source_files;
use crate::extraction::call_graph::gopls::GoplsCallGraphBuilder;
use crate::extraction::call_graph::{CallGraphBuilder, FunctionNode};
use crate::extraction::external_library_models::ExternalLibraryModel;
use crate::model_generation::language_conventions::{GoConventions, LanguageConventions};
use crate::model_generation::Engine as ModelGenerationEngine;
use crate::Language;

/// Configuration for model generation.
pub struct GenerateModelConfig {
    /// Source files to analyze.
    pub source_files: Vec<PathBuf>,
    /// Override programming language detection.
    pub language: Option<String>,
    /// Name for the generated library model.
    pub library_name: String,
    /// Entry points in `file:line:column` format (1-based).
    pub entry_points: Vec<String>,
    /// Optional service hints to filter SDK calls to specific AWS services.
    pub service_hints: Option<Vec<String>>,
}

/// Generate an external library model from source files and entry points.
///
/// This builds a call graph via gopls, extracts SDK calls, and produces
/// an `ExternalLibraryModel` mapping each entry point to its reachable SDK operations.
pub async fn generate_model(config: &GenerateModelConfig) -> Result<ExternalLibraryModel> {
    info!("Generating model for library '{}'", config.library_name);

    // Canonicalize paths so that LSP URI resolution (which resolves symlinks)
    // produces paths matching those stored in SDK call locations by the extractor.
    let source_files: Vec<PathBuf> = config
        .source_files
        .iter()
        .map(|f| f.canonicalize().unwrap_or_else(|_| f.clone()))
        .collect();

    // Detect language and resolve conventions
    let language = {
        let extractor = crate::ExtractionEngine::new();
        let paths: Vec<&Path> = source_files.iter().map(|p| p.as_path()).collect();
        extractor.detect_and_validate_language(&paths)?
    };

    let conventions: Box<dyn LanguageConventions> = match language {
        Language::Go => Box::new(GoConventions),
        _ => anyhow::bail!("Model generation is not yet supported for {language}"),
    };

    let workspace_root = conventions.detect_workspace_root(&source_files)?;

    let mut builder: Box<dyn CallGraphBuilder> = match language {
        Language::Go => Box::new(
            GoplsCallGraphBuilder::new(&workspace_root)
                .await
                .context("Failed to start language server")?,
        ),
        _ => anyhow::bail!("Model generation is not yet supported for {language}"),
    };

    let graph = builder
        .build(&workspace_root, &source_files)
        .await
        .context("Failed to build call graph")?;

    let entry_nodes = resolve_entry_points(&config.entry_points, graph.nodes())?;

    let extracted = {
        use crate::api::model::ServiceHints;
        let service_hints = config.service_hints.as_ref().map(|names| ServiceHints {
            service_names: names.clone(),
        });
        let extractor = crate::ExtractionEngine::new();
        process_source_files(
            &extractor,
            &source_files,
            config.language.as_deref(),
            service_hints,
        )
        .await
        .context("Failed to extract SDK calls")?
    };

    let model = ModelGenerationEngine::new().generate(
        &graph,
        &entry_nodes,
        &extracted.methods,
        &config.library_name,
        language,
        conventions.as_ref(),
    );

    builder
        .shutdown()
        .await
        .context("Failed to shut down language server")?;

    Ok(model)
}

/// Resolve entry point specs in `file:line:column` format to FunctionNodes.
///
/// The position must point to the start of a function declaration (1-based).
fn resolve_entry_points(specs: &[String], nodes: &[FunctionNode]) -> Result<Vec<FunctionNode>> {
    let mut resolved = Vec::new();

    for spec in specs {
        let parts: Vec<&str> = spec.rsplitn(3, ':').collect();
        if parts.len() != 3 {
            anyhow::bail!(
                "Invalid entry point format '{spec}', expected 'file:line:column' (e.g. handler.go:14:1)"
            );
        }

        let col: usize = parts[0]
            .parse()
            .context(format!("Invalid column in entry point '{spec}'"))?;
        let line: usize = parts[1]
            .parse()
            .context(format!("Invalid line number in entry point '{spec}'"))?;
        let file = parts[2];
        let file_path = Path::new(file);

        let matching_files: HashSet<&PathBuf> = nodes
            .iter()
            .map(|n| &n.location.file_path)
            .filter(|p| p.ends_with(file_path))
            .collect();

        if matching_files.len() > 1 {
            anyhow::bail!(
                "Ambiguous file path '{file}' in entry point '{spec}', matches multiple files: {:?}. \
                 Use a longer path to disambiguate.",
                matching_files
            );
        }

        let node = nodes
            .iter()
            .find(|n| {
                n.location.file_path.ends_with(file_path)
                    && n.location.start_position == (line, col)
            })
            .with_context(|| {
                let available: Vec<String> = nodes
                    .iter()
                    .filter(|n| n.location.file_path.ends_with(file_path))
                    .map(|n| {
                        format!(
                            "{} ({}:{})",
                            n.name,
                            n.location.start_line(),
                            n.location.start_col()
                        )
                    })
                    .collect();
                format!("No function declaration at {spec}. Available functions in {file}: {available:?}")
            })?;

        resolved.push(node.clone());
    }

    Ok(resolved)
}
