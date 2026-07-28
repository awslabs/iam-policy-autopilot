//! `iac_runner` — benchmark orchestration library.
//!
//! This crate is a self-contained copy of the open-source integration-test
//! `runner` (see `integration-tests/runner/`), adapted for the paper
//! benchmarks: it can attach either inline policy documents or AWS
//! managed-policy ARNs to a temporary execution role (see [`types::RunPolicies`]),
//! and it can invoke a pre-built `iam-policy-autopilot` binary via
//! [`autopilot::generate_policies`].
//!
//! It orchestrates: CDK deploy → generate/attach IAM policies → temporary
//! execution role → run each language script → CDK destroy.

pub mod autopilot;
pub mod aws;
pub mod cdk;
pub mod execution;
pub mod helpers;
pub mod iam;
pub mod minimizer;
pub mod types;

// ---------------------------------------------------------------------------
// Flat re-exports
//
// The benchmark crates import these symbols directly from the crate root
// (`iac_runner::generate_policies`, `iac_runner::RunPolicies`, …), mirroring
// the original flat `iac-runner` library API.
// ---------------------------------------------------------------------------

pub use autopilot::generate_policies;
pub use aws::{get_aws_account_id, get_caller_arn};
pub use cdk::{cdk_deploy, cdk_destroy};
pub use execution::{run_language, run_language_with_policies};
pub use types::{LangConfig, LangSummary, RoleInfo, RunPolicies, SdkStats};
pub use types::language_configs;
