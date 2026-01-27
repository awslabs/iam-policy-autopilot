use crate::aws::principal::PrincipalKind;
use convert_case::{Case, Casing};
use regex::Regex;
use std::sync::OnceLock;

// AWS IAM policy name character limit (128 characters)
// Reference: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_iam-quotas.html
const MAX_POLICY_NAME_LENGTH: usize = 128;
pub const POLICY_PREFIX: &str = "IamPolicyAutopilot";

fn sanitize_component(component: &str) -> String {
    static SANITIZE_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = SANITIZE_REGEX.get_or_init(|| Regex::new(r"[^a-zA-Z0-9+=,.@_-]").unwrap());
    let sanitized = regex.replace_all(component, "-").to_string();
    let cleaned = Regex::new(r"-+")
        .unwrap()
        .replace_all(&sanitized, "-")
        .trim_matches('-')
        .to_string();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn truncate_policy_name(name: &str) -> String {
    if name.len() <= MAX_POLICY_NAME_LENGTH {
        return name.to_string();
    }
    // Simple truncation - canonical names don't use hashes
    name.chars().take(MAX_POLICY_NAME_LENGTH).collect()
}

/// Generate canonical policy name: `IamPolicyAutopilot-{PrincipalName}`
///
/// Format is immutable; changing it would orphan existing user policies.
/// Sanitizes input and truncates to 128-char IAM limit.
/// TODO: Revisit policy naming based on decision on collecting analytics through policy names.
pub fn build_canonical_policy_name(_kind: &PrincipalKind, name: &str) -> String {
    let sanitized_name = sanitize_component(name);
    let full_name = format!("{}-{}", POLICY_PREFIX, sanitized_name);
    truncate_policy_name(&full_name)
}

/// Generate unique Sid with format IamPolicyAutopilot{Service}{Action}{YYYYMMDD}
/// Handles collision detection by appending counter (2, 3, etc.)
#[allow(unknown_lints, convert_case_pascal)]
pub fn build_statement_sid(action: &str, date: &str, existing_sids: &[String]) -> String {
    // Falls back to "Unknown" service if format is invalid
    let parts: Vec<&str> = action.split(':').collect();
    let (service, action_name) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("Unknown", action)
    };

    let service_cap = service.to_case(Case::Pascal);
    let action_cap = action_name.to_case(Case::Pascal);
    let date_no_hyphens = date.replace("-", "");

    let base_sid = format!(
        "{}{}{}{}",
        POLICY_PREFIX, service_cap, action_cap, date_no_hyphens
    );

    let mut sid = base_sid.clone();
    let mut counter = 2;
    while existing_sids.contains(&sid) {
        sid = format!("{}{}", base_sid, counter);
        counter += 1;
    }

    sid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_canonical_policy_name() {
        let name = build_canonical_policy_name(&PrincipalKind::Role, "MyLambdaRole");
        assert_eq!(name, "IamPolicyAutopilot-MyLambdaRole");
        assert!(name.len() <= MAX_POLICY_NAME_LENGTH);
    }

    #[test]
    fn test_build_canonical_policy_name_sanitization() {
        let name = build_canonical_policy_name(&PrincipalKind::Role, "My Lambda Role!");
        assert_eq!(name, "IamPolicyAutopilot-My-Lambda-Role");
    }

    #[test]
    fn test_build_canonical_policy_name_user() {
        let name = build_canonical_policy_name(&PrincipalKind::User, "john.doe");
        assert_eq!(name, "IamPolicyAutopilot-john.doe");
    }

    #[test]
    fn test_build_statement_sid_basic() {
        let existing_sids: Vec<String> = vec![];
        let sid = build_statement_sid("s3:GetObject", "20231215", &existing_sids);
        assert_eq!(sid, "IamPolicyAutopilotS3GetObject20231215");
    }

    #[test]
    fn test_build_statement_sid_collision() {
        let existing_sids = vec!["IamPolicyAutopilotS3GetObject20231215".to_string()];
        let sid = build_statement_sid("s3:GetObject", "20231215", &existing_sids);
        assert_eq!(sid, "IamPolicyAutopilotS3GetObject202312152");
    }

    #[test]
    fn test_build_statement_sid_multiple_collisions() {
        let existing_sids = vec![
            "IamPolicyAutopilotS3GetObject20231215".to_string(),
            "IamPolicyAutopilotS3GetObject202312152".to_string(),
            "IamPolicyAutopilotS3GetObject202312153".to_string(),
        ];
        let sid = build_statement_sid("s3:GetObject", "20231215", &existing_sids);
        assert_eq!(sid, "IamPolicyAutopilotS3GetObject202312154");
    }

    #[test]
    fn test_build_statement_sid_dynamodb() {
        let existing_sids: Vec<String> = vec![];
        let sid = build_statement_sid("dynamodb:GetItem", "20231215", &existing_sids);
        assert_eq!(sid, "IamPolicyAutopilotDynamodbGetItem20231215");
    }
}
