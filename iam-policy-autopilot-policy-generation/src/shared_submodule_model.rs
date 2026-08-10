/// Exposes the version, commit hash, and data hash for an embedded submodule.
/// The struct defined here is used in both build.rs and model.rs.
/// To share this struct in both the library and the build step, we define it here,
/// and use include!(...) to include it in both uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GitSubmoduleMetadata {
    /// the commit of the submodule, returned on calls to iam-policy-autopilot --version --verbose
    pub git_commit_hash: String,
    /// the version of the submodule, returned on calls to iam-policy-autopilot --version --verbose
    pub git_tag: Option<String>,
    /// the sha hash of the embedded data, returned on calls to iam-policy-autopilot --version --verbose
    pub data_hash: String,
}

impl std::fmt::Display for GitSubmoduleMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "commit_id={}, commit_tag={}, data_hash={}",
            self.git_commit_hash,
            self.git_tag.as_deref().unwrap_or("None"),
            self.data_hash
        )
    }
}