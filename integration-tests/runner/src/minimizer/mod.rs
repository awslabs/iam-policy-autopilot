//! Policy minimizer
//!
//! Implements Zeller's **ddmin** (delta-debugging minimization) algorithm in
//! two passes to find the minimal IAM policy that still allows a language
//! script to run successfully.
//!
//! ## Two-pass minimization
//!
//! **Pass 1 — document level**: treats each individual policy document (one per
//! SDK call, from `--individual-policies`) as an atomic unit and finds the
//! 1-minimal subset of documents that still passes.
//!
//! **Pass 2 — statement level**: takes the merged policy produced by pass 1,
//! extracts every `Statement` object as an atom, and runs ddmin again to find
//! the 1-minimal subset of statements that still passes.  This removes spurious
//! statements that autopilot bundled into the same document as a genuinely
//! required statement (e.g. `s3-object-lambda:PutObject` alongside `s3:PutObject`).
//!
//! ## Algorithm (ddmin)
//!
//! Given a set `c` that is known to pass (the full policy set), ddmin finds a
//! 1-minimal subset `c' ⊆ c` such that:
//!   - `test(c') = pass`
//!   - for every element `e ∈ c'`, `test(c' \ {e}) = fail`
//!
//! The algorithm works by repeatedly trying to remove chunks of the current
//! passing set:
//!   1. Split `current` into `n` equal-sized chunks.
//!   2. For each chunk `c_i`, test `current \ c_i` (remove the chunk).
//!      If it passes → shrink `current` to `current \ c_i`, reset n=2, restart.
//!   3. For each chunk `c_i`, test `c_i` alone.
//!      If it passes → shrink `current` to `c_i`, reset n=2, restart.
//!   4. If no reduction found and `n < |current|` → double `n`, retry from step 2.
//!   5. If `n >= |current|` → done; `current` is 1-minimal.
//!
//! The caller supplies a `run_fn` closure that accepts a **list** of individual
//! policy documents (the subset being tested) and returns:
//!   - `Ok(true)`  — the script succeeded (the subset is sufficient)
//!   - `Ok(false)` — the script failed with AccessDenied (subset insufficient)
//!   - `Err(_)`    — any other (non-IAM) error; treated as "fail" (insufficient)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of a full minimization run.
#[derive(Debug, Serialize, Deserialize)]
pub struct MinimizationResult {
    /// The minimal policy document — a merge of the necessary individual policies.
    pub minimal_policy: Value,
    /// Total number of validation runs performed.
    pub runs_performed: u32,
    /// Number of actions across all individual policies (original total).
    pub original_action_count: usize,
    /// Number of actions in the minimal policy.
    pub minimal_action_count: usize,
    /// Number of actions removed.
    pub actions_removed: usize,
}

// ---------------------------------------------------------------------------
// Helper: count total actions across all Allow statements in a policy
// ---------------------------------------------------------------------------

fn count_policy_actions(policy: &Value) -> usize {
    policy
        .get("Statement")
        .and_then(|s| s.as_array())
        .map(|stmts| {
            stmts
                .iter()
                .filter(|stmt| {
                    stmt.get("Effect")
                        .and_then(|e| e.as_str())
                        .unwrap_or("Allow")
                        == "Allow"
                })
                .map(|stmt| match stmt.get("Action") {
                    Some(Value::Array(arr)) => arr.len(),
                    Some(Value::String(_)) => 1,
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// ddmin core
// ---------------------------------------------------------------------------

/// Run `test_fn` on the given subset of `all_policies` (selected by `indices`).
/// Returns `true` if the run succeeded (`Ok(true)`), `false` otherwise.
#[allow(clippy::future_not_send)]
async fn test_subset<F, Fut>(
    indices: &[usize],
    all_policies: &[Value],
    run_fn: &F,
    runs_performed: &mut u32,
    label: &str,
) -> bool
where
    F: Fn(Vec<Value>) -> Fut,
    Fut: Future<Output = anyhow::Result<bool>>,
{
    let actions: Vec<String> = indices
        .iter()
        .flat_map(|&i| {
            all_policies[i]
                .get("Statement")
                .and_then(|s| s.as_array())
                .map(|stmts| {
                    stmts
                        .iter()
                        .flat_map(|stmt| match stmt.get("Action") {
                            Some(Value::Array(arr)) => arr
                                .iter()
                                .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                                .collect::<Vec<_>>(),
                            Some(Value::String(s)) => vec![s.clone()],
                            _ => vec![],
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    let subset: Vec<Value> = indices.iter().map(|&i| all_policies[i].clone()).collect();
    *runs_performed += 1;
    warn!(
        "[ddmin] run #{}: {} ({} policies, {} actions): {:?}",
        runs_performed,
        label,
        indices.len(),
        actions.len(),
        actions
    );

    let result = run_fn(subset).await;
    let passed = matches!(result, Ok(true));
    warn!(
        "[ddmin] run #{}: {} → {}",
        runs_performed,
        label,
        if passed { "PASS" } else { "FAIL" }
    );
    passed
}

/// Zeller's ddmin algorithm.
///
/// Precondition: `test_fn(all_indices)` must return `true` (the full set passes).
/// Returns the 1-minimal subset of `all_indices` that still passes.
#[allow(clippy::future_not_send)]
async fn ddmin<F, Fut>(
    all_indices: Vec<usize>,
    all_policies: &[Value],
    run_fn: &F,
    runs_performed: &mut u32,
) -> Vec<usize>
where
    F: Fn(Vec<Value>) -> Fut,
    Fut: Future<Output = anyhow::Result<bool>>,
{
    let mut current = all_indices;
    let mut n: usize = 2; // number of chunks to split into

    loop {
        let len = current.len();

        if len == 0 {
            return current;
        }

        // Can't split into more chunks than elements.
        let n_actual = n.min(len);

        // Partition `current` into `n_actual` roughly equal chunks.
        let chunks: Vec<Vec<usize>> = {
            let chunk_size = len.div_ceil(n_actual);
            current.chunks(chunk_size).map(<[usize]>::to_vec).collect()
        };

        info!(
            "[ddmin] current={} policies, n={} chunks of ~{} each",
            len,
            chunks.len(),
            len.div_ceil(n_actual)
        );

        let mut reduced = false;

        // ── Pass 1: try removing each chunk (test complement) ──────────────
        for (i, chunk) in chunks.iter().enumerate() {
            // complement = current \ chunk
            let complement: Vec<usize> = current
                .iter()
                .copied()
                .filter(|idx| !chunk.contains(idx))
                .collect();

            if complement.is_empty() {
                continue;
            }

            let label = format!("complement of chunk {}/{}", i + 1, chunks.len());
            if test_subset(&complement, all_policies, run_fn, runs_performed, &label).await {
                info!(
                    "[ddmin] Reduced: removed chunk {}/{} ({} policies)",
                    i + 1,
                    chunks.len(),
                    chunk.len()
                );
                current = complement;
                n = 2_usize.max(n.saturating_sub(1)); // reset toward 2
                reduced = true;
                break;
            }
        }

        if reduced {
            continue;
        }

        // ── Pass 2: try each chunk alone ────────────────────────────────────
        for (i, chunk) in chunks.iter().enumerate() {
            // Only try if the chunk is strictly smaller than current (otherwise no progress).
            if chunk.len() >= current.len() {
                continue;
            }
            let label = format!("chunk {}/{} alone", i + 1, chunks.len());
            if test_subset(chunk, all_policies, run_fn, runs_performed, &label).await {
                info!(
                    "[ddmin] Reduced: kept only chunk {}/{} ({} policies)",
                    i + 1,
                    chunks.len(),
                    chunk.len()
                );
                current = chunk.clone();
                n = 2;
                reduced = true;
                break;
            }
        }

        if reduced {
            continue;
        }

        // ── No reduction found at this granularity ──────────────────────────
        if n_actual >= len {
            // Already at maximum granularity (one chunk per element) — done.
            info!("[ddmin] No further reduction possible — result is 1-minimal");
            break;
        }

        // Increase granularity.
        n = (n * 2).min(len);
        info!("[ddmin] Increasing granularity to n={}", n);
    }

    current
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Minimize a set of individual policy documents using Zeller's ddmin algorithm
/// in two passes:
///
/// **Pass 1 — document level**: treats each individual policy document (one per
/// SDK call) as an atomic unit and finds the 1-minimal subset of documents that
/// still allows the script to succeed.
///
/// **Pass 2 — statement level**: takes the merged policy from pass 1, extracts
/// every `Statement` object as an atom, and runs ddmin again to find the
/// 1-minimal subset of statements that still passes.  This removes spurious
/// statements that autopilot bundled into the same document as a genuinely
/// required statement (e.g. `s3-object-lambda:PutObject` alongside
/// `s3:PutObject`).
///
/// # Arguments
/// - `individual_policies`: one policy document per SDK call (from `--individual-policies`)
/// - `run_fn`: async closure that takes a **list** of individual policy
///   documents (the subset being tested) and returns `Ok(true)` if the run
///   succeeded, `Ok(false)` if it failed with AccessDenied, `Err(_)` for any
///   other error (treated as "fail").
#[allow(clippy::future_not_send)]
pub async fn minimize_policy<F, Fut>(
    individual_policies: Vec<Value>,
    run_fn: F,
) -> MinimizationResult
where
    F: Fn(Vec<Value>) -> Fut + Clone,
    Fut: Future<Output = anyhow::Result<bool>>,
{
    let n = individual_policies.len();
    info!(
        "[ddmin] Pass 1: starting document-level minimization over {} individual policies ...",
        n
    );

    // Count total actions across all individual policies (original total).
    let original_action_count: usize = individual_policies.iter().map(count_policy_actions).sum();

    let mut runs_performed: u32 = 0;

    // Verify the full set passes (precondition for ddmin).
    let all_indices: Vec<usize> = (0..n).collect();
    info!("[ddmin] Verifying full set passes ...");
    if !test_subset(
        &all_indices,
        &individual_policies,
        &run_fn,
        &mut runs_performed,
        "full set",
    )
    .await
    {
        warn!("[ddmin] Full set does not pass — cannot minimize; returning full set as-is");
        let full_policy = merge_policies(&individual_policies);
        let full_action_count = count_policy_actions(&full_policy);
        return MinimizationResult {
            minimal_policy: full_policy,
            runs_performed,
            original_action_count,
            minimal_action_count: full_action_count,
            actions_removed: 0,
        };
    }

    // ── Pass 1: document-level ddmin ────────────────────────────────────────
    let minimal_doc_indices = ddmin(
        all_indices,
        &individual_policies,
        &run_fn,
        &mut runs_performed,
    )
    .await;

    let removable_docs = n.saturating_sub(minimal_doc_indices.len());
    info!(
        "[ddmin] Pass 1 complete: {}/{} documents removable after {} run(s)",
        removable_docs, n, runs_performed
    );

    // Merge the surviving documents into a single policy for pass 2.
    let surviving_policies: Vec<Value> = minimal_doc_indices
        .iter()
        .map(|&i| individual_policies[i].clone())
        .collect();
    let merged_after_pass1 = merge_policies(&surviving_policies);

    // ── Pass 2: statement-level ddmin ───────────────────────────────────────
    // Extract all Statement objects from the merged policy as a flat list.
    let all_statements: Vec<Value> = merged_after_pass1
        .get("Statement")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    let num_stmts = all_statements.len();
    info!(
        "[ddmin] Pass 2: starting statement-level minimization over {} statements ...",
        num_stmts
    );

    // Build a run_fn adapter that wraps a subset of statements into a single
    // policy document and delegates to the original run_fn.
    let run_fn_stmts = {
        let run_fn = run_fn.clone();
        move |stmt_subset: Vec<Value>| {
            let run_fn = run_fn.clone();
            async move {
                // Wrap the statement subset in a single policy document and
                // pass it as a one-element Vec to the original run_fn.
                let policy_doc = serde_json::json!({
                    "Version": "2012-10-17",
                    "Statement": stmt_subset,
                });
                run_fn(vec![policy_doc]).await
            }
        }
    };

    let minimal_stmt_indices = if num_stmts == 0 {
        vec![]
    } else {
        // Pass 1 already verified that the surviving document set passes.
        // Re-verifying here would waste a run and can spuriously fail due to
        // IAM propagation timing or script non-determinism.  Skip the baseline
        // check and go straight to ddmin.
        let all_stmt_indices: Vec<usize> = (0..num_stmts).collect();
        ddmin(
            all_stmt_indices,
            &all_statements,
            &run_fn_stmts,
            &mut runs_performed,
        )
        .await
    };

    let removable_stmts = num_stmts.saturating_sub(minimal_stmt_indices.len());
    info!(
        "[ddmin] Pass 2 complete: {}/{} statements removable after {} total run(s)",
        removable_stmts, num_stmts, runs_performed
    );

    // Build the final minimal policy from the surviving statements.
    let minimal_statements: Vec<Value> = minimal_stmt_indices
        .iter()
        .map(|&i| all_statements[i].clone())
        .collect();
    let minimal_policy = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": minimal_statements,
    });

    let minimal_action_count = count_policy_actions(&minimal_policy);
    let actions_removed = original_action_count.saturating_sub(minimal_action_count);

    info!(
        "[ddmin] Final result: {} → {} actions ({} removed, {:.0}% reduction)",
        original_action_count,
        minimal_action_count,
        actions_removed,
        if original_action_count > 0 {
            #[allow(clippy::cast_precision_loss)]
            {
                actions_removed as f64 / original_action_count as f64 * 100.0
            }
        } else {
            0.0
        }
    );

    MinimizationResult {
        minimal_policy,
        runs_performed,
        original_action_count,
        minimal_action_count,
        actions_removed,
    }
}

/// Merge a list of individual policy documents into a single combined policy.
///
/// `Sid` fields are stripped from every statement during the merge.  When
/// multiple policy documents are combined into one, duplicate Sid values would
/// cause IAM to reject the merged document with `MalformedPolicyDocument`.
/// Sids are optional identifiers that carry no semantic weight for permission
/// evaluation, so removing them is safe.
fn merge_policies(policies: &[Value]) -> Value {
    let mut statements: Vec<Value> = Vec::new();
    for policy in policies {
        if let Some(stmts) = policy.get("Statement").and_then(|s| s.as_array()) {
            for stmt in stmts {
                let mut s = stmt.clone();
                if let Some(obj) = s.as_object_mut() {
                    obj.remove("Sid");
                }
                statements.push(s);
            }
        }
    }
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": statements,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
