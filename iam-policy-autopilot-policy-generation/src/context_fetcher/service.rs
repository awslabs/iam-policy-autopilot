use crate::aws::{
    resource_explorer_client::AwsResourceExplorerClient, sts::caller_account_id, AwsResult,
};
use aws_config::Region;
use aws_sdk_resourceexplorer2::{types::Resource, Client as ResourceExplorerClient};
use aws_sdk_sts::{operation::get_caller_identity, Client as StsClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) struct AccountMetadata {
    account_id: String,
    region: Option<Region>,
}

/// Account resource from sdk call
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct AccountResource {
    /// resource arn
    pub arn: String,
}

/// Map from resource service:resource_name to resource ARNs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct AccountResourceContext {
    /// map service:resource_type to resource ARNs
    pub resource_map: HashMap<String, Vec<AccountResource>>,
}

/// Main service struct that holds AWS clients and provides business logic operations
pub(crate) struct AccountContextFetcherService {
    pub(crate) sts_client: StsClient,
    pub(crate) resource_explorer_client: AwsResourceExplorerClient,
}

impl AccountContextFetcherService {
    pub async fn new() -> Self {
        // Load AWS configuration using the standard credential provider chain.
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;

        Self {
            resource_explorer_client: AwsResourceExplorerClient::new(ResourceExplorerClient::new(
                &config,
            )),
            sts_client: StsClient::new(&config),
        }
    }

    pub async fn get_account_metadata(&self) -> AwsResult<AccountMetadata> {
        Ok(AccountMetadata {
            account_id: caller_account_id(&self.sts_client).await?,
            region: self
                .resource_explorer_client
                .client
                .config()
                .region()
                .map(|r| r.clone()),
        })
    }

    /// TODO: add caching logic here.
    pub async fn fetch_account_context(&self) -> AwsResult<AccountResourceContext> {
        let resource_result = self.resource_explorer_client.list_resources().await?;

        let mut map = HashMap::<String, Vec<AccountResource>>::new();
        for resource in resource_result {
            if resource.resource_type().is_some() && resource.arn().is_some() {
                let resource_type = resource.resource_type().unwrap().to_string(); // e.g. s3:bucket
                if !map.contains_key(&resource_type) {
                    map.insert(resource_type.clone(), Vec::new());
                }
                map.get_mut(&resource_type).unwrap().push(AccountResource {
                    arn: resource.arn().unwrap().to_string(),
                });
            }
        }

        Ok(AccountResourceContext { resource_map: map })
    }
}
