//! Fetch and cache all AWS managed policy documents via the IAM API.
//!
//! On the first run this issues ~1 468 `GetPolicyVersion` calls (≈ 2.5 min at
//! 10 req/s).  Results are cached in a JSON file with a 7-day TTL so
//! subsequent runs are instant.

use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use aws_sdk_iam::Client as IamClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;

const CACHE_TTL_SECS: u64 = 7 * 24 * 3600; // 7 days
const MAX_CONCURRENT_IAM: usize = 8;

// ── Public types ─────────────────────────────────────────────────────────────

/// A single AWS managed policy with its parsed document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawManagedPolicy {
    pub arn: String,
    pub name: String,
    pub default_version_id: String,
    /// The parsed IAM policy document (JSON object).
    pub document: Value,
}

/// A single Allow statement extracted from a policy document, with both its
/// action patterns and the resource patterns it applies to.
///
/// Used to enable resource-aware matching: when checking whether a managed
/// policy covers a required action, we can also verify that the statement's
/// resource scope overlaps with the required resource ARNs for that action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllowStatement {
    /// IAM action patterns (e.g. `["s3:PutObject", "s3:Get*"]`).
    pub action_patterns: Vec<String>,
    /// Resource ARN patterns from the statement (e.g. `["arn:aws:s3:::*/*"]`).
    /// `["*"]` means the statement applies to all resources.
    pub resource_patterns: Vec<String>,
}

// ── Cache wrapper ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct PolicyCache {
    fetched_at: u64,
    policies: Vec<RawManagedPolicy>,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Load all AWS managed policies from `cache_path` if the cache is fresh,
/// otherwise fetch via the IAM API, persist to `cache_path`, and return.
pub async fn load_or_fetch_managed_policies(
    iam: &IamClient,
    cache_path: &Path,
) -> Result<Vec<RawManagedPolicy>> {
    // Try loading from cache first.
    if let Ok(raw) = std::fs::read(cache_path) {
        if let Ok(cached) = serde_json::from_slice::<PolicyCache>(&raw) {
            let now = unix_now();
            if now.saturating_sub(cached.fetched_at) < CACHE_TTL_SECS {
                tracing::info!(
                    "[managed_policies] Loaded {} policies from cache",
                    cached.policies.len()
                );
                return Ok(cached.policies);
            }
            tracing::info!("[managed_policies] Cache is stale — re-fetching ...");
        }
    }

    let policies = fetch_all_managed_policies(iam).await?;

    // Persist to cache.
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let wrapper = PolicyCache {
        fetched_at: unix_now(),
        policies: policies.clone(),
    };
    let json = serde_json::to_string_pretty(&wrapper)?;
    std::fs::write(cache_path, json)
        .with_context(|| format!("Failed to write policy cache to {:?}", cache_path))?;
    tracing::info!(
        "[managed_policies] Fetched and cached {} policies to {:?}",
        policies.len(),
        cache_path
    );

    Ok(policies)
}

/// Extract Deny patterns from a policy document.
///
/// Returns a tuple `(deny_action_patterns, deny_not_action_patterns)`:
/// - `deny_action_patterns`: patterns from `Effect: Deny` + `Action: [...]`
///   statements.  An action is denied if it matches any of these.
/// - `deny_not_action_patterns`: patterns from `Effect: Deny` + `NotAction: [...]`
///   statements.  An action is denied if it does **not** match any of these
///   (i.e. it is not in the exception list).
///
/// Conditions on Deny statements are ignored (conservative).
pub fn extract_deny_patterns(doc: &Value) -> (Vec<String>, Vec<String>) {
    let mut deny_action: Vec<String> = Vec::new();
    let mut deny_not_action: Vec<String> = Vec::new();

    let statements = match doc.get("Statement").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return (deny_action, deny_not_action),
    };

    for stmt in statements {
        let effect = stmt
            .get("Effect")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Deny") {
            continue;
        }

        // Deny + Action
        if let Some(action_val) = stmt.get("Action") {
            let patterns: Vec<String> = match action_val {
                Value::String(s) => vec![s.clone()],
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => vec![],
            };
            deny_action.extend(patterns);
        }

        // Deny + NotAction
        if let Some(not_action_val) = stmt.get("NotAction") {
            let patterns: Vec<String> = match not_action_val {
                Value::String(s) => vec![s.clone()],
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect(),
                _ => vec![],
            };
            deny_not_action.extend(patterns);
        }
    }

    (deny_action, deny_not_action)
}

/// Extract all Allow statements from a policy document as [`AllowStatement`]
/// values, each carrying both the action patterns and the resource patterns.
///
/// - Ignores `Deny` statements.
/// - Skips statements that use `NotAction` (conservative: we cannot enumerate
///   what is allowed).
/// - Includes statements that have a `Condition` field — conditions are
///   preserved so callers can decide how to handle them.
/// - Returns one [`AllowStatement`] per Allow+Action statement, preserving the
///   resource patterns so callers can do resource-aware matching.
pub fn extract_allow_statements(doc: &Value) -> Vec<AllowStatement> {
    let mut result: Vec<AllowStatement> = Vec::new();

    let statements = match doc.get("Statement").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return result,
    };

    for stmt in statements {
        // Only process Allow statements.
        let effect = stmt
            .get("Effect")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Allow") {
            continue;
        }

        // Skip NotAction — we cannot enumerate what is allowed.
        if stmt.get("NotAction").is_some() {
            continue;
        }

        // Extract action patterns.
        let action_patterns: Vec<String> = match stmt.get("Action") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => continue, // No Action field — skip.
        };

        if action_patterns.is_empty() {
            continue;
        }

        // Extract resource patterns.
        let resource_patterns: Vec<String> = match stmt.get("Resource") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => vec!["*".to_string()], // No Resource field — treat as unrestricted.
        };

        result.push(AllowStatement {
            action_patterns,
            resource_patterns,
        });
    }

    result
}

/// Extract all Allow statements from a policy document, **excluding** any
/// statement that has a `Condition` field.
///
/// This is the conservative variant used when indexing managed AWS policies:
/// a conditional Allow may not fire at runtime, so we skip it to avoid
/// over-counting coverage.
///
/// - Ignores `Deny` statements.
/// - Skips statements that use `NotAction` (conservative: we cannot enumerate
///   what is allowed).
/// - Skips statements that have a `Condition` field (conservative: we cannot
///   evaluate conditions statically, so we assume the Allow may not fire).
/// - Returns one [`AllowStatement`] per Allow+Action statement, preserving the
///   resource patterns so callers can do resource-aware matching.
pub fn extract_unconditional_allow_statements(doc: &Value) -> Vec<AllowStatement> {
    let mut result: Vec<AllowStatement> = Vec::new();

    let statements = match doc.get("Statement").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return result,
    };

    for stmt in statements {
        // Only process Allow statements.
        let effect = stmt
            .get("Effect")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Allow") {
            continue;
        }

        // Skip statements with conditions — we cannot evaluate them statically.
        // A conditional Allow may not fire in practice (conservative).
        if stmt.get("Condition").is_some() {
            continue;
        }

        // Skip NotAction — we cannot enumerate what is allowed.
        if stmt.get("NotAction").is_some() {
            continue;
        }

        // Extract action patterns.
        let action_patterns: Vec<String> = match stmt.get("Action") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => continue, // No Action field — skip.
        };

        if action_patterns.is_empty() {
            continue;
        }

        // Extract resource patterns.
        let resource_patterns: Vec<String> = match stmt.get("Resource") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => vec!["*".to_string()], // No Resource field — treat as unrestricted.
        };

        result.push(AllowStatement {
            action_patterns,
            resource_patterns,
        });
    }

    result
}

/// Extract all concrete Allow action patterns from a policy document.
///
/// This is a convenience wrapper around [`extract_allow_statements`] that
/// flattens all action patterns across all Allow statements, regardless of
/// their Resource field.
///
/// - Handles `Action` as a `String` or an `Array`.
/// - Ignores `Deny` statements.
/// - Returns `[]` for statements that use `NotAction` (conservative: we
///   cannot enumerate what is allowed).
/// - Includes statements that have a `Condition` field (conditions are not
///   filtered here; use [`extract_unconditional_allow_statements`] if you
///   need to exclude conditional Allows).
/// - Returns patterns like `["s3:GetObject", "s3:Put*", "dynamodb:*"]`.
pub fn extract_allow_patterns(doc: &Value) -> Vec<String> {
    extract_allow_statements(doc)
        .into_iter()
        .flat_map(|stmt| stmt.action_patterns)
        .collect()
}

/// Returns `true` if the managed-policy resource pattern covers the required
/// resource pattern — i.e. the managed policy is broad enough to satisfy what
/// the script needs.
///
/// Parameters are **asymmetric**:
/// - `mp_resource`: the `Resource` field from the managed policy statement.
/// - `req_resource`: the `Resource` field from the script's `minimal_policy.json`.
///
/// The managed policy must be **at least as broad** as the required resource
/// scope.  We check `glob_match(mp, req)` — i.e. the managed-policy pattern
/// (treated as a glob) matches the required resource ARN.  We do NOT check
/// the reverse direction (`glob_match(req, mp)`), because that would mean the
/// required pattern is broader than the managed policy, which is exactly the
/// case where the managed policy is too restrictive.
///
/// Rules:
/// - `mp_resource == "*"` → the policy is unrestricted, always covers anything.
/// - `req_resource == "*"` → the script needs all resources; only a policy with
///   `Resource: "*"` covers this.  A restricted policy (e.g.
///   `"arn:aws:cloudwatch:*:*:alarm/*"`) does **not** cover `"*"`.
/// - Otherwise, the managed policy covers the required resource if
///   `glob_match(mp, req)` — the managed-policy pattern subsumes the required
///   resource ARN.
///
/// Examples:
/// - `"*"` vs `"arn:aws:s3:::*/*"` → covers (managed policy is unrestricted)
/// - `"arn:aws:s3:::*"` vs `"*"` → does NOT cover (policy is restricted, script needs all)
/// - `"arn:aws:s3:::*"` vs `"arn:aws:s3:::*/*"` → covers (s3:::* covers s3:::bucket/key)
/// - `"arn:aws:s3:::sagemaker-*"` vs `"arn:aws:s3:::*/*"` → does NOT cover (policy is too narrow)
/// - `"arn:aws:sqs:*"` vs `"arn:aws:s3:::*/*"` → does NOT cover (different services)
pub fn resource_patterns_overlap(mp_resource: &str, req_resource: &str) -> bool {
    use crate::service_ref::glob_match;

    // Policy covers all resources → always covers the required resource.
    if mp_resource == "*" {
        return true;
    }
    // Script requires all resources, but policy is restricted → does NOT cover.
    if req_resource == "*" {
        return false;
    }

    let mp = mp_resource.to_lowercase();
    let req = req_resource.to_lowercase();

    // The managed-policy pattern must subsume the required resource.
    // Only check mp → req direction: the managed policy must be at least as
    // broad as what the script needs.
    glob_match(mp.as_bytes(), req.as_bytes())
}

/// Returns `true` if the policy document contains a Deny statement that would
/// deny `action` (case-insensitive).
///
/// We handle two Deny forms:
///
/// 1. `Effect: Deny` + `Action: [...]`
///    Denies `action` if any pattern in the Action list covers it.
///
/// 2. `Effect: Deny` + `NotAction: [...]`
///    Denies everything **except** the listed actions, so `action` is denied
///    unless it matches one of the NotAction patterns.
///
/// Both forms are treated conservatively: conditions on the Deny statement are
/// **ignored** (we assume the worst case — the Deny fires).  This prevents the
/// benchmarker from selecting policies that are unreliable in practice (e.g.
/// `SageMakerStudioProjectUserRolePermissionsBoundary` which has a broad
/// conditional Deny on `Action: "*"`).
pub fn policy_denies_action(doc: &Value, action: &str) -> bool {
    use crate::service_ref::action_covered_by;

    let statements = match doc.get("Statement").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return false,
    };

    for stmt in statements {
        let effect = stmt
            .get("Effect")
            .and_then(|e| e.as_str())
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Deny") {
            continue;
        }

        // Form 1: Deny + Action
        if let Some(action_val) = stmt.get("Action") {
            let deny_patterns: Vec<&str> = match action_val {
                Value::String(s) => vec![s.as_str()],
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect(),
                _ => vec![],
            };
            if deny_patterns
                .iter()
                .any(|pat| action_covered_by(pat, action))
            {
                return true;
            }
        }

        // Form 2: Deny + NotAction — denies everything NOT in the list.
        if let Some(not_action_val) = stmt.get("NotAction") {
            let except_patterns: Vec<&str> = match not_action_val {
                Value::String(s) => vec![s.as_str()],
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect(),
                _ => vec![],
            };
            // The action is denied unless it matches one of the except patterns.
            let is_excepted = except_patterns
                .iter()
                .any(|pat| action_covered_by(pat, action));
            if !is_excepted {
                return true;
            }
        }
    }

    false
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Fetch all AWS managed policies (scope=AWS) via the IAM API.
async fn fetch_all_managed_policies(iam: &IamClient) -> Result<Vec<RawManagedPolicy>> {
    // Step 1: paginate ListPolicies to collect all ARNs + version IDs.
    tracing::info!("[managed_policies] Listing all AWS managed policies ...");
    let mut policy_stubs: Vec<(String, String, String)> = Vec::new(); // (arn, name, version_id)
    let mut paginator = iam
        .list_policies()
        .scope(aws_sdk_iam::types::PolicyScopeType::Aws)
        .into_paginator()
        .send();

    while let Some(page) = paginator.next().await {
        let page = page.context("iam:ListPolicies page failed")?;
        for p in page.policies() {
            let arn = p.arn().unwrap_or_default().to_string();
            let name = p.policy_name().unwrap_or_default().to_string();
            let version_id = p.default_version_id().unwrap_or_default().to_string();
            if !arn.is_empty() && !version_id.is_empty() {
                policy_stubs.push((arn, name, version_id));
            }
        }
    }

    tracing::info!(
        "[managed_policies] Found {} managed policies — fetching documents ...",
        policy_stubs.len()
    );

    // Step 2: fetch each policy document in parallel, bounded by semaphore.
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_IAM));
    let iam = Arc::new(iam.clone());

    let mut handles = Vec::with_capacity(policy_stubs.len());
    for (arn, name, version_id) in policy_stubs {
        let sem = Arc::clone(&semaphore);
        let iam_ref = Arc::clone(&iam);
        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            fetch_policy_document(&iam_ref, arn, name, version_id).await
        });
        handles.push(handle);
    }

    let mut policies: Vec<RawManagedPolicy> = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(p)) => policies.push(p),
            Ok(Err(e)) => tracing::warn!("[managed_policies] Failed to fetch a policy: {}", e),
            Err(e) => tracing::warn!("[managed_policies] Task panicked: {}", e),
        }
    }

    tracing::info!(
        "[managed_policies] Successfully fetched {} policy documents",
        policies.len()
    );
    Ok(policies)
}

/// Fetch a single policy version document and return a [`RawManagedPolicy`].
async fn fetch_policy_document(
    iam: &IamClient,
    arn: String,
    name: String,
    version_id: String,
) -> Result<RawManagedPolicy> {
    let resp = iam
        .get_policy_version()
        .policy_arn(&arn)
        .version_id(&version_id)
        .send()
        .await
        .with_context(|| format!("iam:GetPolicyVersion failed for {}", arn))?;

    let encoded_doc = resp
        .policy_version()
        .and_then(|v| v.document())
        .unwrap_or_default();

    // The document is URL-encoded.
    let decoded = urlencoding::decode(encoded_doc)
        .with_context(|| format!("URL-decode failed for {}", arn))?;

    let document: Value = serde_json::from_str(&decoded)
        .with_context(|| format!("JSON parse failed for policy document of {}", arn))?;

    Ok(RawManagedPolicy {
        arn,
        name,
        default_version_id: version_id,
        document,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── extract_allow_patterns tests ──────────────────────────────────────────

    #[test]
    fn extract_allow_patterns_string_action() {
        let doc = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*"
            }]
        });
        assert_eq!(extract_allow_patterns(&doc), vec!["s3:GetObject"]);
    }

    #[test]
    fn extract_allow_patterns_array_action() {
        let doc = json!({
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject"],
                "Resource": "*"
            }]
        });
        let patterns = extract_allow_patterns(&doc);
        assert!(patterns.contains(&"s3:GetObject".to_string()));
        assert!(patterns.contains(&"s3:PutObject".to_string()));
    }

    #[test]
    fn extract_allow_patterns_ignores_deny() {
        let doc = json!({
            "Statement": [
                {
                    "Effect": "Deny",
                    "Action": "s3:DeleteObject",
                    "Resource": "*"
                },
                {
                    "Effect": "Allow",
                    "Action": "s3:GetObject",
                    "Resource": "*"
                }
            ]
        });
        let patterns = extract_allow_patterns(&doc);
        assert_eq!(patterns, vec!["s3:GetObject"]);
    }

    #[test]
    fn extract_allow_patterns_ignores_not_action() {
        let doc = json!({
            "Statement": [{
                "Effect": "Allow",
                "NotAction": "iam:*",
                "Resource": "*"
            }]
        });
        assert!(extract_allow_patterns(&doc).is_empty());
    }

    #[test]
    fn extract_allow_patterns_empty_doc() {
        let doc = json!({});
        assert!(extract_allow_patterns(&doc).is_empty());
    }

    // ── extract_allow_statements tests ───────────────────────────────────────

    #[test]
    fn extract_allow_statements_includes_resource_restricted() {
        // Resource-restricted statements should be included (with their resource patterns).
        let doc = json!({
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "cloudwatch:PutMetricData",
                    "Resource": "arn:aws:cloudwatch:*:*:alarm/*"
                },
                {
                    "Effect": "Allow",
                    "Action": "s3:GetObject",
                    "Resource": "*"
                }
            ]
        });
        let stmts = extract_allow_statements(&doc);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].action_patterns, vec!["cloudwatch:PutMetricData"]);
        assert_eq!(stmts[0].resource_patterns, vec!["arn:aws:cloudwatch:*:*:alarm/*"]);
        assert_eq!(stmts[1].action_patterns, vec!["s3:GetObject"]);
        assert_eq!(stmts[1].resource_patterns, vec!["*"]);
    }

    #[test]
    fn extract_allow_statements_includes_conditional() {
        // extract_allow_statements does NOT filter out conditional statements —
        // it is the caller's responsibility to decide whether conditions matter.
        let doc = json!({
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "kms:GenerateDataKey",
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {
                            "kms:ViaService": "s3.us-east-1.amazonaws.com"
                        }
                    }
                },
                {
                    "Effect": "Allow",
                    "Action": "s3:GetObject",
                    "Resource": "*"
                }
            ]
        });
        let stmts = extract_allow_statements(&doc);
        // Both statements (conditional and unconditional) should be included.
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].action_patterns, vec!["kms:GenerateDataKey"]);
        assert_eq!(stmts[1].action_patterns, vec!["s3:GetObject"]);
    }

    #[test]
    fn extract_unconditional_allow_statements_skips_conditional() {
        // extract_unconditional_allow_statements filters out conditional statements
        // (used when indexing managed policies conservatively).
        let doc = json!({
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "cloudwatch:PutMetricData",
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {
                            "cloudwatch:namespace": "PanoramaDeviceMetrics"
                        }
                    }
                },
                {
                    "Effect": "Allow",
                    "Action": "s3:GetObject",
                    "Resource": "*"
                }
            ]
        });
        let stmts = extract_unconditional_allow_statements(&doc);
        // Only the unconditional statement should be included.
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0].action_patterns, vec!["s3:GetObject"]);
    }

    #[test]
    fn extract_allow_statements_resource_array() {
        let doc = json!({
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:PutObject", "s3:GetObject"],
                "Resource": ["arn:aws:s3:::my-bucket/*", "arn:aws:s3:::other-bucket/*"]
            }]
        });
        let stmts = extract_allow_statements(&doc);
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0].action_patterns, vec!["s3:PutObject", "s3:GetObject"]);
        assert_eq!(
            stmts[0].resource_patterns,
            vec!["arn:aws:s3:::my-bucket/*", "arn:aws:s3:::other-bucket/*"]
        );
    }

    #[test]
    fn extract_allow_statements_skips_not_action() {
        let doc = json!({
            "Statement": [{
                "Effect": "Allow",
                "NotAction": "iam:*",
                "Resource": "*"
            }]
        });
        assert!(extract_allow_statements(&doc).is_empty());
    }

    // ── resource_patterns_overlap tests ──────────────────────────────────────

    #[test]
    fn resource_overlap_star_mp_always_matches() {
        // mp_resource="*" covers anything
        assert!(resource_patterns_overlap("*", "arn:aws:s3:::my-bucket/*"));
        assert!(resource_patterns_overlap("*", "*"));
    }

    #[test]
    fn resource_overlap_star_req_only_covered_by_star_mp() {
        // req_resource="*" is only covered if mp_resource is also "*"
        assert!(!resource_patterns_overlap("arn:aws:s3:::*", "*"));
        assert!(!resource_patterns_overlap("arn:aws:cloudwatch:*:*:alarm/*", "*"));
    }

    #[test]
    fn resource_overlap_alarm_resource_does_not_cover_star() {
        // AutoScalingNotificationAccessRole allows cloudwatch:PutMetricData on
        // arn:aws:cloudwatch:*:*:alarm/* only. The script requires Resource: "*".
        // This should NOT overlap — the policy is too restrictive.
        assert!(!resource_patterns_overlap(
            "arn:aws:cloudwatch:*:*:alarm/*",
            "*"
        ));
    }

    #[test]
    fn resource_overlap_glob_subsumes() {
        // Managed policy "arn:aws:s3:::*" covers required "arn:aws:s3:::my-bucket/*"
        assert!(resource_patterns_overlap("arn:aws:s3:::*", "arn:aws:s3:::my-bucket/*"));
        // Required "arn:aws:s3:::*/*" is covered by managed "arn:aws:s3:::*"
        assert!(resource_patterns_overlap("arn:aws:s3:::*", "arn:aws:s3:::*/*"));
    }

    #[test]
    fn resource_overlap_different_services_no_match() {
        assert!(!resource_patterns_overlap(
            "arn:aws:sqs:us-east-1:*:*",
            "arn:aws:s3:::my-bucket/*"
        ));
    }

    #[test]
    fn resource_overlap_exact_match() {
        assert!(resource_patterns_overlap(
            "arn:aws:s3:::my-bucket/*",
            "arn:aws:s3:::my-bucket/*"
        ));
    }

    #[test]
    fn resource_overlap_sqs_wildcard_matches_sqs_arn() {
        // Managed policy "arn:aws:sqs:*:*:*" should overlap with required
        // "arn:aws:sqs:us-east-1:123456789012:*"
        assert!(resource_patterns_overlap(
            "arn:aws:sqs:*:*:*",
            "arn:aws:sqs:us-east-1:123456789012:*"
        ));
    }

    #[test]
    fn resource_overlap_narrow_sagemaker_does_not_cover_broad_s3() {
        // AmazonSageMakerCanvasForecastAccess allows s3:PutObject on
        // "arn:aws:s3:::sagemaker-*/Canvas*" and "arn:aws:s3:::sagemaker-*/canvas*".
        // The script requires s3:PutObject on "arn:aws:s3:::*/*".
        // The managed policy is NARROWER than the required resource → should NOT cover.
        assert!(!resource_patterns_overlap(
            "arn:aws:s3:::sagemaker-*/Canvas*",
            "arn:aws:s3:::*/*"
        ));
        assert!(!resource_patterns_overlap(
            "arn:aws:s3:::sagemaker-*/canvas*",
            "arn:aws:s3:::*/*"
        ));
    }

    #[test]
    fn resource_overlap_broad_covers_narrow_but_not_vice_versa() {
        // "arn:aws:s3:::*" covers "arn:aws:s3:::sagemaker-*/Canvas*" (broad → narrow)
        assert!(resource_patterns_overlap(
            "arn:aws:s3:::*",
            "arn:aws:s3:::sagemaker-*/Canvas*"
        ));
        // But "arn:aws:s3:::sagemaker-*/Canvas*" does NOT cover "arn:aws:s3:::*" (narrow → broad)
        assert!(!resource_patterns_overlap(
            "arn:aws:s3:::sagemaker-*/Canvas*",
            "arn:aws:s3:::*"
        ));
    }

    // ── policy_denies_action tests ────────────────────────────────────────────

    #[test]
    fn deny_action_matches_exact() {
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "Action": "sqs:SendMessage",
                "Resource": "*"
            }]
        });
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
        assert!(!policy_denies_action(&doc, "sqs:ReceiveMessage"));
    }

    #[test]
    fn deny_action_wildcard_covers_required() {
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "Action": "*",
                "Resource": "*"
            }]
        });
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
        assert!(policy_denies_action(&doc, "s3:PutObject"));
    }

    #[test]
    fn deny_action_array_matches() {
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "Action": ["sqs:SendMessage", "s3:DeleteObject"],
                "Resource": "*"
            }]
        });
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
        assert!(policy_denies_action(&doc, "s3:DeleteObject"));
        assert!(!policy_denies_action(&doc, "s3:PutObject"));
    }

    #[test]
    fn deny_not_action_denies_unlisted() {
        // NotAction: ["s3:GetObject"] means everything EXCEPT s3:GetObject is denied.
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "NotAction": ["s3:GetObject"],
                "Resource": "*"
            }]
        });
        // sqs:SendMessage is not in the NotAction list → it IS denied.
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
        // s3:GetObject IS in the NotAction list → it is NOT denied.
        assert!(!policy_denies_action(&doc, "s3:GetObject"));
    }

    #[test]
    fn deny_not_action_case_insensitive() {
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "NotAction": ["S3:GetObject"],
                "Resource": "*"
            }]
        });
        assert!(!policy_denies_action(&doc, "s3:getobject"));
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
    }

    #[test]
    fn allow_only_policy_does_not_deny() {
        let doc = json!({
            "Statement": [{
                "Effect": "Allow",
                "Action": ["sqs:SendMessage", "s3:PutObject"],
                "Resource": "*"
            }]
        });
        assert!(!policy_denies_action(&doc, "sqs:SendMessage"));
        assert!(!policy_denies_action(&doc, "s3:PutObject"));
    }

    #[test]
    fn conditional_deny_still_detected() {
        // Even with conditions, we conservatively treat it as a deny.
        let doc = json!({
            "Statement": [{
                "Effect": "Deny",
                "Action": "*",
                "Resource": "*",
                "Condition": {
                    "Null": { "aws:PrincipalTag/SomeTag": "true" }
                }
            }]
        });
        assert!(policy_denies_action(&doc, "sqs:SendMessage"));
    }
}
