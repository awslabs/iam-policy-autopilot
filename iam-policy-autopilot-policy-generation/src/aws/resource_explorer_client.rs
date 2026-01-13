use aws_sdk_resourceexplorer2::types::Resource;
use aws_sdk_resourceexplorer2::{config::Region, Client as ResourceExplorerClient};

use crate::aws::{AwsError, AwsResult};
/// Client to call AWS Resource Explorer v2
pub struct AwsResourceExplorerClient {
    pub(crate) client: ResourceExplorerClient,
}

/// Impl for AWS resource explorer client wrapper
impl AwsResourceExplorerClient {
    /// New construct
    pub fn new(client: ResourceExplorerClient) -> Self {
        Self { client }
    }

    /// List all resources by calling and paginagting resource explorer
    pub async fn list_resources(&self) -> AwsResult<Vec<Resource>> {
        let mut res_list = Vec::<Resource>::new();

        let mut next_token = Some(String::new());

        while next_token.is_some() {
            let mut query = self.client.list_resources().max_results(999);
            if next_token.clone().unwrap().len() > 0 {
                query = query.next_token(next_token.unwrap());
            }

            let out = query.send().await.map_err(|e| {
                AwsError::ResourceExplorerError(format!(
                    "Failed to call list-resources in resource explorer: {}",
                    e
                ))
            })?;

            next_token = out.next_token().map(|s| s.to_string());

            res_list.append(&mut out.resources.unwrap_or(Vec::new()));
        }

        AwsResult::Ok(res_list)
    }
}
