//! Platform-abstracted providers for I/O, HTTP, and concurrency.
//!
//! This module contains implementations that differ between native and WASM targets.
//! Contributors adding new platform-specific code should add it here so the rest of
//! the codebase remains target-agnostic.

// Native filesystem provider implementation
pub(crate) mod filesystem;

// Native JSON provider implementation
pub(crate) mod json;

// Platform-abstracted concurrent task execution
pub(crate) mod concurrency;

/// Type alias for the filesystem provider implementation.
///
/// Available on all targets: native uses `tokio::fs`, WASM uses `std::fs`
/// against the emscripten virtual filesystem (see `filesystem.rs`).
pub type FileSystemProvider = filesystem::FileSystemProvider;

/// Type alias for the JSON provider implementation.
pub type JsonProvider = json::JsonProvider;
