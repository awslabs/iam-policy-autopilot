//! IAM Policy Autopilot Core API Interface

#[cfg(feature = "tree-sitter")]
mod extract_sdk_calls;
#[cfg(feature = "tree-sitter")]
mod generate_policies;
#[cfg(feature = "tree-sitter")]
mod generate_policies_from_source;
mod generate_policies_from_sdk_calls;
mod get_submodule_version;
#[cfg(feature = "tree-sitter")]
pub use extract_sdk_calls::extract_sdk_calls;
#[cfg(feature = "tree-sitter")]
pub use generate_policies::generate_policies;
#[cfg(feature = "tree-sitter")]
pub use generate_policies_from_source::{
    generate_policies_from_source, GenerateFromSourceConfig,
};
pub use generate_policies_from_sdk_calls::{
    generate_policies_from_sdk_calls, GenerateFromSdkCallsConfig,
};
pub use get_submodule_version::{get_boto3_version_info, get_botocore_version_info};
#[cfg(feature = "tree-sitter")]
pub(crate) #[cfg(feature = "tree-sitter")]
mod common;
pub mod model;
