use std::path::PathBuf;
use rstest::rstest;
use serde::Deserialize;

#[derive(Deserialize)]
struct GlobFixture {
    pattern: String,
    text: String,
    expected: bool,
}

#[rstest]
fn test_glob_match_from_fixture(#[files("tests/fixtures/glob_match/*.json")] path: PathBuf) {
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
    let fixture: GlobFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", path, e));
    let result = iac_benchmarker::service_ref::glob_match(
        fixture.pattern.to_lowercase().as_bytes(),
        fixture.text.to_lowercase().as_bytes(),
    );
    assert_eq!(
        result,
        fixture.expected,
        "fixture {:?}: glob_match({:?}, {:?}) expected {} got {}",
        path.file_name().unwrap(),
        fixture.pattern,
        fixture.text,
        fixture.expected,
        result
    );
}
