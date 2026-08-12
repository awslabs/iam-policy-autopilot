//! Build a single `ExternalLibraryModel` covering the entire Terraform AWS
//! provider, driven by the Terraform CRUD map (`terraform-crud-map.json`).
//!
//! Pipeline:
//!   1. Parse the CRUD map (resource_type + CRUD handler symbols).
//!   2. Group handler symbols by Go service package into one
//!      `GenerateModelConfig` each.
//!   3. Hand the whole batch to `generate_models_batch`, which reuses one
//!      language server across all packages.
//!   4. Union every package model's `call_patterns` into one model.
//!
//! Each handler symbol is fed via the `pkg.func` entry-point-symbol form, so the
//! lowercase SDKv2 free functions (which the default exported-functions heuristic
//! skips) are covered. `(module_path, function_name)` is provider-wide unique, so
//! the union needs no cross-package collision handling.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use iam_policy_autopilot_policy_generation::api::model::ServiceHints;
use iam_policy_autopilot_policy_generation::api::{
    generate_models_batch, terraform_handler_symbol, terraform_service_hint, BatchOptions,
    ExternalLibraryModel, GenerateModelConfig, TerraformCrudMapEntry,
};

use crate::utils::NonSparseSubmodule;

/// Options for the Terraform model build.
pub struct BuildOptions {
    /// Path to the Terraform CRUD map (`terraform-crud-map.json`).
    pub crud_map: PathBuf,
    /// Root of the terraform-provider-aws checkout (contains `internal/service`).
    pub terraform_provider_aws_root: PathBuf,
    /// Where to write the model JSON.
    pub output: PathBuf,
    /// If set, only build these packages (for iteration/debugging).
    pub only_packages: Option<Vec<String>>,
    /// Pretty-print the output JSON.
    pub pretty: bool,
}

/// Parse `output.json` and build one model-generation config per service
/// package. Handler symbols are passed as `pkg.func` entry points so the
/// lowercase SDKv2 free functions (which the default exported-functions
/// heuristic would skip) are covered.
fn plan_configs(opts: &BuildOptions, provider_root: &Path) -> Result<Vec<GenerateModelConfig>> {
    let raw = std::fs::read_to_string(&opts.crud_map).with_context(|| {
        format!(
            "Failed to read Terraform CRUD map at {}",
            opts.crud_map.display()
        )
    })?;
    let resources: Vec<TerraformCrudMapEntry> =
        serde_json::from_str(&raw).context("Failed to parse Terraform CRUD map")?;

    let mut by_package: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut skipped = 0usize;
    for resource in &resources {
        for handler in resource.handler_symbols() {
            match terraform_handler_symbol(handler) {
                Some((package, symbol)) => {
                    by_package.entry(package).or_default().insert(symbol);
                }
                None => skipped += 1,
            }
        }
    }

    log::info!(
        "Parsed {} resources -> {} service packages ({} non-service handler refs skipped)",
        resources.len(),
        by_package.len(),
        skipped
    );

    let service_root = provider_root.join("internal").join("service");
    let only: Option<BTreeSet<&str>> = opts
        .only_packages
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());

    let mut configs = Vec::new();
    for (package, symbols) in by_package {
        if let Some(filter) = &only {
            if !filter.contains(package.as_str()) {
                continue;
            }
        }
        let dir = service_root.join(&package);
        if !dir.is_dir() {
            log::warn!(
                "Skipping package '{package}': directory {} does not exist",
                dir.display()
            );
            continue;
        }
        let source_files = go_source_files(&dir)?;
        if source_files.is_empty() {
            anyhow::bail!("No .go source files in {}", dir.display());
        }
        // Resolve the service hint from the AWS SDK for Go v2 package the
        // service imports — its dash-free name maps to the dashed Botocore
        // service id the extractor recognizes (e.g. elasticsearchservice -> es).
        // A few services (evidently, qldb) have no Botocore data; for those we
        // generate without a hint rather than filtering against a missing
        // service.
        // Resolve the AWS service this package targets. A service with no SDK
        // data in the index (e.g. elastictranscoder, evidently, qldb) cannot be
        // modeled correctly — its operations would be mis-attributed to other
        // services that happen to share an operation name — so skip it entirely
        // rather than emit a wrong model. `None` here means either no SDK client
        // import or a service absent from the index; both are unmodelable.
        let Some(service_hint) =
            go_sdk_import_package(&dir)?.and_then(|pkg| terraform_service_hint(&pkg))
        else {
            log::warn!(
                "Skipping package '{package}': its AWS service has no data in the SDK index \
                 (cannot model it correctly)"
            );
            continue;
        };
        configs.push(GenerateModelConfig {
            source_files,
            language: Some("go".to_string()),
            library_name: format!("terraform-provider-aws-{package}"),
            entry_points: Vec::new(),
            entry_point_symbols: symbols.into_iter().collect(),
            service_hints: Some(ServiceHints {
                service_names: vec![service_hint],
            }),
        });
    }
    Ok(configs)
}

/// The AWS SDK for Go v2 package a Terraform service imports, read from its
/// generated `service_package_gen.go` (e.g. `elasticsearchservice` for the
/// `elasticsearch` service). Returns `None` if the file or import is absent
/// (e.g. framework-only services with no SDK client).
///
/// Each generated file wires up exactly one SDK client, so there is exactly one
/// such import in practice. If a service ever imports more than one distinct
/// SDK service package, that assumption is broken and we error rather than
/// silently picking one (which could mis-attribute the service hint).
fn go_sdk_import_package(service_dir: &Path) -> Result<Option<String>> {
    let gen = service_dir.join("service_package_gen.go");
    let Ok(content) = std::fs::read_to_string(&gen) else {
        return Ok(None);
    };
    let marker = "aws-sdk-go-v2/service/";
    let mut found: BTreeSet<String> = BTreeSet::new();
    for line in content.lines() {
        let Some(start) = line.find(marker) else {
            continue;
        };
        let rest = &line[start + marker.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        if end > 0 {
            found.insert(rest[..end].to_string());
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.into_iter().next()),
        _ => anyhow::bail!(
            "{} imports multiple AWS SDK service packages {:?}; service-hint resolution is ambiguous",
            gen.display(),
            found
        ),
    }
}

/// Collect all non-test `.go` files directly in a package directory.
fn go_source_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read package dir {}", dir.display()))?
    {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.ends_with(".go") && !name.ends_with("_test.go") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Build the unified provider model and write it to `opts.output`.
pub async fn run(opts: BuildOptions) -> Result<()> {
    let overall = Instant::now();

    // Materialize the full provider tree (restored on drop). Both reading the
    // service source files (plan_configs) and the gopls call-graph build need
    // the whole module, not the lean sparse checkout. Held for the whole run.
    let provider = NonSparseSubmodule::new(&opts.terraform_provider_aws_root)
        .context("Failed to materialize full provider checkout")?;

    // Record which provider version this model was built against, stamped into
    // the model so the provenance travels with the artifact (it cannot drift
    // from the committed model the way a build-time submodule hash could).
    let provider_version = provider
        .version()
        .context("Failed to determine terraform-provider-aws version")?;
    log::info!("Building against terraform-provider-aws {provider_version}");

    let configs = plan_configs(&opts, provider.root())?;
    let total = configs.len();
    log::info!("Building Terraform model: {total} packages");

    // Hand the whole batch to the policy-generation API, which reuses a single
    // language server across all packages (its one-time workspace indexing
    // dominates per-package cost). Any package failure aborts the whole build
    // inside the batch call — a partial provider model is silently wrong.
    let options = BatchOptions {
        workspace_root: provider.root().to_path_buf(),
        ..Default::default()
    };
    let models = generate_models_batch(&configs, &options)
        .await
        .context("Aborting unified-model build: a package failed")?;

    // Union all packages' call patterns into one model, keyed by
    // (module_path, class_name, function_name) — the same triple the consumer
    // joins on (see plan_to_calls go_symbol::HandlerKey). class_name is essential
    // for Plugin Framework resources, whose CRUD methods are all named
    // Create/Read/Update/Delete and are distinguished only by their receiver type
    // (e.g. amp `(*workspaceConfigurationResource).Create` vs
    // `(*scraperResource).Create`) — keying on (module_path, function_name) alone
    // would collide them. Sorted for stable, diffable output.
    let mut patterns: BTreeMap<(String, Option<String>, String), _> = BTreeMap::new();
    for model in models {
        for pattern in model.call_patterns {
            let key = (
                pattern.module_path.clone(),
                pattern.class_name.clone(),
                pattern.function_name.clone(),
            );
            // Distinct handlers => distinct keys; a duplicate key would mean two
            // resources claimed the same (pkg, class, func), which the uniqueness
            // analysis says cannot happen — treat it as a hard error.
            if patterns.insert(key.clone(), pattern).is_some() {
                anyhow::bail!("Duplicate call pattern for {key:?} — model key is not unique");
            }
        }
    }

    let unified = ExternalLibraryModel {
        library_name: "terraform-provider-aws".to_string(),
        language: iam_policy_autopilot_policy_generation::Language::Go,
        version: Some(provider_version),
        call_patterns: patterns.into_values().collect(),
    };

    let json = if opts.pretty {
        serde_json::to_string_pretty(&unified)?
    } else {
        serde_json::to_string(&unified)?
    };
    std::fs::write(&opts.output, json)
        .with_context(|| format!("Failed to write {}", opts.output.display()))?;

    let total_ops: usize = unified
        .call_patterns
        .iter()
        .map(|p| p.sdk_operations.len())
        .sum();
    log::info!(
        "Done in {:.1}s: {total} packages, {} call patterns, {total_ops} SDK operation refs -> {}",
        overall.elapsed().as_secs_f64(),
        unified.call_patterns.len(),
        opts.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "github.com/hashicorp/terraform-provider-aws/internal/service/";

    // The symbol parse itself is owned and unit-tested in policy-generation
    // (extraction::go::naming + plan_to_calls::go_symbol). These verify the
    // `terraform_handler_symbol` API seam this builder consumes reassembles the
    // `pkg.entry` form correctly across the four real handler shapes.

    #[test]
    fn handler_symbol_plain_free_function() {
        let (pkg, sym) = terraform_handler_symbol(&format!("{PFX}s3.resourceBucketRead")).unwrap();
        assert_eq!(pkg, "s3");
        assert_eq!(sym, "s3.resourceBucketRead");
    }

    #[test]
    fn handler_symbol_closure_strips_to_enclosing_func() {
        // Go runtime: closure inside resourceResourcePolicyPut, attributed under
        // the enclosing resourceResourcePolicy chain with a .func1 suffix.
        let (pkg, sym) = terraform_handler_symbol(&format!(
            "{PFX}glue.resourceResourcePolicy.resourceResourcePolicyPut.func1"
        ))
        .unwrap();
        assert_eq!(pkg, "glue");
        assert_eq!(sym, "glue.resourceResourcePolicyPut");
    }

    #[test]
    fn handler_symbol_method_value_keeps_receiver() {
        let (pkg, sym) =
            terraform_handler_symbol(&format!("{PFX}sqs.(*queueAttributeHandler).Upsert-fm"))
                .unwrap();
        assert_eq!(pkg, "sqs");
        assert_eq!(sym, "sqs.(*queueAttributeHandler).Upsert");
    }

    #[test]
    fn handler_symbol_rejects_non_service() {
        assert!(terraform_handler_symbol(
            "github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema.NoopContext"
        )
        .is_none());
    }
}
