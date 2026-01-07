
/// Exposes git version and commit hash for boto3 and botocore
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GitSubmoduleMetadata {
    /// the commit of boto3/botocore, returned on calls to iam-policy-autopilot --version --debug
    pub git_commit_hash: String,
    /// the git tag of boto3/botocore, returned on calls to iam-policy-autopilot --version --debug
    pub git_tag: Option<String>,
    /// the sha hash of boto3/botocore simplified models, returned on calls to iam-policy-autopilot --version --debug
    pub data_hash: String,
}