use crate::errors::{ExtractorError, Result};
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
