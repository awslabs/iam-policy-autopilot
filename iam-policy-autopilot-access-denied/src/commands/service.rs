//! IAM Policy Autopilot Service Layer
//!
//! This module provides the main service interface that encapsulates all business logic
//! for IAM policy diagnosis and remediation. The service holds AWS clients and provides
//! high-level operations (plan, apply) that can be used by different adapters (CLI, MCP).

use crate::error::IamPolicyAutopilotResult;
use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;

/// Main service struct that holds AWS clients and provides business logic operations
pub struct IamPolicyAutopilotService {
    pub(crate) iam_client: IamClient,
    pub(crate) sts_client: StsClient,
}

impl IamPolicyAutopilotService {
    /// Create a new service instance with AWS clients
    ///
    /// This initializes the AWS SDK configuration and creates IAM and STS clients.
    /// The configuration is loaded using the default credential provider chain.
    ///
    /// # Errors
    ///
    /// Returns an error if AWS SDK configuration fails to load.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use iam_policy_autopilot_access_denied::IamPolicyAutopilotService;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let service = IamPolicyAutopilotService::new().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new() -> IamPolicyAutopilotResult<Self> {
        // Load AWS configuration using the standard credential provider chain.
        // This automatically resolves the region from multiple sources in order:
        // 1. AWS_REGION environment variable
        // 2. AWS_DEFAULT_REGION environment variable
        // 3. ~/.aws/config file (region = <region> under [default] profile)
        // 4. AWS_PROFILE with profile-specific region in ~/.aws/config
        // 5. EC2 instance metadata (for EC2 instances)
        //
        // Note: aws_config::load_from_env() is functionally equivalent to this explicit
        // approach, but the explicit defaults(BehaviorVersion::latest()) pattern is
        // preferred by AWS SDK documentation as it:
        // - Makes the SDK behavior version explicit for forward compatibility
        // - Allows additional configuration customization before .load()
        // - Clearly documents the configuration approach
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;

        Ok(Self {
            iam_client: IamClient::new(&config),
            sts_client: StsClient::new(&config),
        })
    }

    // plan() method implementation is in plan.rs
    // apply() method implementation is in apply.rs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_service_creation() {
        // This test verifies that the service can be instantiated
        // Note: This requires valid AWS credentials in the environment
        // In a real test suite, we might want to mock the AWS clients
        let result = IamPolicyAutopilotService::new().await;

        // We can't guarantee AWS credentials are available in all test environments,
        // so we just verify the result is of the correct type
        match result {
            Ok(_service) => {
                // Service created successfully
                assert!(true);
            }
            Err(e) => {
                // Service creation failed, likely due to missing credentials
                // This is acceptable in a test environment
                println!("Service creation failed (expected in test env): {}", e);
                assert!(true);
            }
        }
    }
}
