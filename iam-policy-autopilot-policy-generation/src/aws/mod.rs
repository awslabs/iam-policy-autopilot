//! AWS SDK integration: IAM client wrapper, principal parsing, policy naming.

/// resource explorer clients
pub mod resource_explorer_client;

/// sts calls
pub mod sts;

use thiserror::Error;

#[derive(Error, Debug)]
/// AWS Errors from AWS SDK calls
pub enum AwsError {
    #[error("AWS configuration error: {0}")]
    /// config error
    ConfigError(String),
    #[error("ResourceExplorer client error: {0}")]
    /// errors from calls to AWS Resource Explorer
    ResourceExplorerError(String),
    #[error("AWS SDK error: {0}")]
    /// errors from SDK output
    SdkError(String),
}

/// Type of AWS Result extending Result
pub type AwsResult<T> = Result<T, AwsError>;
