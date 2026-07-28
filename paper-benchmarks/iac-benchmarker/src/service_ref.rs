//! Service-reference catalogue: fetch, cache, and glob-match IAM actions.
//!
//! The AWS service-reference endpoint returns a catalogue of every IAM action
//! for every service.  We use it to:
//!   1. Expand wildcard patterns (e.g. `s3:Get*`) into concrete action counts.
//!   2. Determine whether a concrete action is covered by a pattern.

use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use std::sync::Arc;

/// service prefix (e.g. `"s3"`) → sorted list of concrete action names
/// (e.g. `["GetObject", "PutObject", …]`).
///
/// Note: action names are stored **without** the service prefix.
pub type ServiceCatalogue = HashMap<String, Vec<String>>;

const CACHE_TTL_SECS: u64 = 7 * 24 * 3600; // 7 days

// ── Wire types for the service-reference API ────────────────────────────────

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    service: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ServiceDoc {
    #[serde(rename = "Actions")]
    actions: Vec<ActionEntry>,
}

#[derive(Debug, Deserialize)]
struct ActionEntry {
    #[serde(rename = "Name")]
    name: String,
}

// ── Cache wrapper ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CatalogueCache {
    fetched_at: u64,
    catalogue: ServiceCatalogue,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Load the service catalogue from `cache_path` if it is fresh (within
/// [`CACHE_TTL_SECS`]), otherwise fetch from the AWS service-reference
/// endpoint, persist to `cache_path`, and return the result.
pub async fn load_or_fetch_catalogue(cache_path: &Path) -> Result<ServiceCatalogue> {
    // Try loading from cache first.
    if let Ok(raw) = std::fs::read(cache_path) {
        if let Ok(cached) = serde_json::from_slice::<CatalogueCache>(&raw) {
            let now = unix_now();
            if now.saturating_sub(cached.fetched_at) < CACHE_TTL_SECS {
                tracing::info!(
                    "[service_ref] Loaded catalogue from cache ({} services)",
                    cached.catalogue.len()
                );
                return Ok(cached.catalogue);
            }
            tracing::info!("[service_ref] Cache is stale — re-fetching ...");
        }
    }

    let catalogue = fetch_catalogue().await?;

    // Persist to cache.
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let wrapper = CatalogueCache {
        fetched_at: unix_now(),
        catalogue: catalogue.clone(),
    };
    let json = serde_json::to_string_pretty(&wrapper)?;
    std::fs::write(cache_path, json)
        .with_context(|| format!("Failed to write catalogue cache to {:?}", cache_path))?;
    tracing::info!(
        "[service_ref] Fetched and cached {} services to {:?}",
        catalogue.len(),
        cache_path
    );

    Ok(catalogue)
}

/// Count how many concrete actions in the catalogue are matched by `pattern`
/// (e.g. `"s3:Get*"`).  Returns `0` if the service prefix is unknown.
pub fn count_matching_actions(catalogue: &ServiceCatalogue, pattern: &str) -> u32 {
    let (prefix, action_pat) = match split_action(pattern) {
        Some(p) => p,
        None => return 0,
    };
    let actions = match catalogue.get(&prefix.to_lowercase()) {
        Some(a) => a,
        None => return 0,
    };
    actions
        .iter()
        .filter(|a| action_covered_by(&format!("{}:{}", prefix, action_pat), &format!("{}:{}", prefix, a)))
        .count() as u32
}

/// Returns `true` if `pattern` (may contain `*` and `?`) covers `action`
/// (both compared case-insensitively).
///
/// `pattern` and `action` are expected to be in `service:ActionName` form.
pub fn action_covered_by(pattern: &str, action: &str) -> bool {
    glob_match(pattern.to_lowercase().as_bytes(), action.to_lowercase().as_bytes())
}

/// Iterative DP glob matcher.  Handles `*` (zero or more chars) and `?`
/// (exactly one char).  Both slices are expected to already be lowercased.
pub fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (plen, tlen) = (pattern.len(), text.len());
    // dp[i][j] = true iff pattern[..i] matches text[..j]
    // Use two rows to save memory.
    let mut prev = vec![false; tlen + 1];
    let mut curr = vec![false; tlen + 1];

    prev[0] = true;

    // A pattern of all `*`s matches the empty string.
    for i in 1..=plen {
        // curr[0] = true iff pattern[..i] matches the empty string,
        // which is only possible when every character so far is `*`.
        curr[0] = pattern[..i].iter().all(|&c| c == b'*');

        for j in 1..=tlen {
            curr[j] = match pattern[i - 1] {
                b'*' => curr[j - 1] || prev[j], // consume one char OR skip
                b'?' => prev[j - 1],             // exactly one char
                c => prev[j - 1] && c == text[j - 1],
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        // Reset curr for next iteration.
        for v in curr.iter_mut() {
            *v = false;
        }
    }

    prev[tlen]
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Fetch the full service catalogue from the AWS service-reference endpoint.
async fn fetch_catalogue() -> Result<ServiceCatalogue> {
    const INDEX_URL: &str = "https://servicereference.us-east-1.amazonaws.com/";
    const MAX_CONCURRENT: usize = 20;

    tracing::info!("[service_ref] Fetching service-reference index from {}", INDEX_URL);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        // CloudFront on this distribution returns 403 for the default reqwest
        // user-agent.  Using a curl-style user-agent resolves the 403.
        .user_agent("curl/8.17.0")
        .build()
        .context("Failed to build reqwest client")?;

    let response = client
        .get(INDEX_URL)
        .send()
        .await
        .context("GET service-reference index failed")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("Failed to read service-reference index body")?;

    tracing::info!(
        "[service_ref] Index response: status={}, body_len={}",
        status,
        body.len()
    );

    if !status.is_success() {
        anyhow::bail!(
            "Service-reference index returned HTTP {}: {}",
            status,
            &body
        );
    }

    let entries: Vec<ServiceEntry> = serde_json::from_str(&body)
        .with_context(|| format!(
            "Failed to parse service-reference index JSON (status={}, body_preview={:?})",
            status,
            &body[..body.len().min(200)]
        ))?;

    tracing::info!("[service_ref] Index has {} services — fetching action lists ...", entries.len());

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let client = Arc::new(client);

    let mut handles = Vec::with_capacity(entries.len());
    for entry in entries {
        let sem = Arc::clone(&semaphore);
        let cli = Arc::clone(&client);
        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            fetch_service_actions(&cli, &entry.service, &entry.url).await
        });
        handles.push(handle);
    }

    let mut catalogue: ServiceCatalogue = HashMap::new();
    for handle in handles {
        match handle.await {
            Ok(Ok((prefix, mut actions))) => {
                actions.sort();
                catalogue.insert(prefix, actions);
            }
            Ok(Err(e)) => {
                tracing::warn!("[service_ref] Failed to fetch a service doc: {}", e);
            }
            Err(e) => {
                tracing::warn!("[service_ref] Task panicked: {}", e);
            }
        }
    }

    Ok(catalogue)
}

/// Fetch the action list for a single service and return `(prefix, actions)`.
async fn fetch_service_actions(
    client: &reqwest::Client,
    service: &str,
    url: &str,
) -> Result<(String, Vec<String>)> {
    let doc: ServiceDoc = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {} failed", url))?
        .json()
        .await
        .with_context(|| format!("Failed to parse JSON from {}", url))?;

    let actions: Vec<String> = doc.actions.into_iter().map(|a| a.name).collect();
    Ok((service.to_lowercase(), actions))
}

/// Split `"s3:GetObject"` into `("s3", "GetObject")`.  Returns `None` if
/// there is no `:` separator.
fn split_action(action: &str) -> Option<(&str, &str)> {
    let pos = action.find(':')?;
    Some((&action[..pos], &action[pos + 1..]))
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

    #[test]
    fn glob_exact_match() {
        assert!(action_covered_by("s3:GetObject", "s3:GetObject"));
        assert!(!action_covered_by("s3:GetObject", "s3:PutObject"));
    }

    #[test]
    fn glob_star_prefix() {
        assert!(action_covered_by("s3:Get*", "s3:GetObject"));
        assert!(action_covered_by("s3:Get*", "s3:GetBucketAcl"));
        assert!(!action_covered_by("s3:Get*", "s3:PutObject"));
    }

    #[test]
    fn glob_full_wildcard() {
        assert!(action_covered_by("s3:*", "s3:GetObject"));
        assert!(action_covered_by("s3:*", "s3:PutObject"));
        assert!(!action_covered_by("s3:*", "dynamodb:GetItem"));
    }

    #[test]
    fn glob_question_mark() {
        assert!(action_covered_by("s3:GetObjec?", "s3:GetObject"));
        assert!(!action_covered_by("s3:GetObjec?", "s3:GetObjects"));
    }

    #[test]
    fn glob_case_insensitive() {
        assert!(action_covered_by("S3:GETOBJECT", "s3:getobject"));
        assert!(action_covered_by("s3:get*", "s3:GetObject"));
    }

    #[test]
    fn count_matching_returns_zero_for_unknown_prefix() {
        let catalogue: ServiceCatalogue = HashMap::new();
        assert_eq!(count_matching_actions(&catalogue, "unknownsvc:DoThing"), 0);
    }

    #[test]
    fn count_matching_counts_correctly() {
        let mut catalogue: ServiceCatalogue = HashMap::new();
        catalogue.insert(
            "s3".to_string(),
            vec!["GetObject".to_string(), "PutObject".to_string(), "GetBucketAcl".to_string()],
        );
        assert_eq!(count_matching_actions(&catalogue, "s3:Get*"), 2);
        assert_eq!(count_matching_actions(&catalogue, "s3:*"), 3);
        assert_eq!(count_matching_actions(&catalogue, "s3:PutObject"), 1);
    }
}
