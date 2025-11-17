//! Plan creation logic for IAM Policy Autopilot service

use crate::error::{IamPolicyAutopilotError, IamPolicyAutopilotResult};
use crate::parsing::parse;
use crate::synthesis::build_inline_allow;
use crate::types::PlanResult;
use std::collections::HashSet;

impl super::service::IamPolicyAutopilotService {
    /// Create an execution plan from error text
    ///
    /// Analyzes AccessDenied error messages and creates a plan containing the parsed
    /// denial information, actions to be allowed, and synthesized IAM policy document.
    pub async fn plan(&self, error_text: &str) -> IamPolicyAutopilotResult<PlanResult> {
        let lines = extract_access_denied_lines(error_text);
        if lines.is_empty() {
            return Err(IamPolicyAutopilotError::parsing(
                "No AccessDenied messages found in provided text",
            ));
        }

        let candidates = deduplicate_candidates(lines);
        if candidates.is_empty() {
            return Err(IamPolicyAutopilotError::parsing(
                "No valid AccessDenied candidates found",
            ));
        }

        let preferred = candidates[0].clone();

        let mut parsed = parse(&preferred).ok_or_else(|| {
            IamPolicyAutopilotError::parsing(format!(
                "Failed to parse AccessDenied message: {}",
                preferred
            ))
        })?;

        // Normalize S3 resources for object operations
        parsed.resource = crate::parsing::normalize_s3_resource(&parsed.action, &parsed.resource);

        // Design: Single-action-per-statement granularity
        //
        // We process exactly ONE action per invocation to ensure:
        // - Fine-grained audit trail: Each Statement ID = one permission grant
        // - Iterative debugging workflow: Fix one error, run tool again for next
        //
        // If error text contains multiple denials (e.g., s3:GetObject and s3:PutObject),
        // the user runs the tool multiple times. Each run adds one statement to the policy.
        let actions = vec![parsed.action.clone()];

        let policy = build_inline_allow(actions.clone(), parsed.resource.clone());

        Ok(PlanResult {
            diagnosis: parsed,
            actions,
            policy,
        })
    }
}

/// Extract lines containing AccessDenied patterns from multi-line text
fn extract_access_denied_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains(" is not authorized to perform:")
        })
        .map(|s| s.trim().to_string())
        .collect()
}

/// Deduplicate candidate AccessDenied lines
fn deduplicate_candidates(lines: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for line in lines {
        let trimmed = line.trim().to_string();
        if seen.insert(trimmed.clone()) {
            candidates.push(trimmed);
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_access_denied_lines_with_valid_input() {
        let text = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key";
        let lines = extract_access_denied_lines(text);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("is not authorized to perform:"));
    }

    #[test]
    fn test_extract_access_denied_lines_with_noise() {
        let text = r#"ExceptionMessage: Something failed
User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key
Please fix and retry"#;
        let lines = extract_access_denied_lines(text);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("is not authorized to perform:"));
        assert!(lines[0].contains("s3:GetObject"));
    }

    #[test]
    fn test_extract_access_denied_lines_with_multiple_denials() {
        let text = r#"User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::bucket1/key1
User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: dynamodb:GetItem on resource: arn:aws:dynamodb:us-east-1:123456789012:table/MyTable"#;
        let lines = extract_access_denied_lines(text);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("s3:GetObject"));
        assert!(lines[1].contains("dynamodb:GetItem"));
    }

    #[test]
    fn test_extract_access_denied_lines_with_no_match() {
        let text = "Random error message without AccessDenied pattern";
        let lines = extract_access_denied_lines(text);
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_deduplicate_candidates() {
        let lines = vec![
            "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:GetObject"
                .to_string(),
            "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:GetObject"
                .to_string(),
            "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:PutObject"
                .to_string(),
        ];
        let deduped = deduplicate_candidates(lines);
        assert_eq!(deduped.len(), 2);
    }

    #[tokio::test]
    async fn test_plan_normalizes_s3_object_resources() {
        // Create service instance
        let service = crate::commands::service::IamPolicyAutopilotService::new()
            .await
            .expect("Failed to create service");

        // S3 GetObject error with object-specific ARN
        let error_text = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/path/to/file.txt";

        // Call plan
        let result = service.plan(error_text).await;
        assert!(result.is_ok(), "plan() should succeed");

        let plan = result.unwrap();

        // Verify the diagnosis.resource is normalized to bucket wildcard
        assert_eq!(
            plan.diagnosis.resource, "arn:aws:s3:::my-bucket/*",
            "S3 object ARN should be normalized to bucket wildcard in diagnosis"
        );

        // Verify the policy statement uses bucket wildcard
        assert_eq!(plan.policy.statement.len(), 1, "Should have one statement");
        assert_eq!(
            plan.policy.statement[0].resource, "arn:aws:s3:::my-bucket/*",
            "Policy statement should use bucket wildcard resource"
        );

        // Verify action is correct
        assert_eq!(plan.actions, vec!["s3:GetObject"]);
    }

    #[tokio::test]
    async fn test_plan_returns_error_on_no_match() {
        // Create service instance
        let service = crate::commands::service::IamPolicyAutopilotService::new()
            .await
            .expect("Failed to create service");

        // Text without AccessDenied pattern
        let error_text = "Random error message without AccessDenied pattern";

        // Call plan - should return error
        let result = service.plan(error_text).await;
        assert!(result.is_err(), "plan() should return error for non-AccessDenied text");

        // Verify error type is Parsing
        match result {
            Err(IamPolicyAutopilotError::Parsing(_)) => {
                // Expected error type
            }
            _ => panic!("Expected Parsing error"),
        }
    }

    #[tokio::test]
    async fn test_plan_deduplicates_multiple_identical_messages() {
        // Create service instance
        let service = crate::commands::service::IamPolicyAutopilotService::new()
            .await
            .expect("Failed to create service");

        // Multiple identical AccessDenied messages
        let error_text = r#"User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key
User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key
User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key"#;

        // Call plan
        let result = service.plan(error_text).await;
        assert!(result.is_ok(), "plan() should succeed with duplicate messages");

        let plan = result.unwrap();

        // Should only have one action despite three identical messages
        assert_eq!(
            plan.actions.len(),
            1,
            "Should deduplicate to single action"
        );
        assert_eq!(plan.actions[0], "s3:GetObject");

        // Policy should have one statement
        assert_eq!(
            plan.policy.statement.len(),
            1,
            "Should have one policy statement"
        );
    }
}
