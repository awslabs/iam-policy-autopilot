//! Shims around primitives which will eventually need replacements for Wasm

// Native filesystem provider implementation
pub(crate) mod filesystem;

// Native JSON provider implementation
pub(crate) mod json;

// Conditional compilation for provider types
/// Type alias for the filesystem provider implementation.
///
/// On native platforms, this resolves to [`NativeFileSystemProvider`](filesystem::NativeFileSystemProvider).
/// This allows for conditional compilation while maintaining a consistent API.
pub type FileSystemProvider = filesystem::NativeFileSystemProvider;

/// Type alias for the JSON provider implementation.
///
/// On native platforms, this resolves to [`NativeJsonProvider`](json::NativeJsonProvider).
/// This allows for conditional compilation while maintaining a consistent API.
pub type JsonProvider = json::NativeJsonProvider;
