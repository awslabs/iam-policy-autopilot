//! Declarative tests for the policy minimizer (ddmin algorithm).
//!
//! Uses `rstest` parametrized cases with a `MinimizeCase` struct so each test
//! case reads as a self-documenting spec with named fields:
//! - **policies**: input policy documents
//! - **required**: which actions the oracle requires (empty = always-pass)
//! - **expect_kept**: actions that must survive minimization
//! - **expect_removed**: actions that must be eliminated

use super::*;
use rstest::rstest;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ===========================================================================
// Test case struct
// ===========================================================================

/// A declarative minimization test case with named fields.
struct MinimizeCase {
    /// Input policy documents to minimize.
    policies: Vec<Value>,
    /// Actions the oracle requires to pass (empty = always-pass oracle).
    required: Vec<&'static str>,
    /// Actions that must be present in the minimized result.
    expect_kept: Vec<&'static str>,
    /// Actions that must NOT be present in the minimized result.
    expect_removed: Vec<&'static str>,
}

// ===========================================================================
// Test helpers & fixtures
// ===========================================================================

/// Build a single-statement policy document with the given actions and resource.
fn policy(actions: &[&str], resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": resource
        }]
    })
}

/// Build a policy document with multiple statements (one action each).
fn multi_stmt_policy(action_resource_pairs: &[(&str, &str)]) -> Value {
    let stmts: Vec<Value> = action_resource_pairs
        .iter()
        .map(|(action, resource)| {
            json!({
                "Effect": "Allow",
                "Action": [action],
                "Resource": resource,
            })
        })
        .collect();
    json!({
        "Version": "2012-10-17",
        "Statement": stmts,
    })
}

/// Build a policy document with explicit Sid values on each statement.
fn policy_with_sids(stmts: &[(&str, &str, &str)]) -> Value {
    let statements: Vec<Value> = stmts
        .iter()
        .map(|(sid, action, resource)| {
            json!({
                "Sid": sid,
                "Effect": "Allow",
                "Action": [action],
                "Resource": resource,
            })
        })
        .collect();
    json!({
        "Version": "2012-10-17",
        "Statement": statements,
    })
}

// ---------------------------------------------------------------------------
// Oracle (run_fn) factories
// ---------------------------------------------------------------------------

type RunFn =
    dyn Fn(Vec<Value>) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>>>> + Send + Sync;

/// Oracle that succeeds only when ALL of the specified actions are present.
/// If `required` is empty, always passes.
fn oracle_require(required: &[&str]) -> Arc<RunFn> {
    let required: Vec<String> = required.iter().map(|s| (*s).to_string()).collect();
    let required = Arc::new(required);
    Arc::new(move |policies: Vec<Value>| {
        let required = required.clone();
        Box::pin(async move {
            if required.is_empty() {
                return Ok(true);
            }
            let actions = collect_actions(&policies);
            Ok(required.iter().all(|r| actions.contains(r)))
        })
    })
}

/// Collect all action strings from a slice of policy documents.
fn collect_actions(policies: &[Value]) -> Vec<String> {
    policies
        .iter()
        .flat_map(|p| {
            p.get("Statement")
                .and_then(|s| s.as_array())
                .into_iter()
                .flatten()
                .flat_map(|stmt| match stmt.get("Action") {
                    Some(Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>(),
                    Some(Value::String(s)) => vec![s.clone()],
                    _ => vec![],
                })
        })
        .collect()
}

/// Extract all actions from a `MinimizationResult`.
fn result_actions(result: &MinimizationResult) -> Vec<String> {
    result.minimal_policy["Statement"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|stmt| match &stmt["Action"] {
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>(),
            Value::String(s) => vec![s.clone()],
            _ => vec![],
        })
        .collect()
}

/// Run the minimizer with the given case and assert kept/removed actions.
async fn run_and_assert(case: MinimizeCase) {
    let oracle = oracle_require(&case.required);
    let run_fn = {
        let oracle = oracle.clone();
        move |p: Vec<Value>| {
            let oracle = oracle.clone();
            Box::pin(async move { oracle(p).await })
                as Pin<Box<dyn Future<Output = anyhow::Result<bool>>>>
        }
    };

    let result = minimize_policy(case.policies, run_fn).await;
    let actions = result_actions(&result);

    for kept in &case.expect_kept {
        assert!(
            actions.contains(&kept.to_string()),
            "expected action {:?} to be KEPT but it was removed.\nFinal actions: {:?}",
            kept,
            actions
        );
    }

    for removed in &case.expect_removed {
        assert!(
            !actions.contains(&removed.to_string()),
            "expected action {:?} to be REMOVED but it survived.\nFinal actions: {:?}",
            removed,
            actions
        );
    }
}

// ===========================================================================
// Document-level (pass 1) tests
// ===========================================================================

#[rstest]
#[case::all_removable_keeps_last(MinimizeCase {
    policies: vec![
        policy(&["s3:GetObject"], "*"),
        policy(&["s3:PutObject"], "*"),
        policy(&["s3:DeleteObject"], "*"),
    ],
    required: vec![],
    expect_kept: vec!["s3:DeleteObject"],
    expect_removed: vec!["s3:GetObject", "s3:PutObject"],
})]
#[case::all_necessary_keeps_everything(MinimizeCase {
    policies: vec![
        policy(&["s3:GetObject"], "*"),
        policy(&["s3:PutObject"], "*"),
        policy(&["s3:DeleteObject"], "*"),
    ],
    required: vec!["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
    expect_kept: vec!["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
    expect_removed: vec![],
})]
#[case::only_first_necessary(MinimizeCase {
    policies: vec![
        policy(&["s3:GetObject"], "*"),
        policy(&["s3:PutObject"], "*"),
        policy(&["s3:DeleteObject"], "*"),
    ],
    required: vec!["s3:GetObject"],
    expect_kept: vec!["s3:GetObject"],
    expect_removed: vec!["s3:PutObject", "s3:DeleteObject"],
})]
#[case::two_of_four_necessary(MinimizeCase {
    policies: vec![
        policy(&["s3:GetObject"], "*"),
        policy(&["s3:PutObject"], "*"),
        policy(&["s3:DeleteObject"], "*"),
        policy(&["s3:ListBucket"], "*"),
    ],
    required: vec!["s3:GetObject", "s3:PutObject"],
    expect_kept: vec!["s3:GetObject", "s3:PutObject"],
    expect_removed: vec!["s3:DeleteObject", "s3:ListBucket"],
})]
#[case::single_necessary_policy(MinimizeCase {
    policies: vec![policy(&["s3:GetObject"], "*")],
    required: vec!["s3:GetObject"],
    expect_kept: vec!["s3:GetObject"],
    expect_removed: vec![],
})]
#[case::single_policy_with_always_pass(MinimizeCase {
    policies: vec![policy(&["s3:GetObject"], "*")],
    required: vec![],
    expect_kept: vec!["s3:GetObject"],
    expect_removed: vec![],
})]
#[case::last_policy_is_the_only_necessary_one(MinimizeCase {
    policies: vec![
        policy(&["s3:PutObject"], "*"),
        policy(&["s3:DeleteObject"], "*"),
        policy(&["s3:ListBucket"], "*"),
        policy(&["s3:GetObject"], "*"),
    ],
    required: vec!["s3:GetObject"],
    expect_kept: vec!["s3:GetObject"],
    expect_removed: vec!["s3:PutObject", "s3:DeleteObject", "s3:ListBucket"],
})]
#[tokio::test]
async fn document_level_minimization(#[case] case: MinimizeCase) {
    run_and_assert(case).await;
}

// ===========================================================================
// Statement-level (pass 2) tests
// ===========================================================================

#[rstest]
#[case::removes_spurious_statement_in_same_doc(MinimizeCase {
    policies: vec![
        multi_stmt_policy(&[
            ("s3:PutObject", "*"),
            ("s3-object-lambda:PutObject", "*"),
        ]),
    ],
    required: vec!["s3:PutObject"],
    expect_kept: vec!["s3:PutObject"],
    expect_removed: vec!["s3-object-lambda:PutObject"],
})]
#[case::keeps_all_statements_when_all_required(MinimizeCase {
    policies: vec![
        multi_stmt_policy(&[
            ("s3:PutObject", "*"),
            ("cloudwatch:PutMetricData", "*"),
            ("states:StartExecution", "*"),
        ]),
    ],
    required: vec!["s3:PutObject", "cloudwatch:PutMetricData", "states:StartExecution"],
    expect_kept: vec!["s3:PutObject", "cloudwatch:PutMetricData", "states:StartExecution"],
    expect_removed: vec![],
})]
#[case::removes_multiple_spurious_statements(MinimizeCase {
    policies: vec![
        multi_stmt_policy(&[
            ("s3:PutObject", "*"),
            ("s3:PutObjectAcl", "*"),
            ("s3:PutObjectLegalHold", "*"),
            ("s3:PutObjectRetention", "*"),
            ("s3-object-lambda:PutObject", "*"),
        ]),
    ],
    required: vec!["s3:PutObject"],
    expect_kept: vec!["s3:PutObject"],
    expect_removed: vec![
        "s3:PutObjectAcl",
        "s3:PutObjectLegalHold",
        "s3:PutObjectRetention",
        "s3-object-lambda:PutObject",
    ],
})]
#[case::across_multiple_docs_with_spurious_stmts(MinimizeCase {
    policies: vec![
        multi_stmt_policy(&[
            ("s3:PutObject", "*"),
            ("s3-object-lambda:PutObject", "*"),
        ]),
        multi_stmt_policy(&[
            ("cloudwatch:PutMetricData", "*"),
            ("cloudwatch:ListMetrics", "*"),
        ]),
    ],
    required: vec!["s3:PutObject", "cloudwatch:PutMetricData"],
    expect_kept: vec!["s3:PutObject", "cloudwatch:PutMetricData"],
    expect_removed: vec!["s3-object-lambda:PutObject", "cloudwatch:ListMetrics"],
})]
#[tokio::test]
async fn statement_level_minimization(#[case] case: MinimizeCase) {
    run_and_assert(case).await;
}

// ===========================================================================
// Merge / Sid-stripping tests
// ===========================================================================

#[test]
fn merge_policies_strips_all_sids() {
    let doc1 = policy_with_sids(&[("AllowS3", "s3:GetObject", "*")]);
    let doc2 = policy_with_sids(&[("AllowS3", "s3:PutObject", "*")]); // duplicate Sid

    let merged = super::merge_policies(&[doc1, doc2]);
    let stmts = merged["Statement"].as_array().unwrap();
    assert_eq!(stmts.len(), 2);
    for stmt in stmts {
        assert!(
            stmt.get("Sid").is_none(),
            "merge_policies must strip Sid fields; got: {stmt:?}",
        );
    }
}

#[rstest]
#[case::duplicate_sids_across_docs(MinimizeCase {
    policies: vec![
        policy_with_sids(&[
            ("AllowGlueGetPartitions", "glue:GetPartitions", "*"),
            ("AllowGlueGetDatabase", "glue:GetDatabase", "*"),
        ]),
        policy_with_sids(&[
            ("AllowGlueGetPartitions", "glue:GetPartitions", "*"),
            ("AllowS3GetObject", "s3:GetObject", "*"),
        ]),
    ],
    required: vec!["glue:GetPartitions"],
    expect_kept: vec!["glue:GetPartitions"],
    expect_removed: vec!["glue:GetDatabase", "s3:GetObject"],
})]
#[tokio::test]
async fn sid_handling(#[case] case: MinimizeCase) {
    run_and_assert(case).await;
}
