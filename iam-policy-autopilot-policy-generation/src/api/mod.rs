//! IAM Policy Autopilot Core API Interface

mod extract_sdk_calls;
#[cfg(feature = "model-generation")]
mod generate_model;
mod generate_policies;
mod get_submodule_version;
#[cfg(feature = "model-generation")]
pub use crate::extraction::external_library_models::ExternalLibraryModel;
pub use extract_sdk_calls::extract_sdk_calls;
#[cfg(feature = "model-generation")]
pub use generate_model::{
    generate_model, generate_models_batch, terraform_handler_symbol, terraform_service_hint,
    BatchOptions, GenerateModelConfig,
};
pub use generate_policies::generate_policies;
pub use get_submodule_version::{
    get_boto3_version_info, get_botocore_version_info, get_terraform_model_version,
};
pub(crate) mod common;
pub(crate) mod input_kind;
pub mod model;
