//! Realistic integration test: exercises the full pipeline
//! build_index() → greedy_set_cover() using real downloaded data.
//!
//! No network calls are made during the test — all data is stored as
//! static fixture files in tests/fixtures/realistic/.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use rstest::rstest;
use serde::Deserialize;

use iac_benchmarker::managed_policies::RawManagedPolicy;
use iac_benchmarker::policy_index::build_index;
use iac_benchmarker::service_ref::ServiceCatalogue;
use iac_benchmarker::set_cover::greedy_set_cover;

// ── Fixture deserialization types ────────────────────────────────────────────

/// Matches the `CatalogueCache` format written by `load_or_fetch_catalogue()`.
#[derive(Deserialize)]
struct CatalogueCacheFile {
    #[allow(dead_code)]
    fetched_at: u64,
    catalogue: ServiceCatalogue,
}

/// Matches the `PolicyCache` format written by `load_or_fetch_managed_policies()`.
#[derive(Deserialize)]
struct PolicyCacheFile {
    #[allow(dead_code)]
    fetched_at: u64,
    policies: Vec<RawManagedPolicy>,
}

#[derive(Deserialize)]
struct ScenarioFixture {
    description: String,
    required_actions: Vec<String>,
    expected_selected_policy_names: Vec<String>,
    expected_covered_actions: Vec<String>,
    expected_uncovered_actions: Vec<String>,
}

// ── Shared fixture loading ────────────────────────────────────────────────────

fn load_catalogue() -> ServiceCatalogue {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/realistic/service_catalogue_cache.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read catalogue fixture {:?}: {}", path, e));
    let cache: CatalogueCacheFile = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse catalogue fixture: {}", e));
    cache.catalogue
}

fn load_policies() -> Vec<RawManagedPolicy> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/realistic/managed_policy_cache.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read policy fixture {:?}: {}", path, e));
    let cache: PolicyCacheFile = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse policy fixture: {}", e));
    cache.policies
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[rstest]
fn test_realistic_set_cover(
    #[files("tests/fixtures/realistic/scenarios/*.json")] path: PathBuf,
) {
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
    let fixture: ScenarioFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {:?}: {}", path, e));

    // Build the full pipeline from real data.
    let catalogue = load_catalogue();
    let policies = load_policies();
    let index = build_index(&policies, &catalogue, "test_hash");

    // Collect all candidate ARNs from the index.
    let candidates: Vec<String> = index.policy_arn_to_name.keys().cloned().collect();

    let required: HashSet<String> = fixture.required_actions.into_iter().collect();
    let result = greedy_set_cover(&index, &candidates, &required);

    // Assert selected policy NAMES (not ARNs, for readability).
    let mut got_names: Vec<String> = result
        .selected_arns
        .iter()
        .map(|arn| {
            index
                .policy_arn_to_name
                .get(arn)
                .cloned()
                .unwrap_or_else(|| arn.clone())
        })
        .collect();
    got_names.sort();
    let mut exp_names = fixture.expected_selected_policy_names.clone();
    exp_names.sort();
    assert_eq!(
        got_names, exp_names,
        "fixture {:?} ({}): selected policy names mismatch",
        path.file_name().unwrap(), fixture.description
    );

    // Assert covered actions.
    let mut got_covered: Vec<String> = result.covered_actions.into_iter().collect();
    got_covered.sort();
    let mut exp_covered = fixture.expected_covered_actions.clone();
    exp_covered.sort();
    assert_eq!(
        got_covered, exp_covered,
        "fixture {:?} ({}): covered_actions mismatch",
        path.file_name().unwrap(), fixture.description
    );

    // Assert uncovered actions.
    let mut got_uncovered: Vec<String> = result.uncovered_actions.into_iter().collect();
    got_uncovered.sort();
    let mut exp_uncovered = fixture.expected_uncovered_actions.clone();
    exp_uncovered.sort();
    assert_eq!(
        got_uncovered, exp_uncovered,
        "fixture {:?} ({}): uncovered_actions mismatch",
        path.file_name().unwrap(), fixture.description
    );
}

// Suppress unused import warning for HashMap (used implicitly via ServiceCatalogue deserialization)
#[allow(dead_code)]
fn _use_hashmap(_: HashMap<String, String>) {}
