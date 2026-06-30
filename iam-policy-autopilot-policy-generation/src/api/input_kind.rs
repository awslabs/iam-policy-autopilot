//! Classify the action-source inputs to `generate_policies`.
//!
//! `generate_policies` accepts a list of input files in its *action-source*
//! slot (`source_files`). Each is one of two kinds:
//!
//! - **`Source`** — application source code, classified by file extension
//!   (`.py`, `.go`, …) and analyzed by the tree-sitter extractors.
//! - **`Iac(TerraformPlan)`** — a `terraform show -json` plan, classified by
//!   *content* (a `.json` file is ambiguous by extension), mapped to SDK calls
//!   through the embedded CRUD map + model.
//!
//! The two kinds use different producers and cannot be mixed in one run, so we
//! classify all inputs up front and enforce a single consistency rule —
//! mirroring the existing "all source files must be the same language" check,
//! extended to also reject mixing a plan with source (or with a second
//! language). Multiple plans ARE allowed and are unioned downstream.
//!
//! Future IaC formats (plain `.tf` as an action source, CloudFormation) would
//! add variants to [`IacFormat`] without changing this structure.

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::extraction::terraform::plan_to_calls::file_looks_like_plan;

/// A declarative IaC action-source format. Today only the Terraform plan is
/// supported; plain Terraform config and CloudFormation are anticipated peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IacFormat {
    TerraformPlan,
}

/// The classified set of action-source inputs for one `generate_policies` run.
/// Exactly one variant applies; mixing is rejected by [`classify_inputs`].
#[derive(Debug)]
pub(crate) enum ClassifiedInputs {
    /// Application source files (all the same language; that language is what
    /// the extractor will use). Empty only when no inputs were given.
    Source(Vec<PathBuf>),
    /// One or more IaC documents of a single format, unioned downstream.
    Iac(IacFormat, Vec<PathBuf>),
}

/// Partition the action-source inputs into a single coherent kind, or error if
/// they cannot be analyzed together.
///
/// Rules:
/// - all inputs are Terraform plans → `Iac(TerraformPlan, _)` (any count).
/// - all inputs are source files of one language → `Source(_)`.
/// - empty → `Source(vec![])` (caller handles the no-input case).
/// - any mix (plan + source, or two source languages, or a plan + an
///   unrecognized file) → an error naming the offending files.
pub(crate) fn classify_inputs(files: &[PathBuf]) -> Result<ClassifiedInputs> {
    if files.is_empty() {
        return Ok(ClassifiedInputs::Source(Vec::new()));
    }

    // Plans are content-detected; everything else is treated as source and
    // classified by extension (the source path validates the language).
    let (plans, sources): (Vec<&PathBuf>, Vec<&PathBuf>) =
        files.iter().partition(|p| file_looks_like_plan(p));

    match (plans.is_empty(), sources.is_empty()) {
        // All plans → IaC. (Plan count is unconstrained; they union downstream.)
        (false, true) => Ok(ClassifiedInputs::Iac(
            IacFormat::TerraformPlan,
            plans.into_iter().cloned().collect(),
        )),

        // All source → defer language-consistency validation to the source path
        // (process_source_files), which already emits the canonical
        // mixed-language / unknown-extension errors.
        (true, false) => Ok(ClassifiedInputs::Source(
            sources.into_iter().cloned().collect(),
        )),

        // Mix of plan(s) and non-plan file(s) → not analyzable together.
        (false, false) => {
            use std::fmt::Write as _;
            let mut msg = String::from(
                "Cannot mix a Terraform plan with other inputs in one run.\n\
                 Pass either one or more `terraform show -json` plans, or application \
                 source files of a single language — not both.\n",
            );
            msg.push_str("  Terraform plans:\n");
            for p in &plans {
                let _ = writeln!(msg, "    {}", p.display());
            }
            msg.push_str("  Other inputs:\n");
            for p in &sources {
                let _ = writeln!(msg, "    {}", p.display());
            }
            bail!(msg)
        }

        // Unreachable: files is non-empty, so at least one partition is non-empty.
        (true, true) => unreachable!("non-empty inputs partitioned into two empty sets"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn empty_is_empty_source() {
        match classify_inputs(&[]).unwrap() {
            ClassifiedInputs::Source(v) => assert!(v.is_empty()),
            ClassifiedInputs::Iac(..) => panic!("expected Source"),
        }
    }

    #[rstest]
    #[case(&["a.py", "b.py"])]
    #[case(&["a.go"])]
    #[case(&["a.py", "b.go"])] // mixed language: classified as Source; the source
                               // path emits the canonical mixed-language error.
    fn all_source_classifies_as_source(#[case] files: &[&str]) {
        let files = paths(files);
        match classify_inputs(&files).unwrap() {
            ClassifiedInputs::Source(v) => assert_eq!(v, files),
            ClassifiedInputs::Iac(..) => panic!("expected Source for {files:?}"),
        }
    }

    // Plan detection is content-based, so the IaC/mixed cases need real files.

    fn write_temp(dir: &std::path::Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    const PLAN_JSON: &str = r#"{ "format_version": "1.2", "resource_changes": [] }"#;
    const NOT_A_PLAN_JSON: &str = r#"{ "some": "config", "settings": {} }"#;

    #[test]
    fn multiple_plans_classify_as_iac_union() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = write_temp(dir.path(), "a.tfplan.json", PLAN_JSON);
        let p2 = write_temp(dir.path(), "b.tfplan.json", PLAN_JSON);
        match classify_inputs(&[p1.clone(), p2.clone()]).unwrap() {
            ClassifiedInputs::Iac(IacFormat::TerraformPlan, v) => assert_eq!(v, vec![p1, p2]),
            ClassifiedInputs::Source(_) => panic!("expected Iac(TerraformPlan)"),
        }
    }

    #[test]
    fn plan_mixed_with_source_errors() {
        let dir = tempfile::tempdir().unwrap();
        let plan = write_temp(dir.path(), "plan.json", PLAN_JSON);
        let src = write_temp(dir.path(), "app.py", "import boto3\n");
        let err = classify_inputs(&[plan, src]).unwrap_err().to_string();
        assert!(err.contains("Cannot mix a Terraform plan with other inputs"));
    }

    #[test]
    fn non_plan_json_is_treated_as_source_not_iac() {
        // A .json that isn't a plan must NOT be misclassified as IaC; it goes to
        // the source path (which will then reject .json as an unknown language).
        let dir = tempfile::tempdir().unwrap();
        let cfg = write_temp(dir.path(), "config.json", NOT_A_PLAN_JSON);
        match classify_inputs(&[cfg.clone()]).unwrap() {
            ClassifiedInputs::Source(v) => assert_eq!(v, vec![cfg]),
            ClassifiedInputs::Iac(..) => panic!("non-plan JSON must not be IaC"),
        }
    }
}
