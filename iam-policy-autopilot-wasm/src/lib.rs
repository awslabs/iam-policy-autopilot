//! WebAssembly entry point for IAM Policy Autopilot.
//!
//! This crate compiles the full extraction + enrichment + policy generation pipeline
//! to WebAssembly via `wasm32-unknown-emscripten`. It includes the Rust extraction
//! engine (ast-grep + tree-sitter) so there is a single source of truth for SDK call
//! extraction — no JS/TS extractor fork needed.
//!
//! # Exported functions
//!
//! - `generate_policies_wasm(json_input)` — accepts a JSON string describing source
//!   files and options, runs the full pipeline, returns JSON policies.
//! - `free_string(ptr)` — frees a string returned by `generate_policies_wasm`.
//!
//! # Safety
//!
//! This crate uses `unsafe` for C FFI interop with Emscripten (pointer passing for
//! string input/output). The unsafe blocks are limited to the two exported functions.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;

use iam_policy_autopilot_policy_generation::api::generate_policies_from_source;
use iam_policy_autopilot_policy_generation::api::model::AwsContext;
use iam_policy_autopilot_policy_generation::api::GenerateFromSourceConfig;
use iam_policy_autopilot_policy_generation::extraction::SourceFile;
use iam_policy_autopilot_policy_generation::Language;
use serde::Deserialize;

/// Input format for the WASM entry point.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateInput {
    /// Source files with their content and filenames.
    files: Vec<FileInput>,
    /// AWS region for ARN generation (e.g. "us-east-1"). Defaults to "*".
    #[serde(default = "default_wildcard")]
    region: String,
    /// AWS account ID for ARN generation. Defaults to "*".
    #[serde(default = "default_wildcard")]
    account: String,
    /// Optional language override (auto-detected from filename if omitted).
    language: Option<String>,
    /// Enable minimal policy size by allowing cross-service merging. Defaults to false.
    #[serde(default)]
    minimize_policy_size: bool,
}

#[derive(Deserialize)]
struct FileInput {
    /// Filename (used for language detection), e.g. "handler.py"
    filename: String,
    /// Full source code content.
    content: String,
}

fn default_wildcard() -> String {
    "*".to_string()
}

/// Main entry point exposed to JavaScript via Emscripten.
///
/// Accepts a JSON string, runs extraction + enrichment + policy generation,
/// returns a JSON string with the result. Caller must free the returned
/// pointer with `free_string`.
///
/// # Safety
/// - `input_ptr` must be a valid, non-null, null-terminated C string.
/// - The caller must free the returned pointer with `free_string`.
#[no_mangle]
pub unsafe extern "C" fn generate_policies_wasm(input_ptr: *const c_char) -> *mut c_char {
    // Null guard — CStr::from_ptr(null) is instant UB.
    if input_ptr.is_null() {
        let err = serde_json::json!({"error": "input_ptr is null"}).to_string();
        return CString::new(err).unwrap_or_default().into_raw();
    }

    let result = std::panic::catch_unwind(|| {
        let input_str = match CStr::from_ptr(input_ptr).to_str() {
            Ok(s) => s,
            Err(e) => {
                let err = serde_json::json!({"error": format!("Input is not valid UTF-8: {e}")})
                    .to_string();
                return CString::new(err).unwrap_or_default().into_raw();
            }
        };

        let output = run_generate(input_str);
        CString::new(output).unwrap_or_default().into_raw()
    });

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            let err = serde_json::json!({"error": "panic in generate_policies_wasm"}).to_string();
            CString::new(err).unwrap_or_default().into_raw()
        }
    }
}

/// Free a string previously returned by `generate_policies_wasm`.
///
/// # Safety
/// `ptr` must have been returned by `generate_policies_wasm` and must not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

fn run_generate(input_json: &str) -> String {
    let input: GenerateInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({"error": format!("Invalid input JSON: {e}")}).to_string();
        }
    };

    if input.files.is_empty() {
        return serde_json::json!({"Policies": []}).to_string();
    }

    // Detect or validate language
    let language = if let Some(ref lang_str) = input.language {
        match Language::try_from_str(lang_str) {
            Ok(l) => l,
            Err(e) => {
                return serde_json::json!({"error": format!("Unsupported language '{lang_str}': {e}")}).to_string();
            }
        }
    } else {
        // Detect from first file
        match SourceFile::detect_language(std::path::Path::new(&input.files[0].filename)) {
            Some(l) => l,
            None => {
                return serde_json::json!({"error": format!("Cannot detect language for '{}'. Specify 'language' in options.", input.files[0].filename)}).to_string();
            }
        }
    };

    // Build SourceFile structs from in-memory content
    let source_files: Vec<SourceFile> = input
        .files
        .iter()
        .map(|f| SourceFile::with_language(PathBuf::from(&f.filename), f.content.clone(), language))
        .collect();

    // Run the async pipeline on a single-threaded tokio runtime
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            return serde_json::json!({"error": format!("Failed to create async runtime: {e}")})
                .to_string();
        }
    };

    let result = rt.block_on(async {
        let aws_context = AwsContext::new(input.region, input.account)?;

        let config = GenerateFromSourceConfig {
            source_files,
            language,
            aws_context,
            minimize_policy_size: input.minimize_policy_size,
        };

        generate_policies_from_source(&config).await
    });

    match result {
        Ok(gen_result) => serde_json::to_string(&gen_result).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("Serialization failed: {e}")}).to_string()
        }),
        Err(e) => serde_json::json!({"error": format!("{e:#}")}).to_string(),
    }
}
