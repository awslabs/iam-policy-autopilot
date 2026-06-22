use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use crate::api::common::process_source_files;
use crate::api::model::ServiceHints;
use crate::extraction::call_graph::gopls::GoplsCallGraphBuilder;
use crate::extraction::call_graph::{innermost_enclosing, CallGraphBuilder, FunctionNode};
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
    /// Entry points as language-specific symbols (Go: `pkg.func`).
    pub entry_point_symbols: Vec<String>,
    /// Optional service hints to filter SDK calls to specific AWS services.
    pub service_hints: Option<ServiceHints>,
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

    // Resolve the language once, honoring the `--language` override if given and
    // otherwise detecting from the source files. Everything downstream (call
    // graph conventions, language server, and SDK extraction) keys off this so
    // an override applies consistently across all of them.
    let language = match config.language.as_deref() {
        Some(override_str) => Language::try_from_str(override_str)
            .with_context(|| format!("Invalid --language override '{override_str}'"))?,
        None => {
            let extractor = crate::ExtractionEngine::new();
            let paths: Vec<&Path> = source_files.iter().map(PathBuf::as_path).collect();
            extractor.detect_and_validate_language(&paths)?
        }
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

    // Run the fallible pipeline in an inner block so the language server is
    // always shut down afterwards, even if any step fails — otherwise an error
    // between here and shutdown would leak the gopls process.
    let result: Result<ExternalLibraryModel> = async {
        let graph = builder
            .build(&workspace_root, &source_files)
            .await
            .context("Failed to build call graph")?;

        let entry_nodes = if config.entry_points.is_empty()
            && config.entry_point_symbols.is_empty()
        {
            graph
                .nodes()
                .iter()
                .filter(|n| conventions.is_exported(n))
                .cloned()
                .collect()
        } else {
            let mut nodes = resolve_entry_points(&config.entry_points, graph.nodes())?;
            for spec in &config.entry_point_symbols {
                let node = conventions
                    .resolve_symbol(spec, graph.nodes())
                    .with_context(|| format!("Failed to resolve entry point symbol '{spec}'"))?;
                nodes.push(node.clone());
            }
            nodes
        };

        let extracted = {
            let extractor = crate::ExtractionEngine::new();
            process_source_files(
                &extractor,
                &source_files,
                Some(language.to_string().as_str()),
                config.service_hints.clone(),
            )
            .await
            .context("Failed to extract SDK calls")?
        };

        Ok(ModelGenerationEngine::new().generate(
            &graph,
            &entry_nodes,
            &extracted.methods,
            &config.library_name,
            language,
            conventions.as_ref(),
        ))
    }
    .await;

    // Always shut the server down before propagating, so failures don't leak it.
    if let Err(e) = builder.shutdown().await {
        log::warn!("Failed to shut down language server: {e}");
    }

    result
}

/// Resolve entry point specs in `file:line:column` format to FunctionNodes.
///
/// The position (1-based) may point anywhere within a function declaration —
/// the `func` keyword, the function name, or its body. This matches the
/// positions reported by `gopls symbols` (the function name) as well as
/// editor cursor positions inside the function. When several functions
/// contain the position (e.g. a closure inside a function), the innermost
/// (smallest) enclosing function is chosen.
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

        let pos = (line, col);
        // `file_path` is a partial, user-supplied path, so match by suffix
        // against the canonical node paths. The helper picks the innermost
        // enclosing function when the position falls inside several.
        let node = innermost_enclosing(nodes, pos, |path| path.ends_with(file_path))
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
