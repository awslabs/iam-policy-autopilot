use crate::errors::Result;
use crate::{api::model::GitSubmoduleMetadata, embedded_data::GitSubmoduleVersionInfo};

/// Gets the version information for the boto3 submodule.
///
/// # Returns
///
/// Returns the Git submodule metadata for boto3, including commit hash and version information.
///
/// # Errors
///
/// Returns an error if the boto3 version information cannot be retrieved.
pub fn get_boto3_version_info() -> Result<GitSubmoduleMetadata> {
    GitSubmoduleVersionInfo::get_boto3_version_info()
}

/// Gets the version information for the botocore submodule.
///
/// # Returns
///
/// Returns the Git submodule metadata for botocore, including commit hash and version information.
///
/// # Errors
///
/// Returns an error if the botocore version information cannot be retrieved.
pub fn get_botocore_version_info() -> Result<GitSubmoduleMetadata> {
    GitSubmoduleVersionInfo::get_botocore_version_info()
}

/// Gets the terraform-provider-aws version tag the embedded Terraform model
/// (`terraform-model.json`) was built against, e.g. `v6.34.0`.
///
/// # Returns
///
/// `Some(tag)` if the embedded model records a version, `None` otherwise.
///
/// # Errors
///
/// Returns an error if the embedded model cannot be parsed.
pub fn get_terraform_model_version() -> Result<Option<String>> {
    crate::extraction::terraform::plan_to_calls::model_version().map_err(|e| {
        crate::errors::ExtractorError::Configuration {
            message: format!("Failed to read embedded Terraform model version: {e:#}"),
            source: None,
        }
    })
}
