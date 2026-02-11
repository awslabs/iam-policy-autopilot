//! Error types for IAM Policy Autopilot

use crate::aws::AwsError;
use crate::types::ApplyError;
use thiserror::Error;

/// Main error type for IAM Policy Autopilot operations
#[derive(Error, Debug)]
pub enum IamPolicyAutopilotError {
    #[error("AWS operation failed: {0}")]
    Aws(#[from] AwsError),

    #[error("Command execution failed: {0}")]
    Command(String),

    #[error("Policy synthesis failed: {0}")]
    PolicySynthesis(String),

    #[error("Principal validation failed: {0}")]
    PrincipalValidation(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Parsing error: {0}")]
    Parsing(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Apply operation failed: {0}")]
    Apply(#[from] ApplyError),
}

/// Result type alias
pub type IamPolicyAutopilotResult<T> = Result<T, IamPolicyAutopilotError>;

impl IamPolicyAutopilotError {
    pub fn command(msg: impl Into<String>) -> Self {
        Self::Command(msg.into())
    }
    pub fn policy_synthesis(msg: impl Into<String>) -> Self {
        Self::PolicySynthesis(msg.into())
    }
    pub fn principal_validation(msg: impl Into<String>) -> Self {
        Self::PrincipalValidation(msg.into())
    }
    pub fn configuration(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }
    pub fn parsing(msg: impl Into<String>) -> Self {
        Self::Parsing(msg.into())
    }
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }
    #[must_use]
    pub fn apply(e: ApplyError) -> Self {
        Self::Apply(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::AwsError;

    #[test]
    fn test_error_creation() {
        let error = IamPolicyAutopilotError::command("Test command error");
        assert!(error.to_string().contains("Command execution failed"));
    }

    #[test]
    fn test_aws_error_conversion() {
        let aws_error = AwsError::ConfigError("Test config error".to_string());
        let iam_error = IamPolicyAutopilotError::from(aws_error);
        assert!(iam_error.to_string().contains("AWS operation failed"));
    }
}
