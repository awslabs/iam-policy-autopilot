use std::path::PathBuf;
use rstest::rstest;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct ExtractFixture {
    description: String,
    policy_document: Value,
    expected_patterns: Vec<String>,
}

#[rstest]
fn test_extract_allow_patterns_from_fixture(
    #[files("tests/fixtures/extract_allow_patterns/*.json")] path: PathBuf,
) {
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
    let fixture: ExtractFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", path, e));
    let mut result = iac_benchmarker::managed_policies::extract_allow_patterns(&fixture.policy_document);
    result.sort();
    let mut expected = fixture.expected_patterns.clone();
    expected.sort();
    assert_eq!(
        result,
        expected,
        "fixture {:?} ({}): extract_allow_patterns mismatch",
        path.file_name().unwrap(),
        fixture.description
    );
}
