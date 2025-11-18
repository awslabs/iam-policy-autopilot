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
    pub async fn new() -> IamPolicyAutopilotResult<Self> {
        // Load AWS configuration using the standard credential provider chain.
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
