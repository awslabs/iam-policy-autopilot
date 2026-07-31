//! IAM Policy Autopilot Core API Interface

mod extract_sdk_calls;
#[cfg(feature = "model-generation")]
mod generate_model;
mod generate_policies;
mod generate_policies_from_source;
mod get_submodule_version;
#[cfg(feature = "model-generation")]
pub use crate::extraction::external_library_models::ExternalLibraryModel;
pub use extract_sdk_calls::extract_sdk_calls;
#[cfg(feature = "model-generation")]
pub use generate_model::{generate_model, GenerateModelConfig};
pub use generate_policies::generate_policies;
pub use generate_policies_from_source::{generate_policies_from_source, GenerateFromSourceConfig};
pub use get_submodule_version::{get_boto3_version_info, get_botocore_version_info};
pub(crate) mod common;
pub mod model;
