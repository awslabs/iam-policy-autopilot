//! JavaScript/TypeScript AWS SDK extraction module
//!
//! This module provides functionality for extracting AWS SDK method calls
//! from JavaScript and TypeScript source code using ast-grep patterns.

pub(crate) mod argument_extractor;
pub(crate) mod extractor;
pub(crate) mod scanner;
pub(crate) mod shared;
pub(crate) mod types;
