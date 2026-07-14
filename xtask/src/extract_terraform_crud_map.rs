//! Extract the Terraform resource → CRUD-handler map from the
//! terraform-provider-aws submodule.
//!
//! The extractor is a small Go program (`go/main.go`) that builds the provider
//! in-process and reflects over its registered resources to emit, per resource
//! type, the fully-qualified CRUD handler function symbols. It imports the
//! provider's `internal/` packages, so Go's `internal` visibility rule forces
//! it to live *inside* the provider's module tree — hence we copy it into the
//! submodule's `tools/` directory and run it there.
//!
//! Output is committed as `terraform-crud-map.json` and consumed by
//! `build-terraform-model` (resource → handler symbols → model lookup).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::utils::NonSparseSubmodule;

/// The Go extractor source, copied into the submodule at build time.
const GO_MAIN: &str = include_str!("../extract-terraform-crud-map/go/main.go");

/// Subdirectory under the provider's `tools/` where we drop the extractor.
const TOOL_SUBDIR: &str = "tools/terraform-crud-map-extractor";

/// Options for the crud-map extraction.
pub struct ExtractOptions {
    /// Root of the terraform-provider-aws checkout (the Go module root).
    pub terraform_provider_aws_root: PathBuf,
    /// Where to write `terraform-crud-map.json`.
    pub output: PathBuf,
}

/// A copied-in tool directory that is removed on drop, so a failed run never
/// leaves stray files inside the (otherwise pristine) submodule.
///
/// `#[must_use]`: cleanup happens in `Drop`, so the guard must be bound for the
/// work's duration — `ScopedToolDir::create(..)?;` would drop it immediately.
#[must_use = "the tool dir is removed when this guard is dropped; bind it for the work's duration"]
struct ScopedToolDir {
    dir: PathBuf,
}

impl ScopedToolDir {
    fn create(provider_root: &Path) -> Result<Self> {
        let dir = provider_root.join(TOOL_SUBDIR);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        std::fs::write(dir.join("main.go"), GO_MAIN)
            .with_context(|| format!("Failed to write extractor source into {}", dir.display()))?;
        Ok(Self { dir })
    }
}

impl Drop for ScopedToolDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            log::warn!(
                "Failed to clean up extractor dir {}: {e}",
                self.dir.display()
            );
        }
    }
}

/// Run the Go extractor against a materialized provider checkout and write the
/// CRUD map to `output`.
///
/// Takes `&NonSparseSubmodule` so it is only callable while the full tree is
/// materialized — the extractor needs the provider's `internal/` packages.
fn extract_crud_map(provider: &NonSparseSubmodule, output: &Path) -> Result<()> {
    let root = provider.root();
    anyhow::ensure!(
        root.join("go.mod").is_file(),
        "No go.mod at {} after materializing the checkout — is \
         --terraform-provider-aws-root a terraform-provider-aws checkout?",
        root.display()
    );

    // Copy the extractor into the module tree (auto-removed on drop).
    let tool = ScopedToolDir::create(root)?;
    log::info!("Running Go extractor in {}", tool.dir.display());

    // `go run .` resolves the package in the tool dir against the provider's
    // module. The output JSON path is the extractor's single CLI arg.
    let status = Command::new("go")
        .arg("run")
        .arg(".")
        .arg(output)
        .current_dir(&tool.dir)
        .status()
        .context("Failed to spawn `go run` — is Go installed and on PATH?")?;

    anyhow::ensure!(
        status.success(),
        "Go extractor failed (exit {:?}). If this is a Go toolchain/version error \
         after a provider bump, ensure the runner's Go supports the provider's \
         go.mod toolchain directive.",
        status.code()
    );

    log::info!("Wrote Terraform CRUD map to {}", output.display());
    Ok(())
}

/// Run the Go extractor against the provider submodule and write the crud map.
pub fn run(opts: ExtractOptions) -> Result<()> {
    let output = std::path::absolute(&opts.output).unwrap_or_else(|_| opts.output.clone());

    // Materialize the full tree if the checkout is sparse (restored on drop),
    // then run the extractor against it.
    let provider = NonSparseSubmodule::new(&opts.terraform_provider_aws_root)
        .context("Failed to materialize full provider checkout")?;
    extract_crud_map(&provider, &output)
}
