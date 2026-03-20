//! WebAssembly bindings for IAM Policy Autopilot.
//!
//! Extraction is done in JavaScript via web-tree-sitter.
//! This crate exposes `validateAndGeneratePolicies` which accepts
//! pre-extracted SDK calls as JSON and runs the Rust enrichment +
//! policy generation pipeline.

mod policy;

pub use policy::validate_and_generate_policies;
