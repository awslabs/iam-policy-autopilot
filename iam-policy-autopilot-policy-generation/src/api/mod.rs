//! IAM Policy Autopilot Core API Interface

mod extract_sdk_calls;
mod generate_policies;
mod generate_policies_from_source;
mod get_submodule_version;
pub use extract_sdk_calls::extract_sdk_calls;
pub use generate_policies::generate_policies;
pub use generate_policies_from_source::{
    generate_policies_from_source, GenerateFromSourceConfig,
};
pub use get_submodule_version::{get_boto3_version_info, get_botocore_version_info};
pub(crate) mod common;
pub mod model;
