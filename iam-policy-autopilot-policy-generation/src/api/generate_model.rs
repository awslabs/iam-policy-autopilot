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

/// Options for a batch model-generation run.
pub struct BatchOptions {
    /// Workspace root the language server is started against. Every job's source
    /// files must live under this directory (they share one indexed workspace).
    pub workspace_root: PathBuf,
    /// Restart the language server after this many jobs to cap memory growth.
    ///
    /// gopls accumulates per-package analysis state that cannot be configured
    /// away or freed via `didClose`; over a large batch its memory grows until
    /// it OOMs. Restarting periodically hard-caps the footprint at the cost of
    /// re-paying the one-time workspace indexing each restart. `0` disables
    /// restarts (single server for the whole batch). Defaults to a value that
    /// balances peak memory against re-index overhead.
    pub restart_every_n: usize,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::new(),
            restart_every_n: 25,
        }
    }
}

/// Generate an external library model from source files and entry points.
///
/// Convenience wrapper for the single-job case: starts a language server,
/// generates one model, and shuts the server down. For many jobs that share a
/// workspace, prefer [`generate_models_batch`], which reuses one server (the
/// per-job cost is dominated by the server's one-time workspace indexing).
pub async fn generate_model(config: &GenerateModelConfig) -> Result<ExternalLibraryModel> {
    // Derive the workspace root the same way batch generation expects it, so the
    // single-job and batch paths share all downstream logic.
    let source_files = canonicalize_files(&config.source_files);
    let language = detect_language(&source_files)?;
    let conventions = conventions_for(language)?;
    let workspace_root = conventions.detect_workspace_root(&source_files)?;

    let options = BatchOptions {
        workspace_root,
        restart_every_n: 0, // single job — no restart needed
    };
    let mut models = generate_models_batch(std::slice::from_ref(config), &options).await?;
    Ok(models.pop().expect("one job yields one model"))
}

/// Generate models for many jobs that share a single workspace, reusing one
/// language server across all of them.
///
/// The server (e.g. gopls) pays a large one-time cost to index the workspace;
/// running every job against the same warm server amortizes that across the
/// whole batch instead of paying it per job. The server's lifecycle is owned
/// here — callers never see it. Any job failure aborts the whole batch.
///
/// All jobs must target the same language and have source files under
/// `options.workspace_root`.
pub async fn generate_models_batch(
    configs: &[GenerateModelConfig],
    options: &BatchOptions,
) -> Result<Vec<ExternalLibraryModel>> {
    if configs.is_empty() {
        return Ok(Vec::new());
    }

    // Canonicalize every job's paths up front (LSP resolves symlinks, so call
    // locations must match), and verify they live under the shared workspace.
    let canonical_root = options
        .workspace_root
        .canonicalize()
        .unwrap_or_else(|_| options.workspace_root.clone());
    let jobs: Vec<Vec<PathBuf>> = configs
        .iter()
        .map(|c| canonicalize_files(&c.source_files))
        .collect();
    for (config, files) in configs.iter().zip(&jobs) {
        for file in files {
            if !file.starts_with(&canonical_root) {
                anyhow::bail!(
                    "Job '{}' has source file outside workspace root {}: {}",
                    config.library_name,
                    canonical_root.display(),
                    file.display()
                );
            }
        }
    }

    // Detect language from the first job and require the rest to agree — one
    // server, one language. Resolving conventions per language keeps this
    // generic over future call-graph-backed languages.
    let language = detect_language(&jobs[0])?;
    let conventions = conventions_for(language)?;

    let mut builder = start_call_graph_builder(language, &canonical_root)
        .await
        .context("Failed to start language server")?;

    // Build each job's call graph against the shared, already-indexed server.
    // Jobs run serially: the dominant per-package cost is the server's one-time
    // workspace indexing, which is shared across the jobs in a restart window.
    //
    // The server is restarted every `restart_every_n` jobs to cap memory: gopls
    // accumulates per-package analysis state that grows unbounded over a large
    // batch (and cannot be freed via didClose or disabled via settings), so a
    // periodic fresh start is the only reliable cap. Each restart re-pays the
    // one-time workspace indexing.
    let restart_every_n = options.restart_every_n;
    let mut models = Vec::with_capacity(configs.len());
    let result: Result<()> = async {
        for (index, (config, source_files)) in configs.iter().zip(&jobs).enumerate() {
            // Restart before this job if we've hit the interval (never before
            // the first job — the server was just started).
            if restart_every_n != 0 && index != 0 && index % restart_every_n == 0 {
                log::info!("Restarting language server after {index} jobs to cap memory");
                let old = std::mem::replace(
                    &mut builder,
                    start_call_graph_builder(language, &canonical_root)
                        .await
                        .context("Failed to restart language server")?,
                );
                if let Err(e) = old.shutdown().await {
                    log::warn!("Failed to shut down language server during restart: {e}");
                }
            }

            if !builder.is_running() {
                anyhow::bail!(
                    "Language server is no longer running (failed before job '{}')",
                    config.library_name
                );
            }
            let model = generate_one(
                builder.as_mut(),
                conventions.as_ref(),
                language,
                config,
                source_files,
            )
            .await
            .with_context(|| format!("Failed to generate model for '{}'", config.library_name))?;
            models.push(model);
        }
        Ok(())
    }
    .await;

    // Always shut the server down, even on a job failure, before propagating.
    if let Err(e) = builder.shutdown().await {
        log::warn!("Failed to shut down language server: {e}");
    }
    result?;
    Ok(models)
}

/// Generate a single model against an already-running call-graph builder.
async fn generate_one(
    builder: &mut dyn CallGraphBuilder,
    conventions: &dyn LanguageConventions,
    language: Language,
    config: &GenerateModelConfig,
    source_files: &[PathBuf],
) -> Result<ExternalLibraryModel> {
    info!("Generating model for library '{}'", config.library_name);

    let t_build = std::time::Instant::now();
    let graph = builder
        .build(
            &config_workspace_root(conventions, source_files)?,
            source_files,
        )
        .await
        .context("Failed to build call graph")?;
    let build_ms = t_build.elapsed().as_millis();

    let entry_nodes = if config.entry_points.is_empty() && config.entry_point_symbols.is_empty() {
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

    let t_extract = std::time::Instant::now();
    let extracted = {
        let extractor = crate::ExtractionEngine::new();
        process_source_files(
            &extractor,
            source_files,
            config.language.as_deref(),
            config.service_hints.clone(),
        )
        .await
        .context("Failed to extract SDK calls")?
    };
    let extract_ms = t_extract.elapsed().as_millis();

    let t_gen = std::time::Instant::now();
    let model = ModelGenerationEngine::new().generate(
        &graph,
        &entry_nodes,
        &extracted.methods,
        &config.library_name,
        language,
        conventions,
    );

    // Per-package phase breakdown — one concise line per model. Useful for
    // monitoring batch runs and catching performance regressions (gopls call
    // graph build typically dominates).
    info!(
        "Timing [{}]: build={}ms extract={}ms generate={}ms | {} files, {} nodes, {} entry points, {} SDK calls",
        config.library_name,
        build_ms,
        extract_ms,
        t_gen.elapsed().as_millis(),
        source_files.len(),
        graph.nodes().len(),
        entry_nodes.len(),
        extracted.methods.len(),
    );

    Ok(model)
}

/// Canonicalize source paths so LSP URIs (which resolve symlinks) match the
/// paths the extractor stores in SDK call locations. Unresolvable paths are
/// kept as-is so the downstream error names the real input.
fn canonicalize_files(files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|f| f.canonicalize().unwrap_or_else(|_| f.clone()))
        .collect()
}

/// Detect and validate the source language for a set of files.
fn detect_language(source_files: &[PathBuf]) -> Result<Language> {
    let extractor = crate::ExtractionEngine::new();
    let paths: Vec<&Path> = source_files.iter().map(PathBuf::as_path).collect();
    Ok(extractor.detect_and_validate_language(&paths)?)
}

/// Resolve the language conventions for a model-generation-capable language.
fn conventions_for(language: Language) -> Result<Box<dyn LanguageConventions>> {
    match language {
        Language::Go => Ok(Box::new(GoConventions)),
        _ => anyhow::bail!("Model generation is not yet supported for {language}"),
    }
}

/// Start the call-graph builder (language server) for a language.
async fn start_call_graph_builder(
    language: Language,
    workspace_root: &Path,
) -> Result<Box<dyn CallGraphBuilder>> {
    match language {
        Language::Go => Ok(Box::new(GoplsCallGraphBuilder::new(workspace_root).await?)),
        _ => anyhow::bail!("Model generation is not yet supported for {language}"),
    }
}

/// The workspace root for a single job (the builder ignores this for the shared
/// server, but `build` still takes it; kept consistent with the conventions).
fn config_workspace_root(
    conventions: &dyn LanguageConventions,
    source_files: &[PathBuf],
) -> Result<PathBuf> {
    conventions.detect_workspace_root(source_files)
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
                "Ambiguous file path '{file}' in entry point '{spec}', matches multiple files: \
                 {matching_files:?}. Use a longer path to disambiguate."
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

/// The AWS service hint (Botocore service id) for an AWS SDK for Go v2 import
/// package — the last path segment of `aws-sdk-go-v2/service/<pkg>` that a
/// Terraform provider service imports (e.g. `elasticsearchservice`,
/// `chimesdkvoice`, `applicationautoscaling`).
///
/// The Go import segment is the dash-free Smithy service name; this resolves it
/// to the dashed Botocore service id the SDK-call extractor recognizes
/// (`elasticsearchservice` → `es`, `chimesdkvoice` → `chime-sdk-voice`). Uses
/// the shared SDK-import service map (also used for Java imports), falling back
/// to the segment itself when it is already a valid Botocore service.
///
/// Returns `None` when the resolved service is not one the extractor actually
/// knows — i.e. has no Botocore data (e.g. `evidently`, `qldb`,
/// `elastictranscoder`). Callers should then generate without a service hint
/// rather than passing one the extractor's hint validator would reject.
///
/// Deliberately NOT based on `arn_namespace`: that collapses distinct SDK
/// services to one IAM prefix (e.g. `chime`, `chimesdkvoice` both → `chime`),
/// which mis-attributes operations.
#[must_use]
pub fn terraform_service_hint(go_sdk_import_package: &str) -> Option<String> {
    // The set of services the extractor (and its hint validator) recognizes.
    // A returned hint MUST be a member, or extraction rejects it. The
    // smithy→botocore map can yield names absent from this set (services with
    // no bundled data), so both resolution paths are validated against it.
    let known_services = crate::embedded_data::BotocoreData::build_service_versions_map();

    let config = crate::service_configuration::load_service_configuration().ok()?;
    if let Some(service) = config
        .build_sdk_import_service_map()
        .get(go_sdk_import_package)
    {
        if known_services.contains_key(service) {
            return Some(service.clone());
        }
    }
    // Fallback: the import segment is already a valid Botocore service id
    // (e.g. `s3`, `ec2`, `events`) with no rename needed.
    known_services
        .contains_key(go_sdk_import_package)
        .then(|| go_sdk_import_package.to_string())
}
