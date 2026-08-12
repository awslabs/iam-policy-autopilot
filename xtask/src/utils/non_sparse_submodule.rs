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

    /// A human-readable version string for the checked-out commit: the exact tag
    /// on HEAD (e.g. `v6.0.0`) if known, otherwise the short commit hash.
    ///
    /// A submodule is often a shallow, single-commit checkout with no tag refs,
    /// so `git describe` finds nothing. Rather than fetch *all* tags (which on a
    /// shallow clone pulls every tagged commit's history — hundreds of MB), we
    /// ask the remote which tag points at HEAD (`ls-remote`, metadata only — no
    /// objects fetched).
    pub fn version(&self) -> Result<String> {
        if let Some(tag) = self.describe_tags() {
            return Ok(tag);
        }
        if let Some(tag) = self.remote_tag_for_head() {
            return Ok(tag);
        }
        let commit = git(&self.root, &["rev-parse", "--short", "HEAD"])?;
        Ok(commit.trim().to_string())
    }

    /// `git describe --tags` if it yields a non-empty result, else `None`.
    fn describe_tags(&self) -> Option<String> {
        let desc = git(&self.root, &["describe", "--tags"]).ok()?;
        let desc = desc.trim();
        (!desc.is_empty()).then(|| desc.to_string())
    }

    /// The tag name pointing at HEAD, resolved from the remote via `ls-remote`.
    ///
    /// Metadata-only — no objects are fetched. On a shallow checkout (no local
    /// tag refs), this is how we name the version without pulling tag history.
    /// `None` if HEAD is not tagged or the lookup fails.
    fn remote_tag_for_head(&self) -> Option<String> {
        let head = git(&self.root, &["rev-parse", "HEAD"]).ok()?;
        let head = head.trim();

        // `ls-remote --tags` lists each tag's ref and, for annotated tags, its
        // peeled target as `refs/tags/<name>^{}`. Match HEAD against either, so
        // both lightweight and annotated tags resolve.
        let listing = git(&self.root, &["ls-remote", "--tags", "origin"]).ok()?;
        let tag_ref = listing.lines().find_map(|line| {
            let (sha, refname) = line.split_once('\t')?;
            (sha.trim() == head).then(|| refname.trim().to_string())
        })?;
        // Strip the `refs/tags/` prefix and any `^{}` peel suffix.
        Some(
            tag_ref
                .strip_prefix("refs/tags/")
                .unwrap_or(&tag_ref)
                .trim_end_matches("^{}")
                .to_string(),
        )
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
