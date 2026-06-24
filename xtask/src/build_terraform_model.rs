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
use iam_policy_autopilot_policy_generation::api::{
    generate_models_batch, terraform_service_hint, BatchOptions, ExternalLibraryModel,
    GenerateModelConfig,
};
use serde::Deserialize;

use crate::utils::NonSparseSubmodule;

/// Only handlers under this import-path segment are real provider resource
/// handlers we can model; anything else (e.g. `schema.NoopContext`) is skipped.
const SERVICE_PATH_MARKER: &str = "/internal/service/";

/// A single resource entry as emitted by the schema-extractor reflection tool.
/// Only the fields we need are deserialized; the rest (`schema`, timeouts, …)
/// are ignored.
#[derive(Debug, Deserialize)]
struct ResourceEntry {
    #[allow(dead_code)]
    resource_type: String,
    create_without_timeout: Option<String>,
    read_without_timeout: Option<String>,
    update_without_timeout: Option<String>,
    delete_without_timeout: Option<String>,
}

impl ResourceEntry {
    /// All non-empty CRUD handler symbols for this resource.
    fn handler_symbols(&self) -> impl Iterator<Item = &str> {
        [
            self.create_without_timeout.as_deref(),
            self.read_without_timeout.as_deref(),
            self.update_without_timeout.as_deref(),
            self.delete_without_timeout.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}

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

/// Split a reflection handler symbol into `(package, entry_point_symbol)`, or
/// `None` if it is not a provider service handler (e.g. lives in the plugin SDK).
///
/// The package is always the path segment immediately after
/// `internal/service/` — never inferred by splitting on dots, since the
/// qualifier after the package can itself contain dots / receivers.
///
/// The qualifier is normalized to a resolvable Go entry point:
/// - plain free function: `glue.resourceBucketRead` → `glue.resourceBucketRead`
/// - closure: `glue.resourceResourcePolicyPut.func1` → the enclosing function
///   `glue.resourceResourcePolicyPut` (Go runtime appends `.funcN` to closures)
/// - bound method value: `sqs.(*queueAttributeHandler).Upsert-fm` →
///   `sqs.(*queueAttributeHandler).Upsert` (Go runtime appends `-fm` to method
///   values; the resolver matches the method node by its receiver+name)
fn service_symbol(full: &str) -> Option<(String, String)> {
    let after = full.split_once(SERVICE_PATH_MARKER)?.1;
    // package = first path-or-dot-delimited segment after the marker.
    let pkg_end = after.find(['/', '.'])?;
    let package = &after[..pkg_end];
    // qualifier = everything after "<package>." (must be a '.', not a '/': a
    // '/' would mean a sub-package path we don't handle).
    if after.as_bytes().get(pkg_end) != Some(&b'.') {
        return None;
    }
    let qualifier = &after[pkg_end + 1..];
    if package.is_empty() || qualifier.is_empty() {
        return None;
    }

    let entry = normalize_go_entry_point(qualifier);
    if entry.is_empty() {
        return None;
    }
    Some((package.to_string(), format!("{package}.{entry}")))
}

/// Normalize a Go runtime qualifier (everything after `pkg.`) to the entry
/// point the symbol resolver can match.
///
/// The Go runtime decorates handler function names in two ways:
/// - method value: trailing `-fm`, e.g. `(*queueAttributeHandler).Upsert-fm`
///   → keep the receiver form `(*queueAttributeHandler).Upsert`.
/// - closure: trailing `.funcN` (possibly nested), prefixed by the enclosing
///   function chain, e.g. `resourceResourcePolicy.resourceResourcePolicyPut.func1`.
///   The real entry point is the innermost *named* function the closure is
///   defined in — the last identifier segment before the `.funcN` suffix
///   (`resourceResourcePolicyPut`) — which is the function gopls has a node for
///   and whose body holds the SDK calls.
fn normalize_go_entry_point(qualifier: &str) -> String {
    // Method value: strip `-fm`, keep the `(*Type).Method` form whole.
    if let Some(method) = qualifier.strip_suffix("-fm") {
        return method.to_string();
    }

    // Closure: strip trailing `.funcN` segments, then take the last named
    // segment of the remaining dotted chain.
    let mut q = qualifier;
    let mut had_closure = false;
    while let Some((head, tail)) = q.rsplit_once('.') {
        let is_closure_seg = tail.len() > 4
            && tail.starts_with("func")
            && tail[4..].bytes().all(|b| b.is_ascii_digit());
        if is_closure_seg {
            q = head;
            had_closure = true;
        } else {
            break;
        }
    }
    if had_closure {
        // For a closure, the enclosing named function is the last segment.
        return q.rsplit('.').next().unwrap_or(q).to_string();
    }

    // Plain free function (no decoration).
    q.to_string()
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
    let resources: Vec<ResourceEntry> =
        serde_json::from_str(&raw).context("Failed to parse Terraform CRUD map")?;

    let mut by_package: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut skipped = 0usize;
    for resource in &resources {
        for handler in resource.handler_symbols() {
            match service_symbol(handler) {
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
        let service_hints = go_sdk_import_package(&dir)?
            .and_then(|pkg| terraform_service_hint(&pkg))
            .map(|hint| vec![hint]);
        if service_hints.is_none() {
            log::warn!("No service hint resolved for package '{package}'; generating without one");
        }
        configs.push(GenerateModelConfig {
            source_files,
            language: Some("go".to_string()),
            library_name: format!("terraform-provider-aws-{package}"),
            entry_points: Vec::new(),
            entry_point_symbols: symbols.into_iter().collect(),
            service_hints,
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

    // Union all packages' call patterns into one model, sorted by
    // (module_path, function_name) for stable, diffable output.
    let mut patterns: BTreeMap<(String, String), _> = BTreeMap::new();
    for model in models {
        for pattern in model.call_patterns {
            let key = (pattern.module_path.clone(), pattern.function_name.clone());
            // Distinct handlers => distinct keys; a duplicate key would mean two
            // packages claimed the same (pkg, func), which the uniqueness
            // analysis says cannot happen — treat it as a hard error.
            if patterns.insert(key.clone(), pattern).is_some() {
                anyhow::bail!("Duplicate call pattern for {key:?} — model key is not unique");
            }
        }
    }

    let unified = ExternalLibraryModel {
        library_name: "terraform-provider-aws".to_string(),
        language: iam_policy_autopilot_policy_generation::Language::Go,
        version: None,
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

    #[test]
    fn service_symbol_plain_free_function() {
        let (pkg, sym) = service_symbol(&format!("{PFX}s3.resourceBucketRead")).unwrap();
        assert_eq!(pkg, "s3");
        assert_eq!(sym, "s3.resourceBucketRead");
    }

    #[test]
    fn service_symbol_closure_strips_to_enclosing_func() {
        // Go runtime: closure inside resourceResourcePolicyPut, attributed under
        // the enclosing resourceResourcePolicy chain with a .func1 suffix.
        let (pkg, sym) = service_symbol(&format!(
            "{PFX}glue.resourceResourcePolicy.resourceResourcePolicyPut.func1"
        ))
        .unwrap();
        assert_eq!(pkg, "glue");
        assert_eq!(sym, "glue.resourceResourcePolicyPut");
    }

    #[test]
    fn service_symbol_method_value_keeps_receiver() {
        let (pkg, sym) =
            service_symbol(&format!("{PFX}sqs.(*queueAttributeHandler).Upsert-fm")).unwrap();
        assert_eq!(pkg, "sqs");
        assert_eq!(sym, "sqs.(*queueAttributeHandler).Upsert");
    }

    #[test]
    fn service_symbol_rejects_non_service() {
        assert!(service_symbol(
            "github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema.NoopContext"
        )
        .is_none());
    }

    #[test]
    fn normalize_entry_point_forms() {
        assert_eq!(
            normalize_go_entry_point("resourceBucketRead"),
            "resourceBucketRead"
        );
        assert_eq!(
            normalize_go_entry_point("resourceResourcePolicyPut.func1"),
            "resourceResourcePolicyPut"
        );
        assert_eq!(
            normalize_go_entry_point("resourceResourcePolicy.resourceResourcePolicyPut.func2"),
            "resourceResourcePolicyPut"
        );
        assert_eq!(
            normalize_go_entry_point("(*queueAttributeHandler).Read-fm"),
            "(*queueAttributeHandler).Read"
        );
    }
}
