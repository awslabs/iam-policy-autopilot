//! Policy document generation for IAM Policy Autopilot

use crate::types::{ActionType, PolicyDocument, Statement};
use std::collections::HashSet;

/// Build an inline IAM policy document with Allow effect, with deterministic
/// action ordering and deduplication.
#[must_use]
pub fn build_inline_allow(actions: Vec<String>, resource: String) -> PolicyDocument {
    let mut unique_actions: Vec<String> = actions
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    unique_actions.sort();

    let action_type = if unique_actions.len() == 1 {
        ActionType::Single(unique_actions[0].clone())
    } else {
        ActionType::Multiple(unique_actions)
    };

    let statement = Statement {
        sid: "IamPolicyAutopilotAllow".to_string(),
        effect: "Allow".to_string(),
        action: action_type,
        resource,
    };

    PolicyDocument {
        id: Some("IamPolicyAutopilot".to_string()),
        version: "2012-10-17".to_string(),
        statement: vec![statement],
    }
}

/// Merge existing statements with a new statement
/// Returns a combined list with deduplication
pub fn merge_statements(existing: Vec<Statement>, new: Statement) -> Vec<Statement> {
    let mut all_statements = existing;
    all_statements.push(new);
    deduplicate_statements(all_statements)
}

/// Deduplicate statements based on (Effect, Action, Resource) tuple
/// Returns deduplicated list with first occurrence preserved
pub fn deduplicate_statements(statements: Vec<Statement>) -> Vec<Statement> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for stmt in statements {
        let key = stmt.to_key();
        if seen.insert(key) {
            // insert returns true if the value was newly inserted
            result.push(stmt);
        }
    }

    result
}

/// Sort statements by Sid for consistent ordering
pub fn sort_statements(statements: &mut [Statement]) {
    statements.sort_by(|a, b| a.sid.cmp(&b.sid));
}

/// Build a single-action statement with proper Sid
/// This is used for canonical policy consolidation
#[must_use]
pub fn build_single_statement(action: String, resource: String, sid: String) -> Statement {
    Statement {
        sid,
        effect: "Allow".to_string(),
        action: ActionType::Single(action),
        resource,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_inline_allow_multiple_actions_sorted() {
        let policy = build_inline_allow(
            vec!["s3:PutObject".into(), "s3:GetObject".into()],
            "arn:aws:s3:::bucket/*".into(),
        );
        let stmt = &policy.statement[0];
        match &stmt.action {
            ActionType::Multiple(values) => {
                assert_eq!(values, &vec!["s3:GetObject", "s3:PutObject"])
            }
            _ => panic!("expected multiple"),
        }
        assert_eq!(policy.id, Some("IamPolicyAutopilot".to_string()));
    }

    #[test]
    fn test_build_single_statement() {
        let stmt = build_single_statement(
            "s3:GetObject".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
            "IamPolicyAutopilotS3GetObject20231215".to_string(),
        );
        assert_eq!(stmt.sid, "IamPolicyAutopilotS3GetObject20231215");
        assert_eq!(stmt.effect, "Allow");
        match stmt.action {
            ActionType::Single(value) => assert_eq!(value, "s3:GetObject"),
            _ => panic!("expected single action"),
        }
        assert_eq!(stmt.resource, "arn:aws:s3:::bucket/*");
    }

    #[test]
    fn test_merge_statements() {
        let existing = vec![build_single_statement(
            "s3:GetObject".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
            "Sid1".to_string(),
        )];
        let new = build_single_statement(
            "s3:PutObject".to_string(),
            "arn:aws:s3:::bucket/*".to_string(),
            "Sid2".to_string(),
        );

        let merged = merge_statements(existing, new);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_deduplicate_statements() {
        let statements = vec![
            build_single_statement(
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "Sid1".to_string(),
            ),
            build_single_statement(
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "Sid2".to_string(),
            ),
            build_single_statement(
                "s3:PutObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "Sid3".to_string(),
            ),
        ];

        let deduped = deduplicate_statements(statements);
        assert_eq!(deduped.len(), 2);
        // First occurrence should be preserved
        assert_eq!(deduped[0].sid, "Sid1");
        assert_eq!(deduped[1].sid, "Sid3");
    }

    #[test]
    fn test_sort_statements() {
        let mut statements = vec![
            build_single_statement(
                "s3:PutObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "ZZZ".to_string(),
            ),
            build_single_statement(
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "AAA".to_string(),
            ),
            build_single_statement(
                "s3:DeleteObject".to_string(),
                "arn:aws:s3:::bucket/*".to_string(),
                "MMM".to_string(),
            ),
        ];

        sort_statements(&mut statements);
        assert_eq!(statements[0].sid, "AAA");
        assert_eq!(statements[1].sid, "MMM");
        assert_eq!(statements[2].sid, "ZZZ");
    }

    #[test]
    fn test_deduplicate_statements_different_resources() {
        let statements = vec![
            build_single_statement(
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket1/*".to_string(),
                "Sid1".to_string(),
            ),
            build_single_statement(
                "s3:GetObject".to_string(),
                "arn:aws:s3:::bucket2/*".to_string(),
                "Sid2".to_string(),
            ),
        ];

        let deduped = deduplicate_statements(statements);
        // Different resources should not be deduplicated
        assert_eq!(deduped.len(), 2);
    }
}
