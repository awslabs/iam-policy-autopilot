//! WebAssembly bindings for IAM Policy Autopilot.
//!
//! Extraction is done in JavaScript via web-tree-sitter.
//! This crate exposes `validateAndGeneratePolicies` which accepts
//! pre-extracted SDK calls as JSON and runs the Rust enrichment +
//! policy generation pipeline.

mod policy;

pub use policy::validate_and_generate_policies;

use wasm_bindgen::prelude::*;

/// Initialize panic hook for better error messages in the browser console.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}
