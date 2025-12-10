//! AWS SDK integration: IAM client wrapper, principal parsing, policy naming.

pub(crate) mod iam_client;
pub mod policy_naming;
pub mod principal;
pub(crate) mod sts;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AwsError {
    #[error("AWS configuration error: {0}")]
    ConfigError(String),
    #[error("IAM client error: {0}")]
    IamError(String),
    #[error("Principal resolution error: {0}")]
    PrincipalError(String),
    #[error("Policy naming error: {0}")]
    PolicyError(String),
    #[error("AWS SDK error: {0}")]
    SdkError(String),
}

pub type AwsResult<T> = Result<T, AwsError>;
