//! Shims around primitives which will eventually need replacements for Wasm

// Native filesystem provider implementation
pub(crate) mod filesystem;

// Native JSON provider implementation
pub(crate) mod json;

// These type aliases are remnants of supporting compilation to wasm.
// We can use these eventually for conditional compilation again.
/// Type alias for the filesystem provider implementation.
pub type FileSystemProvider = filesystem::FileSystemProvider;

/// Type alias for the JSON provider implementation.
pub type JsonProvider = json::JsonProvider;
