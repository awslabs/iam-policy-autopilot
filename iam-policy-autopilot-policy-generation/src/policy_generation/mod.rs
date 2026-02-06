//! Policy Generation module for creating IAM policies from enriched method calls
//!
//! This module provides functionality to generate AWS IAM policies from enriched SDK method calls.
//! Each EnrichedSdkMethodCall produces one IAM policy, with each Action becoming a separate statement.
//! ARN patterns are processed to replace placeholder variables with actual values or wildcards.

use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;

pub(crate) mod engine;
pub(crate) mod merge;
pub(crate) mod utils;

#[cfg(test)]
mod integration_tests;

pub use engine::Engine;

use crate::enrichment::Condition;

/// Custom serializer for IAM policy conditions
/// Converts Vec<Condition> to the proper IAM policy condition format
fn serialize_conditions<S>(conditions: &Vec<Condition>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut condition_map = HashMap::new();

    for condition in conditions {
        let operator_str = match condition.operator {
            crate::enrichment::Operator::StringEquals => "StringEquals",
            crate::enrichment::Operator::StringLike => "StringLike",
        };

        let operator_conditions = condition_map
            .entry(operator_str)
            .or_insert_with(HashMap::new);

        operator_conditions.insert(&condition.key, &condition.values);
    }

    condition_map.serialize(serializer)
}

/// Represents a complete IAM policy document
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct IamPolicy {
    /// Policy ID
    #[serde(rename = "Id")]
    pub(crate) id: String,
    /// Policy language version (typically "2012-10-17")
    #[serde(rename = "Version")]
    pub(crate) version: String,
    /// List of policy statements
    #[serde(rename = "Statement")]
    pub(crate) statements: Vec<Statement>,
}

/// Represents an individual IAM policy statement
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Statement {
    /// Optional statement identifier
    #[serde(rename = "Sid", skip_serializing_if = "Option::is_none")]
    pub(crate) sid: Option<String>,
    /// Effect of the statement (Allow or Deny)
    #[serde(rename = "Effect")]
    pub(crate) effect: Effect,
    /// List of IAM actions this statement applies to
    #[serde(rename = "Action")]
    pub(crate) action: Vec<String>,
    /// List of resources this statement applies to
    #[serde(rename = "Resource")]
    pub(crate) resource: Vec<String>,
    /// List of conditions for the statement
    /// Simplified type (from Option<Vec<Condition>>), but we don't need to deserialize into it.
    #[serde(
        rename = "Condition",
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_conditions"
    )]
    pub(crate) condition: Vec<Condition>,
}

/// Effect of an IAM policy statement
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Effect {
    /// Allow access
    Allow,
    /// Deny access
    Deny,
}

/// Policy type enumeration
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PolicyType {
    /// Identity-based policy (attached to users, groups, or roles)
    #[default]
    Identity,
}

/// A policy with its associated type information
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct PolicyWithMetadata {
    /// The IAM policy document as a JSON object
    pub policy: IamPolicy,
    /// Type of the policy
    pub policy_type: PolicyType,
}

impl IamPolicy {
    /// Create a new IAM policy with the standard version
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: "IamPolicyAutopilot".to_string(),
            version: "2012-10-17".to_string(),
            statements: Vec::new(),
        }
    }

    /// Add a statement to the policy
    pub fn add_statement(&mut self, statement: Statement) {
        self.statements.push(statement);
    }
}

impl Default for IamPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl Statement {
    /// Create a new IAM policy statement
    #[must_use]
    pub fn new(effect: Effect, action: Vec<String>, resource: Vec<String>) -> Self {
        Self {
            sid: None,
            effect,
            action,
            resource,
            condition: vec![],
        }
    }

    /// Create a new Allow statement
    #[must_use]
    pub fn allow(action: Vec<String>, resource: Vec<String>) -> Self {
        Self::new(Effect::Allow, action, resource)
    }

    /// Set the condition
    pub(crate) fn with_conditions(mut self, condition: Vec<Condition>) -> Self {
        self.condition = condition;
        self
    }

    /// Set the statement ID
    pub(crate) fn with_sid(mut self, sid: String) -> Self {
        self.sid = Some(sid);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iam_policy_creation() {
        let policy = IamPolicy::new();
        assert_eq!(policy.version, "2012-10-17");
        assert_eq!(policy.id, "IamPolicyAutopilot");
        assert_eq!(policy.statements.len(), 0);
    }

    #[test]
    fn test_statement_creation() {
        let statement = Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::bucket/*".to_string()],
        );

        assert_eq!(statement.effect, Effect::Allow);
        assert_eq!(statement.action, vec!["s3:GetObject"]);
        assert_eq!(statement.resource, vec!["arn:aws:s3:::bucket/*"]);
        assert!(statement.sid.is_none());
        assert!(statement.condition.is_empty());
    }

    #[test]
    fn test_statement_with_sid() {
        let statement = Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::bucket/*".to_string()],
        )
        .with_sid("AllowS3GetObject".to_string());

        assert_eq!(statement.sid, Some("AllowS3GetObject".to_string()));
    }

    #[test]
    fn test_policy_serialization() {
        let mut policy = IamPolicy::new();
        policy.add_statement(Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::bucket/*".to_string()],
        ));

        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("\"Version\":\"2012-10-17\""));
        assert!(json.contains("\"Effect\":\"Allow\""));
        assert!(json.contains("\"Action\":[\"s3:GetObject\"]"));
    }

    #[test]
    fn test_condition_serialization() {
        use crate::enrichment::{Condition, Operator};

        // Create a condition
        let condition = Condition {
            operator: Operator::StringEquals,
            key: "aws:RequestedRegion".to_string(),
            values: vec!["us-east-1".to_string(), "us-west-2".to_string()],
        };

        let statement = Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::bucket/*".to_string()],
        )
        .with_conditions(vec![condition]);

        let json = serde_json::to_string(&statement).unwrap();

        // Verify the condition is serialized in the correct IAM format
        assert!(json.contains("\"Condition\":{\"StringEquals\":{\"aws:RequestedRegion\":[\"us-east-1\",\"us-west-2\"]}}"));
    }

    #[test]
    fn test_stringlike_condition_serialization() {
        use crate::enrichment::{Condition, Operator};

        // Create a condition with StringLike operator
        let condition = Condition {
            operator: Operator::StringLike,
            key: "s3:ExistingObjectTag/Environment".to_string(),
            values: vec!["production-*".to_string(), "staging-*".to_string()],
        };

        let statement = Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        )
        .with_conditions(vec![condition]);

        let mut policy = IamPolicy::new();
        policy.add_statement(statement);
        let json = serde_json::to_string(&policy).unwrap();

        // Verify the condition is serialized with StringLike operator
        assert!(json.contains("\"Condition\":{\"StringLike\":{\"s3:ExistingObjectTag/Environment\":[\"production-*\",\"staging-*\"]}}"));
    }

    #[test]
    fn test_policy_with_metadata_serialization() {
        let mut policy = IamPolicy::new();
        policy.add_statement(Statement::allow(
            vec!["s3:GetObject".to_string()],
            vec!["arn:aws:s3:::bucket/*".to_string()],
        ));

        let policy_with_metadata = PolicyWithMetadata {
            policy,
            policy_type: PolicyType::Identity,
        };

        let json = serde_json::to_string(&policy_with_metadata).unwrap();

        // Verify PascalCase field names and PolicyType serialization
        assert!(json.contains("\"Policy\":"));
        assert!(json.contains("\"PolicyType\":\"Identity\""));
        assert!(json.contains("\"Version\":\"2012-10-17\""));
    }
}
