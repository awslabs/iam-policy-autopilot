//! This crate provides the core business logic for AWS IAM Policy Autopilot:
//! - AccessDenied text parsing
//! - Policy synthesis
//! - Principal ARN resolution and basic IAM operations (inline policies)
//!

mod aws;
pub mod commands;
mod error;
mod parsing;
mod synthesis;
mod types;

// Re-exports for a small, focused public API
pub use aws::principal::{resolve_principal, PrincipalInfo, PrincipalKind};
pub use aws::AwsError;
pub use commands::IamPolicyAutopilotService;
pub use error::{IamPolicyAutopilotError, IamPolicyAutopilotResult};
pub use parsing::{normalize_s3_resource, parse};
pub use synthesis::{build_inline_allow, build_single_statement};
pub use types::{
    ApplyError, ApplyOptions, ApplyResult, DenialType, ParsedDenial, PlanResult, PolicyDocument,
    PolicyMetadata, StatementKey,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsing_sample_message() {
        let msg = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key";
        let parsed = parse(msg).expect("should parse");
        assert_eq!(parsed.action, "s3:GetObject");
        assert_eq!(parsed.resource, "arn:aws:s3:::my-bucket/my-key");
        assert_eq!(
            parsed.principal_arn,
            "arn:aws:iam::123456789012:user/testuser"
        );
    }
}
