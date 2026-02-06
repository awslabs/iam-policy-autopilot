//! STS helpers for caller identity

use crate::aws::{AwsError, AwsResult};
use aws_sdk_sts::Client as StsClient;

/// Return the current caller account ID using STS GetCallerIdentity.
///
/// This is used for same-account guardrail checks before attempting IAM mutations.
///
/// # Arguments
///
/// * `client` - STS client to use for the API call
pub async fn caller_account_id(client: &StsClient) -> AwsResult<String> {
    let out = client
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| AwsError::SdkError(format!("STS GetCallerIdentity failed: {e}")))?;
    let acct = out
        .account()
        .map(std::string::ToString::to_string)
        .ok_or_else(|| AwsError::SdkError("STS GetCallerIdentity missing Account".to_string()))?;
    Ok(acct)
}
