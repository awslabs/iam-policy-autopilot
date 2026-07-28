use std::collections::HashMap;
use std::path::PathBuf;
use rstest::rstest;
use serde::Deserialize;
use serde_json::Value;
use iac_benchmarker::managed_policies::RawManagedPolicy;
use iac_benchmarker::policy_index::build_index;
use iac_benchmarker::service_ref::ServiceCatalogue;

#[derive(Deserialize)]
struct BuildIndexFixture {
    description: String,
    policies: Vec<RawManagedPolicy>,
    catalogue: ServiceCatalogue,
    expected_service_prefixes: Vec<String>,
    expected_policy_arns_for_s3: Vec<String>,
    expected_concrete_action_count_for_arn: HashMap<String, u32>,
}

#[rstest]
fn test_build_index_from_fixture(
    #[files("tests/fixtures/build_index/*.json")] path: PathBuf,
) {
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
    let fixture: BuildIndexFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", path, e));

    let index = build_index(&fixture.policies, &fixture.catalogue, "test_hash");

    let mut got_prefixes: Vec<String> = index.service_prefix_to_policy_arns.keys().cloned().collect();
    got_prefixes.sort();
    let mut exp_prefixes = fixture.expected_service_prefixes.clone();
    exp_prefixes.sort();
    assert_eq!(
        got_prefixes,
        exp_prefixes,
        "fixture {:?} ({}): service_prefix keys mismatch",
        path.file_name().unwrap(),
        fixture.description
    );

    if !fixture.expected_policy_arns_for_s3.is_empty() {
        let mut got_s3 = index
            .service_prefix_to_policy_arns
            .get("s3")
            .cloned()
            .unwrap_or_default();
        got_s3.sort();
        let mut exp_s3 = fixture.expected_policy_arns_for_s3.clone();
        exp_s3.sort();
        assert_eq!(
            got_s3,
            exp_s3,
            "fixture {:?} ({}): s3 policy ARNs mismatch",
            path.file_name().unwrap(),
            fixture.description
        );
    }

    for (arn, expected_count) in &fixture.expected_concrete_action_count_for_arn {
        let got_count = index
            .policy_arn_to_concrete_action_count
            .get(arn)
            .copied()
            .unwrap_or(0);
        assert_eq!(
            got_count,
            *expected_count,
            "fixture {:?} ({}): concrete_action_count for {} mismatch",
            path.file_name().unwrap(),
            fixture.description,
            arn
        );
    }
}

// Suppress unused import warning for Value (used implicitly via RawManagedPolicy deserialization)
#[allow(dead_code)]
fn _use_value(_: Value) {}
