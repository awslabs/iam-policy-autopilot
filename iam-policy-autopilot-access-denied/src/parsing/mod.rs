//! AccessDenied message parsing (pure Rust)

pub mod catalog;
pub mod utils;

pub use catalog::parse;
pub use utils::normalize_s3_resource;
