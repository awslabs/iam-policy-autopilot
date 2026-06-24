//! Temporarily materialize a sparse git submodule for the duration of some work.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// A git submodule with its full tree temporarily materialized — sparse-checkout
/// is disabled on construction and the prior sparse patterns are restored on
/// drop, so the working tree (and `git status`) is left as we found it, even on
/// error.
///
/// Submodules are sparse-checked-out to a small subset for lean dev checkouts,
/// but some xtasks need the full tree (e.g. the provider's `internal/`
/// packages). Submodule-agnostic: hold an instance for the duration of work that
/// needs the full tree.
///
/// `#[must_use]`: the sparse state is reverted in `Drop`, so it must be bound for
/// the duration of the work — `NonSparseSubmodule::new(..)?;` would revert
/// immediately, removing the files the work needs.
#[must_use = "the full checkout is reverted when this is dropped; bind it for the work's duration"]
pub struct NonSparseSubmodule {
    root: PathBuf,
    /// Original sparse patterns to restore, or `None` if sparse was already off.
    restore: Option<Vec<String>>,
}

impl NonSparseSubmodule {
    /// Materialize the full tree at `root`, disabling sparse-checkout if enabled.
    pub fn new(root: &Path) -> Result<Self> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

        // Only act if sparse-checkout is enabled for this checkout.
        let sparse_on = git(&root, &["config", "--get", "core.sparseCheckout"])
            .map(|out| out.trim() == "true")
            .unwrap_or(false);
        if !sparse_on {
            return Ok(Self {
                root,
                restore: None,
            });
        }

        // Capture current patterns so we can restore the lean checkout after.
        let patterns: Vec<String> = git(&root, &["sparse-checkout", "list"])
            .context("Failed to list sparse-checkout patterns")?
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect();

        log::info!(
            "Disabling sparse-checkout to materialize the full tree at {}",
            root.display()
        );
        git(&root, &["sparse-checkout", "disable"]).context("Failed to disable sparse-checkout")?;

        Ok(Self {
            root,
            restore: Some(patterns),
        })
    }

    /// The (canonicalized) checkout root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for NonSparseSubmodule {
    fn drop(&mut self) {
        let Some(patterns) = self.restore.take() else {
            return; // sparse was already off — nothing to restore
        };
        log::info!("Restoring sparse-checkout to its prior patterns");
        // Re-enable in non-cone mode (path patterns) and reapply them.
        let mut args = vec!["sparse-checkout", "set", "--no-cone"];
        args.extend(patterns.iter().map(String::as_str));
        if let Err(e) = git(&self.root, &args) {
            log::warn!(
                "Failed to restore sparse-checkout in {}: {e} — the working tree is left \
                 fully materialized; run `git sparse-checkout set --no-cone {}` to restore",
                self.root.display(),
                patterns.join(" ")
            );
        }
    }
}

/// Run a `git` command in `dir`, returning stdout on success.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("Failed to run `git {}`", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "`git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
