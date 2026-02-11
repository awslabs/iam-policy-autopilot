//! Core type definitions for IAM Policy Autopilot (pure Rust)

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Classification of denial type inferred from message context
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DenialType {
    ImplicitIdentity,
    ExplicitIdentity,
    ResourcePolicy,
    Other,
}

/// Parsed denial tuple extracted from an AccessDenied message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct ParsedDenial {
    pub principal_arn: String,
    pub action: String,
    pub resource: String,
    pub denial_type: DenialType,
}

impl ParsedDenial {
    #[must_use]
    pub fn new(
        principal_arn: String,
        action: String,
        resource: String,
        denial_type: DenialType,
    ) -> Self {
        Self {
            principal_arn,
            action,
            resource,
            denial_type,
        }
    }
}

/// Policy statement structure
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Statement {
    pub sid: String,
    pub effect: String,
    pub action: ActionType,
    pub resource: String,
}

/// Action can be a single string or list of strings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ActionType {
    Single(String),
    Multiple(Vec<String>),
}

impl ActionType {
    pub fn as_string(&self) -> String {
        match self {
            Self::Single(value) => value.clone(),
            Self::Multiple(values) => values.join(","),
        }
    }
}

/// Policy document structure
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct PolicyDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub version: String,
    pub statement: Vec<Statement>,
}

/// Key for statement deduplication based on (Effect, Action, Resource)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StatementKey {
    pub effect: String,
    pub action: String,
    pub resource: String,
}

/// Metadata about existing IAM policies
#[derive(Debug, Clone)]
pub struct PolicyMetadata {
    pub name: String,
    pub document: PolicyDocument,
}

/// Represents a complete execution plan with all metadata (replaces CLI's Plan type)
#[derive(Debug, Clone)]
pub struct PlanResult {
    pub diagnosis: ParsedDenial,
    pub actions: Vec<String>,
    pub policy: PolicyDocument,
}

/// Configuration options for apply operations
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub skip_confirmation: bool,
    pub skip_tty_check: bool,
}

/// Result of an apply operation with detailed metadata
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub success: bool,
    pub policy_name: String,
    pub principal_kind: String,
    pub principal_name: String,
    pub is_new_policy: bool,
    pub statement_count: usize,
    pub error: Option<String>,
}

/// Detailed error types for apply operation failures
#[derive(Error, Debug)]
pub enum ApplyError {
    #[error("Only ImplicitIdentity denials can be fixed")]
    UnsupportedDenialType,

    #[error("Principal type not supported: {0}")]
    UnsupportedPrincipal(String),

    #[error("Account mismatch: principal in {principal_account}, caller in {caller_account}")]
    AccountMismatch {
        principal_account: String,
        caller_account: String,
    },

    #[error("Duplicate statement: {action} on {resource}")]
    DuplicateStatement { action: String, resource: String },

    #[error("Expected exactly 1 action, got {0}")]
    MultiActionError(usize),

    #[error("AWS error: {0}")]
    Aws(#[from] crate::aws::AwsError),
}

/// Result type for apply operations
pub type ApplyResultWithError = Result<ApplyResult, ApplyError>;

impl Statement {
    /// Extract deduplication key from statement
    pub fn to_key(&self) -> StatementKey {
        StatementKey {
            effect: self.effect.clone(),
            action: self.action.as_string(),
            resource: self.resource.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::aws::policy_naming::POLICY_PREFIX;

    use super::*;

    #[test]
    fn test_statement_to_key() {
        let stmt = Statement {
            sid: "TestSid".to_string(),
            effect: "Allow".to_string(),
            action: ActionType::Single("s3:GetObject".to_string()),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let key = stmt.to_key();
        assert_eq!(key.effect, "Allow");
        assert_eq!(key.action, "s3:GetObject");
        assert_eq!(key.resource, "arn:aws:s3:::my-bucket/*");
    }

    #[test]
    fn test_statement_to_key_multiple_actions() {
        let stmt = Statement {
            sid: "TestSid".to_string(),
            effect: "Allow".to_string(),
            action: ActionType::Multiple(vec![
                "s3:GetObject".to_string(),
                "s3:PutObject".to_string(),
            ]),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let key = stmt.to_key();
        assert_eq!(key.effect, "Allow");
        assert_eq!(key.action, "s3:GetObject,s3:PutObject");
        assert_eq!(key.resource, "arn:aws:s3:::my-bucket/*");
    }

    #[test]
    fn test_statement_key_equality() {
        let key1 = StatementKey {
            effect: "Allow".to_string(),
            action: "s3:GetObject".to_string(),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let key2 = StatementKey {
            effect: "Allow".to_string(),
            action: "s3:GetObject".to_string(),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let key3 = StatementKey {
            effect: "Allow".to_string(),
            action: "s3:PutObject".to_string(),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_statement_key_hash() {
        use std::collections::HashSet;

        let key1 = StatementKey {
            effect: "Allow".to_string(),
            action: "s3:GetObject".to_string(),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let key2 = StatementKey {
            effect: "Allow".to_string(),
            action: "s3:GetObject".to_string(),
            resource: "arn:aws:s3:::my-bucket/*".to_string(),
        };

        let mut set = HashSet::new();
        set.insert(key1.clone());
        set.insert(key2);

        // Should only have one entry since keys are equal
        assert_eq!(set.len(), 1);
        assert!(set.contains(&key1));
    }

    #[test]
    fn test_plan_result_creation() {
        let plan = PlanResult {
            diagnosis: ParsedDenial::new(
                "arn:aws:iam::123456789012:role/test".to_string(),
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                DenialType::ImplicitIdentity,
            ),
            actions: vec!["s3:GetObject".to_string()],
            policy: PolicyDocument {
                id: Some(POLICY_PREFIX.to_string()),
                version: "2012-10-17".to_string(),
                statement: vec![],
            },
        };

        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.diagnosis.action, "s3:GetObject");
    }

    #[test]
    fn test_apply_options_default() {
        let options = ApplyOptions::default();
        assert!(!options.skip_confirmation);
        assert!(!options.skip_tty_check);
    }

    #[test]
    fn test_apply_result_success() {
        let result = ApplyResult {
            success: true,
            policy_name: "TestPolicy".to_string(),
            principal_kind: "Role".to_string(),
            principal_name: "TestRole".to_string(),
            is_new_policy: true,
            statement_count: 1,
            error: None,
        };

        assert!(result.success);
        assert!(result.is_new_policy);
        assert_eq!(result.statement_count, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_apply_error_types() {
        let err = ApplyError::UnsupportedDenialType;
        assert_eq!(
            err.to_string(),
            "Only ImplicitIdentity denials can be fixed"
        );

        let err = ApplyError::UnsupportedPrincipal("root".to_string());
        assert!(err.to_string().contains("not supported"));

        let err = ApplyError::AccountMismatch {
            principal_account: "123456789012".to_string(),
            caller_account: "987654321098".to_string(),
        };
        assert!(err.to_string().contains("mismatch"));

        let err = ApplyError::DuplicateStatement {
            action: "s3:GetObject".to_string(),
            resource: "arn:aws:s3:::bucket/*".to_string(),
        };
        assert!(err.to_string().contains("Duplicate"));

        let err = ApplyError::MultiActionError(2);
        assert!(err.to_string().contains("Expected exactly 1"));
    }

    #[test]
    fn test_parsed_denial_serialization() {
        let parsed_denial = ParsedDenial::new(
            "arn:aws:iam::123456789012:user/testuser".to_string(),
            "s3:GetObject".to_string(),
            "arn:aws:s3:::my-bucket/my-key".to_string(),
            DenialType::ImplicitIdentity,
        );

        let json = serde_json::to_string(&parsed_denial).unwrap();

        // Verify PascalCase field names
        assert!(json.contains("\"PrincipalArn\""));
        assert!(json.contains("\"Action\""));
        assert!(json.contains("\"Resource\""));
        assert!(json.contains("\"DenialType\""));
    }
}
