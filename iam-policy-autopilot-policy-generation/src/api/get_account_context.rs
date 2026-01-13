use crate::context_fetcher::service::{AccountContextFetcherService, AccountResourceContext};
use crate::errors::ExtractorError;
use anyhow::{Context, Result};

/// Get account resources by calling resource explorer
pub async fn get_account_context() -> Result<(AccountResourceContext)> {
    let account_context = AccountContextFetcherService::new().await;

    Ok(account_context
        .fetch_account_context()
        .await
        .map_err(|e| ExtractorError::account_resource_context_with_source(e.to_string(), e))?)
}
