//! Pre-computed policy index: build once, load fast on every subsequent run.
//!
//! The index is invalidated (and rebuilt) whenever the SHA-256 of the managed
//! policy cache file changes, ensuring it always reflects the latest data.

use std::{
    collections::HashMap,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::managed_policies::{
    extract_unconditional_allow_statements, extract_deny_patterns, AllowStatement, RawManagedPolicy,
};
use crate::service_ref::{action_covered_by, count_matching_actions, ServiceCatalogue};

// ── Public types ─────────────────────────────────────────────────────────────

/// The pre-computed, persistable policy index.
///
/// Serialised to/from `policy_index.json`.  All expensive glob-expansion and
/// action-counting is done once at build time so the benchmarker can do O(1)
/// lookups at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyIndex {
    /// ISO-8601 timestamp of when the index was built.
    pub built_at: String,
    /// SHA-256 hex of the managed-policy cache file at build time.
    /// Used to detect staleness without re-reading the full cache.
    pub policy_cache_hash: String,
    /// service prefix (e.g. `"s3"`) → managed policy ARNs that allow ≥1
    /// action for that prefix.
    pub service_prefix_to_policy_arns: HashMap<String, Vec<String>>,
    /// managed policy ARN → list of Allow action patterns.
    pub policy_arn_to_allow_patterns: HashMap<String, Vec<String>>,
    /// managed policy ARN → pre-computed total concrete action count
    /// (sum across all allow patterns, expanded against the catalogue).
    pub policy_arn_to_concrete_action_count: HashMap<String, u32>,
    /// managed policy ARN → human-readable name.
    pub policy_arn_to_name: HashMap<String, String>,
    /// managed policy ARN → list of Deny `Action` patterns (from
    /// `Effect: Deny` + `Action: [...]` statements).
    ///
    /// An action is denied if it matches any of these patterns.
    pub policy_arn_to_deny_action_patterns: HashMap<String, Vec<String>>,
    /// managed policy ARN → list of Deny `NotAction` patterns (from
    /// `Effect: Deny` + `NotAction: [...]` statements).
    ///
    /// An action is denied if it does **not** match any of these patterns
    /// (i.e. it is not in the exception list).
    pub policy_arn_to_deny_not_action_patterns: HashMap<String, Vec<String>>,
    /// managed policy ARN → list of [`AllowStatement`] values, each carrying
    /// both action patterns and resource patterns.
    ///
    /// Used for resource-aware matching: when checking whether a managed policy
    /// covers a required action, callers can also verify that the statement's
    /// resource scope overlaps with the required resource ARNs for that action.
    pub policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>>,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Load the index from `index_path` if it is valid (exists and
/// `policy_cache_hash` matches the current SHA-256 of `policy_cache_path`),
/// otherwise build from `policies` + `catalogue`, save to `index_path`, and
/// return.
pub fn load_or_build_index(
    index_path: &Path,
    policy_cache_path: &Path,
    policies: &[RawManagedPolicy],
    catalogue: &ServiceCatalogue,
) -> Result<PolicyIndex> {
    // Compute the current hash of the policy cache file.
    let current_hash = file_sha256(policy_cache_path)
        .unwrap_or_else(|_| String::new());

    // Try loading from disk.
    if let Ok(raw) = std::fs::read(index_path) {
        if let Ok(index) = serde_json::from_slice::<PolicyIndex>(&raw) {
            if !current_hash.is_empty() && index.policy_cache_hash == current_hash {
                tracing::info!(
                    "[policy_index] Loaded valid index from {:?} ({} policies)",
                    index_path,
                    index.policy_arn_to_name.len()
                );
                return Ok(index);
            }
            tracing::info!("[policy_index] Index hash mismatch — rebuilding ...");
        }
    }

    // Build from scratch.
    let index = build_index(policies, catalogue, &current_hash);

    // Persist.
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(&index)?;
    std::fs::write(index_path, json)
        .with_context(|| format!("Failed to write policy index to {:?}", index_path))?;
    tracing::info!(
        "[policy_index] Built and saved index ({} policies, {} service prefixes) to {:?}",
        index.policy_arn_to_name.len(),
        index.service_prefix_to_policy_arns.len(),
        index_path
    );

    Ok(index)
}

/// Build the index from scratch.
///
/// For each policy:
/// 1. Extract unconditional Allow statements via
///    [`extract_unconditional_allow_statements`] (conditional Allows are
///    skipped — they may not fire at runtime).
/// 2. For each action pattern, extract the service prefix and add the policy
///    ARN to `service_prefix_to_policy_arns[prefix]`.
/// 3. For each action pattern, count matching concrete actions via
///    [`count_matching_actions`] and accumulate into
///    `policy_arn_to_concrete_action_count`.
/// 4. Extract Deny patterns via [`extract_deny_patterns`] and store in
///    `policy_arn_to_deny_action_patterns` and
///    `policy_arn_to_deny_not_action_patterns`.
pub fn build_index(
    policies: &[RawManagedPolicy],
    catalogue: &ServiceCatalogue,
    policy_cache_hash: &str,
) -> PolicyIndex {
    let mut service_prefix_to_policy_arns: HashMap<String, Vec<String>> = HashMap::new();
    let mut policy_arn_to_allow_patterns: HashMap<String, Vec<String>> = HashMap::new();
    let mut policy_arn_to_concrete_action_count: HashMap<String, u32> = HashMap::new();
    let mut policy_arn_to_name: HashMap<String, String> = HashMap::new();
    let mut policy_arn_to_deny_action_patterns: HashMap<String, Vec<String>> = HashMap::new();
    let mut policy_arn_to_deny_not_action_patterns: HashMap<String, Vec<String>> = HashMap::new();
    let mut policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>> = HashMap::new();

    for policy in policies {
        let allow_stmts = extract_unconditional_allow_statements(&policy.document);
        let patterns: Vec<String> = allow_stmts
            .iter()
            .flat_map(|s| s.action_patterns.iter().cloned())
            .collect();
        let (deny_action, deny_not_action) = extract_deny_patterns(&policy.document);

        // Populate name map.
        policy_arn_to_name.insert(policy.arn.clone(), policy.name.clone());

        // Populate allow-patterns map.
        policy_arn_to_allow_patterns.insert(policy.arn.clone(), patterns.clone());

        // Populate deny-patterns maps.
        policy_arn_to_deny_action_patterns.insert(policy.arn.clone(), deny_action);
        policy_arn_to_deny_not_action_patterns.insert(policy.arn.clone(), deny_not_action);

        // Populate allow-statements map (action + resource patterns per statement).
        policy_arn_to_allow_statements.insert(policy.arn.clone(), allow_stmts);

        // Accumulate concrete action count and service-prefix map.
        let mut total_count: u32 = 0;
        for pattern in &patterns {
            // Extract service prefix (before the first `:`).
            if let Some(colon_pos) = pattern.find(':') {
                let prefix = pattern[..colon_pos].to_lowercase();
                service_prefix_to_policy_arns
                    .entry(prefix)
                    .or_default()
                    .push(policy.arn.clone());
            }
            total_count += count_matching_actions(catalogue, pattern);
        }
        policy_arn_to_concrete_action_count.insert(policy.arn.clone(), total_count);
    }

    // Deduplicate ARN lists in service_prefix_to_policy_arns.
    for arns in service_prefix_to_policy_arns.values_mut() {
        arns.sort();
        arns.dedup();
    }

    PolicyIndex {
        built_at: Utc::now().to_rfc3339(),
        policy_cache_hash: policy_cache_hash.to_string(),
        service_prefix_to_policy_arns,
        policy_arn_to_allow_patterns,
        policy_arn_to_concrete_action_count,
        policy_arn_to_name,
        policy_arn_to_deny_action_patterns,
        policy_arn_to_deny_not_action_patterns,
        policy_arn_to_allow_statements,
    }
}

/// Returns `true` if the policy identified by `arn` in the index would deny
/// `action` (case-insensitive).
///
/// Checks both:
/// - `Deny + Action` patterns: action is denied if it matches any pattern.
/// - `Deny + NotAction` patterns: action is denied if it does NOT match any
///   of the exception patterns.
///
/// Conditions on Deny statements are ignored (conservative).
pub fn index_denies_action(index: &PolicyIndex, arn: &str, action: &str) -> bool {
    // Check Deny + Action patterns.
    if let Some(deny_patterns) = index.policy_arn_to_deny_action_patterns.get(arn) {
        if deny_patterns
            .iter()
            .any(|pat| action_covered_by(pat, action))
        {
            return true;
        }
    }

    // Check Deny + NotAction patterns.
    if let Some(not_action_patterns) = index.policy_arn_to_deny_not_action_patterns.get(arn) {
        if !not_action_patterns.is_empty() {
            // The action is denied unless it matches one of the exception patterns.
            let is_excepted = not_action_patterns
                .iter()
                .any(|pat| action_covered_by(pat, action));
            if !is_excepted {
                return true;
            }
        }
    }

    false
}

/// Compute the SHA-256 hex digest of the file at `path`.
pub fn file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Cannot open {:?} for hashing", path))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("Read error during SHA-256")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_policies::RawManagedPolicy;
    use serde_json::json;

    fn make_policy(arn: &str, name: &str, actions: &[&str]) -> RawManagedPolicy {
        let action_val: Vec<Value> = actions.iter().map(|a| json!(a)).collect();
        RawManagedPolicy {
            arn: arn.to_string(),
            name: name.to_string(),
            default_version_id: "v1".to_string(),
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": action_val,
                    "Resource": "*"
                }]
            }),
        }
    }

    use serde_json::Value;

    #[test]
    fn build_index_populates_maps() {
        let mut catalogue = ServiceCatalogue::new();
        catalogue.insert(
            "s3".to_string(),
            vec!["GetObject".to_string(), "PutObject".to_string()],
        );

        let policies = vec![
            make_policy(
                "arn:aws:iam::aws:policy/S3Full",
                "S3Full",
                &["s3:*"],
            ),
            make_policy(
                "arn:aws:iam::aws:policy/S3ReadOnly",
                "S3ReadOnly",
                &["s3:GetObject"],
            ),
        ];

        let index = build_index(&policies, &catalogue, "testhash");

        assert_eq!(index.policy_cache_hash, "testhash");
        assert!(index.service_prefix_to_policy_arns.contains_key("s3"));
        let s3_arns = &index.service_prefix_to_policy_arns["s3"];
        assert!(s3_arns.contains(&"arn:aws:iam::aws:policy/S3Full".to_string()));
        assert!(s3_arns.contains(&"arn:aws:iam::aws:policy/S3ReadOnly".to_string()));

        assert_eq!(
            index.policy_arn_to_concrete_action_count["arn:aws:iam::aws:policy/S3Full"],
            2
        );
        assert_eq!(
            index.policy_arn_to_concrete_action_count["arn:aws:iam::aws:policy/S3ReadOnly"],
            1
        );
    }
}
