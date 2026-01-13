//! IAM Policy Autopilot Core API Interface

mod extract_sdk_calls;
mod generate_policies;
mod get_submodule_version;
pub use extract_sdk_calls::extract_sdk_calls;
pub use generate_policies::generate_policies;
pub use get_submodule_version::{get_boto3_version_info, get_botocore_version_info};
mod common;
/// Get account context
pub mod get_account_context;
pub mod model;
