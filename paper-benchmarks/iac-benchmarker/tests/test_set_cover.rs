use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use rstest::rstest;
use serde::Deserialize;
use iac_benchmarker::managed_policies::AllowStatement;
use iac_benchmarker::policy_index::PolicyIndex;
use iac_benchmarker::set_cover::greedy_set_cover;

#[derive(Deserialize)]
struct IndexData {
    policy_arn_to_allow_patterns: HashMap<String, Vec<String>>,
    policy_arn_to_name: HashMap<String, String>,
    policy_arn_to_concrete_action_count: HashMap<String, u32>,
    service_prefix_to_policy_arns: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct SetCoverFixture {
    description: String,
    index: IndexData,
    candidates: Vec<String>,
    required_actions: Vec<String>,
    expected_selected_arns: Vec<String>,
    expected_covered_actions: Vec<String>,
    expected_uncovered_actions: Vec<String>,
}

#[rstest]
fn test_greedy_set_cover_from_fixture(
    #[files("tests/fixtures/set_cover/*.json")] path: PathBuf,
) {
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
    let fixture: SetCoverFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", path, e));

    // Derive allow statements from the allow patterns (Resource: "*" for all,
    // since fixture data does not carry resource information).
    let policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>> = fixture
        .index
        .policy_arn_to_allow_patterns
        .iter()
        .map(|(arn, patterns)| {
            let stmts: Vec<AllowStatement> = patterns
                .iter()
                .map(|p| AllowStatement {
                    action_patterns: vec![p.clone()],
                    resource_patterns: vec!["*".to_string()],
                })
                .collect();
            (arn.clone(), stmts)
        })
        .collect();

    let index = PolicyIndex {
        built_at: "2026-01-01T00:00:00Z".to_string(),
        policy_cache_hash: "test".to_string(),
        service_prefix_to_policy_arns: fixture.index.service_prefix_to_policy_arns,
        policy_arn_to_allow_patterns: fixture.index.policy_arn_to_allow_patterns,
        policy_arn_to_concrete_action_count: fixture.index.policy_arn_to_concrete_action_count,
        policy_arn_to_name: fixture.index.policy_arn_to_name,
        policy_arn_to_deny_action_patterns: HashMap::new(),
        policy_arn_to_deny_not_action_patterns: HashMap::new(),
        policy_arn_to_allow_statements,
    };

    let required: HashSet<String> = fixture.required_actions.into_iter().collect();
    let result = greedy_set_cover(&index, &fixture.candidates, &required);

    let mut got_selected = result.selected_arns.clone();
    got_selected.sort();
    let mut exp_selected = fixture.expected_selected_arns.clone();
    exp_selected.sort();
    assert_eq!(
        got_selected,
        exp_selected,
        "fixture {:?} ({}): selected_arns mismatch",
        path.file_name().unwrap(),
        fixture.description
    );

    let mut got_covered: Vec<String> = result.covered_actions.into_iter().collect();
    got_covered.sort();
    let mut exp_covered = fixture.expected_covered_actions.clone();
    exp_covered.sort();
    assert_eq!(
        got_covered,
        exp_covered,
        "fixture {:?} ({}): covered_actions mismatch",
        path.file_name().unwrap(),
        fixture.description
    );

    let mut got_uncovered: Vec<String> = result.uncovered_actions.into_iter().collect();
    got_uncovered.sort();
    let mut exp_uncovered = fixture.expected_uncovered_actions.clone();
    exp_uncovered.sort();
    assert_eq!(
        got_uncovered,
        exp_uncovered,
        "fixture {:?} ({}): uncovered_actions mismatch",
        path.file_name().unwrap(),
        fixture.description
    );
}
