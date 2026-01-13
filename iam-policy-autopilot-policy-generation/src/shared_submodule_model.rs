/// Exposes git version and commit hash for boto3 and botocore
/// The struct defined here is used in both build.rs and model.rs.
/// To share this struct in both the library and the build step, we define it here,
/// and use include!(...) to include it in both uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GitSubmoduleMetadata {
    /// the commit of boto3/botocore, returned on calls to iam-policy-autopilot --version --verbose
    pub git_commit_hash: String,
    /// the git tag of boto3/botocore, returned on calls to iam-policy-autopilot --version --verbose
    pub git_tag: Option<String>,
    /// the sha hash of boto3/botocore simplified models, returned on calls to iam-policy-autopilot --version --verbose
    pub data_hash: String,
}