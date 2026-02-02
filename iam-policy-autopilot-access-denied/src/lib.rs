//! This crate provides the core business logic for IAM Policy Autopilot:
//! - AccessDenied text parsing
//! - Policy synthesis
//! - Principal ARN resolution and basic IAM operations (inline policies)
//!

pub mod aws;
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

    #[test]
    fn test_parsing_sample_message_eusc() {
        let msg = "botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the GetMetricStatistics operation: User: arn:aws-eusc:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key";
        let parsed = parse(msg).expect("should parse");
        assert_eq!(parsed.action, "s3:GetObject");
        assert_eq!(parsed.resource, "arn:aws:s3:::my-bucket/my-key");
        assert_eq!(
            parsed.principal_arn,
            "arn:aws-eusc:iam::123456789012:user/testuser"
        );
    }

    #[test]
    fn test_parsing_sample_message_cn() {
        let msg = "Error: an error occurred invoking 'context deploy'
            with variables: {contexts:[] deployAll:false}
            caused by: 1 context deployment failures
            suggestion: To resolve failure 1, determine the cause of: operation error ECR: ListImages, https response error StatusCode: 400, RequestID: xxx, api error AccessDeniedException: User: arn:aws-cn:iam::680431765560:user/auser is not authorized to perform: ecr:ListImages on resource: arn:aws-cn:ecr:cn-northwest-1:680431765560:repository/aws/cromwell-mirror because no resource-based policy allows the ecr:ListImages action";
        let parsed = parse(msg).expect("should parse");
        assert_eq!(parsed.action, "ecr:ListImages");
        assert_eq!(
            parsed.resource,
            "arn:aws-cn:ecr:cn-northwest-1:680431765560:repository/aws/cromwell-mirror"
        );
        assert_eq!(
            parsed.principal_arn,
            "arn:aws-cn:iam::680431765560:user/auser"
        );
    }
}
