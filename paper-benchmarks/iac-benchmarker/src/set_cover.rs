//! Greedy, exact (branch-and-bound), and min-actions (ILP via HiGHS)
//! set-cover algorithms for managed policy selection.
//!
//! **Input**: a set of required IAM actions R (already filtered to only
//! coverable actions) and a list of candidate managed policy ARNs.
//!
//! **Output**: the smallest subset of candidates whose allow patterns
//! collectively cover every action in R (greedy/exact), or the subset
//! that minimises the total concrete-action count (min-actions).

use std::collections::{HashMap, HashSet};

use highs::{HighsModelStatus, RowProblem, Sense};

use crate::managed_policies::resource_patterns_overlap;
use crate::policy_index::PolicyIndex;
use crate::service_ref::action_covered_by;

// ── Public types ─────────────────────────────────────────────────────────────

/// Result of a set-cover run (greedy or exact).
pub struct SetCoverResult {
    /// ARNs of selected managed policies, in selection order.
    pub selected_arns: Vec<String>,
    /// Required actions that are covered by the selected policies.
    pub covered_actions: HashSet<String>,
    /// Required actions that could not be covered by any candidate.
    pub uncovered_actions: HashSet<String>,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Greedy set-cover: at each step pick the candidate that covers the most
/// currently-uncovered required actions.
///
/// Tie-breaking: when two candidates cover the same number of required actions,
/// prefer the one with the **lower total concrete action count** (i.e. the more
/// targeted / least-privileged policy).  This makes selection deterministic and
/// semantically correct — it avoids granting broader permissions than necessary.
///
/// `required` must already be filtered to only coverable actions (see Phase 1
/// design decision in `BENCHMARK_PLAN.md`).
///
/// Time complexity: O(|candidates|² × |required|) in the worst case, which is
/// fast enough for the typical candidate set size of 10–80.
pub fn greedy_set_cover(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
) -> SetCoverResult {
    greedy_set_cover_impl(index, candidates, required, &HashMap::new())
}

/// Resource-aware variant of [`greedy_set_cover`].
///
/// Identical to [`greedy_set_cover`] but uses
/// [`actions_covered_by_policy_with_resources`] internally so that a managed
/// policy statement is only counted as covering an action when its resource
/// scope overlaps with the required resource ARNs for that action.
pub fn greedy_set_cover_with_resources(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    greedy_set_cover_impl(index, candidates, required, required_action_to_resources)
}

fn greedy_set_cover_impl(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    let mut uncovered: HashSet<String> = required.clone();
    let mut covered_actions: HashSet<String> = HashSet::new();
    let mut selected_arns: Vec<String> = Vec::new();
    let mut remaining_candidates: Vec<&String> = candidates.iter().collect();

    while !uncovered.is_empty() && !remaining_candidates.is_empty() {
        // Find the candidate that covers the most uncovered actions.
        // Tie-break by preferring the policy with the fewest total concrete
        // actions (least-privileged), using Reverse so that a smaller count
        // sorts higher in max_by_key.
        let best_idx = remaining_candidates
            .iter()
            .enumerate()
            .map(|(i, arn)| {
                let covered = actions_covered_by_policy_with_resources(
                    index, arn, &uncovered, required_action_to_resources,
                );
                let total = index
                    .policy_arn_to_concrete_action_count
                    .get(*arn)
                    .copied()
                    .unwrap_or(u32::MAX);
                (i, covered.len(), total)
            })
            .max_by(|a, b| {
                // Primary: more covered actions is better.
                // Secondary: fewer total concrete actions is better (least-privilege).
                a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2))
            })
            .map(|(i, count, _)| (i, count));

        match best_idx {
            None => break,
            Some((_, 0)) => break, // No candidate covers anything new.
            Some((idx, _)) => {
                let arn = remaining_candidates.remove(idx);
                // Collect to owned Strings first to release the borrow on `uncovered`.
                let newly_covered: Vec<String> = actions_covered_by_policy_with_resources(
                    index, arn, &uncovered, required_action_to_resources,
                )
                .into_iter()
                .map(|s| s.to_string())
                .collect();
                for action in newly_covered {
                    uncovered.remove(&action);
                    covered_actions.insert(action);
                }
                selected_arns.push(arn.clone());
            }
        }
    }

    SetCoverResult {
        selected_arns,
        covered_actions,
        uncovered_actions: uncovered,
    }
}

/// Exact branch-and-bound set-cover.
///
/// Only call when `candidates.len() <= 30`; for larger sets the search space
/// may be too large.  Falls back to the greedy result as an upper bound to
/// prune branches early.
pub fn exact_set_cover(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
) -> SetCoverResult {
    exact_set_cover_impl(index, candidates, required, &HashMap::new())
}

/// Resource-aware variant of [`exact_set_cover`].
pub fn exact_set_cover_with_resources(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    exact_set_cover_impl(index, candidates, required, required_action_to_resources)
}

fn exact_set_cover_impl(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    // Use greedy solution as the initial upper bound.
    let greedy = greedy_set_cover_impl(index, candidates, required, required_action_to_resources);
    let greedy_size = greedy.selected_arns.len();

    // If greedy already covers everything with 0 or 1 policy, it's optimal.
    if greedy_size <= 1 || required.is_empty() {
        return greedy;
    }

    // Pre-compute coverage sets for each candidate.
    let coverage: Vec<HashSet<String>> = candidates
        .iter()
        .map(|arn| {
            actions_covered_by_policy_with_resources(
                index, arn, required, required_action_to_resources,
            )
            .into_iter()
            .map(|s| s.to_string())
            .collect()
        })
        .collect();

    // Branch-and-bound state.
    let mut best_selection: Vec<usize> = (0..greedy_size)
        .map(|i| {
            candidates
                .iter()
                .position(|a| *a == greedy.selected_arns[i])
                .unwrap_or(i)
        })
        .collect();
    let mut best_size = greedy_size;

    // Recursive search.
    let mut current: Vec<usize> = Vec::new();
    let mut covered: HashSet<String> = HashSet::new();
    branch_and_bound(
        &coverage,
        required,
        0,
        &mut current,
        &mut covered,
        &mut best_selection,
        &mut best_size,
    );

    // Reconstruct result from best_selection.
    let mut selected_arns: Vec<String> = best_selection
        .iter()
        .map(|&i| candidates[i].clone())
        .collect();
    // Preserve a deterministic order (index order).
    selected_arns.sort_by_key(|a| candidates.iter().position(|c| c == a).unwrap_or(usize::MAX));

    let covered_actions: HashSet<String> = selected_arns
        .iter()
        .flat_map(|arn| {
            actions_covered_by_policy_with_resources(
                index, arn, required, required_action_to_resources,
            )
            .into_iter()
            .map(|s| s.to_string())
        })
        .collect();

    let uncovered_actions: HashSet<String> = required
        .iter()
        .filter(|a| !covered_actions.contains(*a))
        .cloned()
        .collect();

    SetCoverResult {
        selected_arns,
        covered_actions,
        uncovered_actions,
    }
}

/// Min-actions ILP set-cover via HiGHS.
///
/// Finds a covering subset of `candidates` that minimises the **sum** of
/// `policy_arn_to_concrete_action_count` values across selected policies,
/// formulated as a binary integer linear programme and solved by HiGHS.
///
/// Falls back to the greedy result if HiGHS returns no optimal solution.
pub fn min_actions_cover(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
) -> SetCoverResult {
    min_actions_cover_impl(index, candidates, required, &HashMap::new())
}

/// Resource-aware variant of [`min_actions_cover`].
pub fn min_actions_cover_with_resources(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    min_actions_cover_impl(index, candidates, required, required_action_to_resources)
}

fn min_actions_cover_impl(
    index: &PolicyIndex,
    candidates: &[String],
    required: &HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> SetCoverResult {
    // Fast path: nothing to cover.
    let greedy = greedy_set_cover_impl(index, candidates, required, required_action_to_resources);
    if required.is_empty() {
        return greedy;
    }

    let n = candidates.len();

    // Pre-compute coverage sets for each candidate.
    let coverage: Vec<HashSet<String>> = candidates
        .iter()
        .map(|arn| {
            actions_covered_by_policy_with_resources(
                index, arn, required, required_action_to_resources,
            )
            .into_iter()
            .map(|s| s.to_string())
            .collect()
        })
        .collect();

    // Concrete-action cost for each candidate (objective coefficient).
    let concrete_count: Vec<f64> = candidates
        .iter()
        .map(|arn| {
            index
                .policy_arn_to_concrete_action_count
                .get(arn)
                .copied()
                .unwrap_or(0) as f64
        })
        .collect();

    // Build ILP:
    //   Variables: x_i ∈ {0,1}  (one per candidate)
    //   Objective: minimise Σ cost_i · x_i
    //   Constraints: for each required action a_j,
    //                Σ { x_i | a_j ∈ coverage[i] } >= 1
    let mut pb = RowProblem::default();

    // Add one binary variable per candidate with its cost as the objective coefficient.
    let cols: Vec<highs::Col> = (0..n)
        .map(|i| pb.add_integer_column(concrete_count[i], 0..=1))
        .collect();

    // Add one coverage constraint per required action.
    for action in required {
        let row_factors: Vec<(highs::Col, f64)> = (0..n)
            .filter(|&i| coverage[i].contains(action))
            .map(|i| (cols[i], 1.0))
            .collect();

        // Only add the constraint if at least one candidate covers this action.
        // (If none do, the problem is infeasible — fall back to greedy.)
        if row_factors.is_empty() {
            return greedy;
        }

        // Constraint: sum of covering x_i >= 1
        pb.add_row(1.0.., &row_factors);
    }

    // Solve.
    let solved = pb.optimise(Sense::Minimise).solve();

    if solved.status() != HighsModelStatus::Optimal {
        // Infeasible or error — fall back to greedy.
        return greedy;
    }

    let solution = solved.get_solution();
    let col_values = solution.columns();

    // Collect selected candidates (value >= 0.5 means selected).
    let mut selected_arns: Vec<String> = (0..n)
        .filter(|&i| col_values[i] >= 0.5)
        .map(|i| candidates[i].clone())
        .collect();

    // Preserve deterministic index order.
    selected_arns.sort_by_key(|a| candidates.iter().position(|c| c == a).unwrap_or(usize::MAX));

    let covered_actions: HashSet<String> = selected_arns
        .iter()
        .flat_map(|arn| {
            actions_covered_by_policy_with_resources(
                index, arn, required, required_action_to_resources,
            )
            .into_iter()
            .map(|s| s.to_string())
        })
        .collect();

    let uncovered_actions: HashSet<String> = required
        .iter()
        .filter(|a| !covered_actions.contains(*a))
        .cloned()
        .collect();

    SetCoverResult {
        selected_arns,
        covered_actions,
        uncovered_actions,
    }
}

/// Return the subset of `required` actions that the policy identified by `arn`
/// covers (using [`action_covered_by`] against its allow patterns).
///
/// This is the **resource-unaware** variant — it only checks action patterns,
/// not resource scope.  Use [`actions_covered_by_policy_with_resources`] when
/// you have per-action required resource ARNs available.
pub fn actions_covered_by_policy<'a>(
    index: &PolicyIndex,
    arn: &str,
    required: &'a HashSet<String>,
) -> HashSet<&'a str> {
    let patterns = match index.policy_arn_to_allow_patterns.get(arn) {
        Some(p) => p,
        None => return HashSet::new(),
    };

    required
        .iter()
        .filter(|action| {
            patterns
                .iter()
                .any(|pat| action_covered_by(pat, action))
        })
        .map(|s| s.as_str())
        .collect()
}

/// Return the subset of `required` actions that the policy identified by `arn`
/// covers, **also checking resource overlap**.
///
/// For each required action, the policy covers it only if there exists an
/// Allow statement in the policy where:
/// 1. The statement's action patterns cover the action (glob match), AND
/// 2. **Every** required resource ARN for that action is covered by at least
///    one of the statement's resource patterns (ALL-required-to-ANY-managed).
///    IAM evaluates all resource ARNs in a request; a partial match (e.g.
///    covering `dbname` but not `dbuser`) still results in a deny at runtime.
///
/// `required_action_to_resources` maps each required action (concrete, e.g.
/// `"s3:PutObject"`) to the list of resource ARN patterns from the minimal
/// policy that the action needs to operate on.  If an action has no entry in
/// this map (or the list is empty), resource checking is skipped for that
/// action (i.e. any resource scope is accepted).
pub fn actions_covered_by_policy_with_resources<'a>(
    index: &PolicyIndex,
    arn: &str,
    required: &'a HashSet<String>,
    required_action_to_resources: &HashMap<String, Vec<String>>,
) -> HashSet<&'a str> {
    let stmts = match index.policy_arn_to_allow_statements.get(arn) {
        Some(s) => s,
        None => return HashSet::new(),
    };

    required
        .iter()
        .filter(|action| {
            // Get the required resource ARNs for this action (if any).
            let req_resources: &[String] = required_action_to_resources
                .get(*action)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            // The policy covers this action if any of its Allow statements:
            //   (a) has an action pattern that covers the action, AND
            //   (b) has a resource pattern that overlaps with at least one
            //       required resource (or no required resources are specified).
            stmts.iter().any(|stmt| {
                let action_matches = stmt
                    .action_patterns
                    .iter()
                    .any(|pat| action_covered_by(pat, action));

                if !action_matches {
                    return false;
                }

                // If no required resources are specified, accept any resource scope.
                if req_resources.is_empty() {
                    return true;
                }

                // Check resource overlap: EVERY required resource must be
                // covered by at least one managed-policy resource pattern.
                // (IAM evaluates all resource ARNs in a request; a partial
                // match — e.g. covering dbname but not dbuser — still results
                // in a deny at runtime.)
                req_resources.iter().all(|req_res| {
                    stmt.resource_patterns
                        .iter()
                        .any(|mp_res| resource_patterns_overlap(mp_res, req_res))
                })
            })
        })
        .map(|s| s.as_str())
        .collect()
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Recursive branch-and-bound search (minimise number of policies).
///
/// - `coverage[i]` = set of required actions covered by candidate `i`.
/// - `start` = first candidate index to consider (avoids revisiting).
/// - `current` = indices of currently selected candidates.
/// - `covered` = union of coverage sets for `current`.
/// - `best_selection` / `best_size` = best solution found so far.
#[allow(clippy::too_many_arguments)]
fn branch_and_bound(
    coverage: &[HashSet<String>],
    required: &HashSet<String>,
    start: usize,
    current: &mut Vec<usize>,
    covered: &mut HashSet<String>,
    best_selection: &mut Vec<usize>,
    best_size: &mut usize,
) {
    // If everything is covered, update best if current is smaller.
    if covered.is_superset(required) {
        if current.len() < *best_size {
            *best_size = current.len();
            *best_selection = current.clone();
        }
        return;
    }

    // Prune: even if we add all remaining candidates, can we beat best_size?
    let remaining = coverage.len().saturating_sub(start);
    if current.len() + remaining < *best_size {
        // Optimistic check: can remaining candidates cover everything?
        // (We still need to check if they actually cover the uncovered set.)
    }
    // Hard prune: current size already >= best_size.
    if current.len() + 1 >= *best_size {
        return;
    }

    for i in start..coverage.len() {
        // Skip candidates that cover nothing new.
        let new_coverage: HashSet<String> = coverage[i]
            .iter()
            .filter(|a| !covered.contains(*a))
            .cloned()
            .collect();
        if new_coverage.is_empty() {
            continue;
        }

        // Add candidate i.
        current.push(i);
        for a in &new_coverage {
            covered.insert(a.clone());
        }

        branch_and_bound(coverage, required, i + 1, current, covered, best_selection, best_size);

        // Remove candidate i (backtrack).
        current.pop();
        for a in &new_coverage {
            covered.remove(a);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_policies::AllowStatement;
    use crate::policy_index::PolicyIndex;
    use std::collections::HashMap;

    /// Build a test index where every Allow statement has `Resource: ["*"]`.
    fn make_index(policies: &[(&str, &str, &[&str])]) -> PolicyIndex {
        let mut policy_arn_to_allow_patterns: HashMap<String, Vec<String>> = HashMap::new();
        let mut policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>> =
            HashMap::new();
        let mut policy_arn_to_name: HashMap<String, String> = HashMap::new();
        let mut policy_arn_to_concrete_action_count: HashMap<String, u32> = HashMap::new();
        let mut service_prefix_to_policy_arns: HashMap<String, Vec<String>> = HashMap::new();

        for (arn, name, patterns) in policies {
            let pattern_strings: Vec<String> =
                patterns.iter().map(|s| s.to_string()).collect();
            policy_arn_to_allow_patterns.insert(arn.to_string(), pattern_strings.clone());
            // Each pattern gets its own statement with Resource: ["*"].
            let stmts: Vec<AllowStatement> = pattern_strings
                .iter()
                .map(|p| AllowStatement {
                    action_patterns: vec![p.clone()],
                    resource_patterns: vec!["*".to_string()],
                })
                .collect();
            policy_arn_to_allow_statements.insert(arn.to_string(), stmts);
            policy_arn_to_name.insert(arn.to_string(), name.to_string());
            policy_arn_to_concrete_action_count.insert(arn.to_string(), patterns.len() as u32);
            for pat in *patterns {
                if let Some(pos) = pat.find(':') {
                    let prefix = pat[..pos].to_lowercase();
                    service_prefix_to_policy_arns
                        .entry(prefix)
                        .or_default()
                        .push(arn.to_string());
                }
            }
        }

        PolicyIndex {
            built_at: "2026-01-01T00:00:00Z".to_string(),
            policy_cache_hash: "test".to_string(),
            service_prefix_to_policy_arns,
            policy_arn_to_allow_patterns,
            policy_arn_to_concrete_action_count,
            policy_arn_to_name,
            policy_arn_to_deny_action_patterns: HashMap::new(),
            policy_arn_to_deny_not_action_patterns: HashMap::new(),
            policy_arn_to_allow_statements,
        }
    }

    #[test]
    fn greedy_covers_all_actions() {
        let index = make_index(&[
            ("arn:p1", "P1", &["s3:GetObject", "s3:PutObject"]),
            ("arn:p2", "P2", &["dynamodb:GetItem", "dynamodb:PutItem"]),
        ]);
        let candidates = vec!["arn:p1".to_string(), "arn:p2".to_string()];
        let required: HashSet<String> = [
            "s3:GetObject", "s3:PutObject", "dynamodb:GetItem", "dynamodb:PutItem",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let result = greedy_set_cover(&index, &candidates, &required);
        assert_eq!(result.selected_arns.len(), 2);
        assert!(result.uncovered_actions.is_empty());
        assert_eq!(result.covered_actions.len(), 4);
    }

    #[test]
    fn greedy_selects_most_covering_first() {
        let index = make_index(&[
            ("arn:p1", "P1", &["s3:GetObject"]),
            ("arn:p2", "P2", &["s3:GetObject", "s3:PutObject", "s3:ListBucket"]),
        ]);
        let candidates = vec!["arn:p1".to_string(), "arn:p2".to_string()];
        let required: HashSet<String> = [
            "s3:GetObject", "s3:PutObject", "s3:ListBucket",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let result = greedy_set_cover(&index, &candidates, &required);
        // P2 covers all 3, so only 1 policy should be selected.
        assert_eq!(result.selected_arns.len(), 1);
        assert_eq!(result.selected_arns[0], "arn:p2");
        assert!(result.uncovered_actions.is_empty());
    }

    #[test]
    fn greedy_reports_uncovered_actions() {
        let index = make_index(&[
            ("arn:p1", "P1", &["s3:GetObject"]),
        ]);
        let candidates = vec!["arn:p1".to_string()];
        let required: HashSet<String> = [
            "s3:GetObject", "lambda:InvokeFunction",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let result = greedy_set_cover(&index, &candidates, &required);
        assert!(result.uncovered_actions.contains("lambda:InvokeFunction"));
        assert!(result.covered_actions.contains("s3:GetObject"));
    }

    #[test]
    fn actions_covered_by_policy_uses_glob() {
        let index = make_index(&[
            ("arn:p1", "P1", &["s3:*"]),
        ]);
        let required: HashSet<String> = [
            "s3:GetObject", "s3:PutObject", "dynamodb:GetItem",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let covered = actions_covered_by_policy(&index, "arn:p1", &required);
        assert!(covered.contains("s3:GetObject"));
        assert!(covered.contains("s3:PutObject"));
        assert!(!covered.contains("dynamodb:GetItem"));
    }

    #[test]
    fn exact_cover_finds_optimal() {
        let index = make_index(&[
            ("arn:p1", "P1", &["s3:GetObject", "s3:PutObject"]),
            ("arn:p2", "P2", &["s3:GetObject"]),
            ("arn:p3", "P3", &["s3:PutObject"]),
        ]);
        let candidates = vec![
            "arn:p1".to_string(),
            "arn:p2".to_string(),
            "arn:p3".to_string(),
        ];
        let required: HashSet<String> = ["s3:GetObject", "s3:PutObject"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let result = exact_set_cover(&index, &candidates, &required);
        // P1 alone covers both — optimal is 1 policy.
        assert_eq!(result.selected_arns.len(), 1);
        assert_eq!(result.selected_arns[0], "arn:p1");
        assert!(result.uncovered_actions.is_empty());
    }

    // ── min_actions_cover tests ───────────────────────────────────────────────

    /// Helper that builds an index where each policy has an explicit concrete
    /// action count (not derived from the pattern list length).
    /// Every Allow statement gets `Resource: ["*"]`.
    fn make_index_with_counts(policies: &[(&str, &str, &[&str], u32)]) -> PolicyIndex {
        let mut policy_arn_to_allow_patterns: HashMap<String, Vec<String>> = HashMap::new();
        let mut policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>> =
            HashMap::new();
        let mut policy_arn_to_name: HashMap<String, String> = HashMap::new();
        let mut policy_arn_to_concrete_action_count: HashMap<String, u32> = HashMap::new();
        let mut service_prefix_to_policy_arns: HashMap<String, Vec<String>> = HashMap::new();

        for (arn, name, patterns, count) in policies {
            let pattern_strings: Vec<String> =
                patterns.iter().map(|s| s.to_string()).collect();
            policy_arn_to_allow_patterns.insert(arn.to_string(), pattern_strings.clone());
            let stmts: Vec<AllowStatement> = pattern_strings
                .iter()
                .map(|p| AllowStatement {
                    action_patterns: vec![p.clone()],
                    resource_patterns: vec!["*".to_string()],
                })
                .collect();
            policy_arn_to_allow_statements.insert(arn.to_string(), stmts);
            policy_arn_to_name.insert(arn.to_string(), name.to_string());
            policy_arn_to_concrete_action_count.insert(arn.to_string(), *count);
            for pat in *patterns {
                if let Some(pos) = pat.find(':') {
                    let prefix = pat[..pos].to_lowercase();
                    service_prefix_to_policy_arns
                        .entry(prefix)
                        .or_default()
                        .push(arn.to_string());
                }
            }
        }

        PolicyIndex {
            built_at: "2026-01-01T00:00:00Z".to_string(),
            policy_cache_hash: "test".to_string(),
            service_prefix_to_policy_arns,
            policy_arn_to_allow_patterns,
            policy_arn_to_concrete_action_count,
            policy_arn_to_name,
            policy_arn_to_deny_action_patterns: HashMap::new(),
            policy_arn_to_deny_not_action_patterns: HashMap::new(),
            policy_arn_to_allow_statements,
        }
    }

    /// min_actions_cover should cover all required actions.
    #[test]
    fn min_actions_cover_covers_all_required() {
        let index = make_index_with_counts(&[
            ("arn:p1", "P1", &["s3:GetObject", "s3:PutObject"], 2),
            ("arn:p2", "P2", &["dynamodb:GetItem", "dynamodb:PutItem"], 2),
        ]);
        let candidates = vec!["arn:p1".to_string(), "arn:p2".to_string()];
        let required: HashSet<String> = [
            "s3:GetObject",
            "s3:PutObject",
            "dynamodb:GetItem",
            "dynamodb:PutItem",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let result = min_actions_cover(&index, &candidates, &required);
        assert!(result.uncovered_actions.is_empty());
        assert_eq!(result.covered_actions.len(), 4);
    }

    /// When greedy is suboptimal, min_actions_cover should find the cheaper
    /// selection.
    ///
    /// Setup (3 required actions: a:X, a:Y, a:Z):
    ///   P_big: covers {a:X, a:Y, a:Z}  concrete_count = 100
    ///   P_a:   covers {a:X, a:Y}        concrete_count = 10
    ///   P_b:   covers {a:Z}             concrete_count = 5
    ///
    /// Greedy: P_big covers all 3 in one step → cost 100.
    /// min_actions: P_a + P_b covers all 3 → cost 15.
    #[test]
    fn min_actions_cover_beats_greedy_when_suboptimal() {
        let index = make_index_with_counts(&[
            ("arn:pbig", "P_big", &["a:X", "a:Y", "a:Z"], 100),
            ("arn:pa",   "P_a",   &["a:X", "a:Y"],         10),
            ("arn:pb",   "P_b",   &["a:Z"],                  5),
        ]);
        let candidates = vec![
            "arn:pbig".to_string(),
            "arn:pa".to_string(),
            "arn:pb".to_string(),
        ];
        let required: HashSet<String> = ["a:X", "a:Y", "a:Z"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let greedy_result = greedy_set_cover(&index, &candidates, &required);
        let min_result = min_actions_cover(&index, &candidates, &required);

        let greedy_cost: u32 = greedy_result
            .selected_arns
            .iter()
            .map(|arn| {
                index
                    .policy_arn_to_concrete_action_count
                    .get(arn)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        let min_cost: u32 = min_result
            .selected_arns
            .iter()
            .map(|arn| {
                index
                    .policy_arn_to_concrete_action_count
                    .get(arn)
                    .copied()
                    .unwrap_or(0)
            })
            .sum();

        // Greedy should pick P_big alone (cost 100); min_actions should find
        // P_a + P_b (cost 15).
        assert_eq!(greedy_result.selected_arns, vec!["arn:pbig"],
            "greedy should pick P_big (covers all 3 actions in one step)");
        assert!(
            min_cost < greedy_cost,
            "min_actions cost ({}) should be less than greedy cost ({})",
            min_cost,
            greedy_cost
        );
        assert!(min_result.uncovered_actions.is_empty());
        // min_actions should select P_a and P_b (total cost 15).
        let mut min_arns = min_result.selected_arns.clone();
        min_arns.sort();
        assert_eq!(min_arns, vec!["arn:pa", "arn:pb"]);
    }

    /// With empty required actions, min_actions_cover should return an empty
    /// selection with no uncovered actions.
    #[test]
    fn min_actions_cover_empty_required() {
        let index = make_index_with_counts(&[("arn:p1", "P1", &["s3:GetObject"], 1)]);
        let candidates = vec!["arn:p1".to_string()];
        let required: HashSet<String> = HashSet::new();

        let result = min_actions_cover(&index, &candidates, &required);
        assert!(result.selected_arns.is_empty());
        assert!(result.uncovered_actions.is_empty());
        assert!(result.covered_actions.is_empty());
    }

    // ── actions_covered_by_policy_with_resources tests ────────────────────────

    /// Build an index with explicit per-statement resource patterns.
    fn make_index_with_resources(
        policies: &[(&str, &[(&[&str], &[&str])])],
    ) -> PolicyIndex {
        let mut policy_arn_to_allow_patterns: HashMap<String, Vec<String>> = HashMap::new();
        let mut policy_arn_to_allow_statements: HashMap<String, Vec<AllowStatement>> =
            HashMap::new();
        let mut policy_arn_to_name: HashMap<String, String> = HashMap::new();
        let mut policy_arn_to_concrete_action_count: HashMap<String, u32> = HashMap::new();
        let mut service_prefix_to_policy_arns: HashMap<String, Vec<String>> = HashMap::new();

        for (arn, stmts_spec) in policies {
            let mut all_patterns: Vec<String> = Vec::new();
            let mut stmts: Vec<AllowStatement> = Vec::new();
            for (actions, resources) in *stmts_spec {
                let action_patterns: Vec<String> =
                    actions.iter().map(|s| s.to_string()).collect();
                let resource_patterns: Vec<String> =
                    resources.iter().map(|s| s.to_string()).collect();
                all_patterns.extend(action_patterns.clone());
                stmts.push(AllowStatement {
                    action_patterns,
                    resource_patterns,
                });
            }
            policy_arn_to_allow_patterns.insert(arn.to_string(), all_patterns.clone());
            policy_arn_to_allow_statements.insert(arn.to_string(), stmts);
            policy_arn_to_name.insert(arn.to_string(), arn.to_string());
            policy_arn_to_concrete_action_count
                .insert(arn.to_string(), all_patterns.len() as u32);
            for pat in &all_patterns {
                if let Some(pos) = pat.find(':') {
                    let prefix = pat[..pos].to_lowercase();
                    service_prefix_to_policy_arns
                        .entry(prefix)
                        .or_default()
                        .push(arn.to_string());
                }
            }
        }

        PolicyIndex {
            built_at: "2026-01-01T00:00:00Z".to_string(),
            policy_cache_hash: "test".to_string(),
            service_prefix_to_policy_arns,
            policy_arn_to_allow_patterns,
            policy_arn_to_concrete_action_count,
            policy_arn_to_name,
            policy_arn_to_deny_action_patterns: HashMap::new(),
            policy_arn_to_deny_not_action_patterns: HashMap::new(),
            policy_arn_to_allow_statements,
        }
    }

    /// A managed policy with `Resource: "*"` should cover an action regardless
    /// of the required resource ARN.
    #[test]
    fn resource_aware_star_resource_covers_any_required_resource() {
        let index = make_index_with_resources(&[(
            "arn:p1",
            &[(&["s3:PutObject"], &["*"])],
        )]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "s3:PutObject".to_string(),
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        );

        let covered =
            actions_covered_by_policy_with_resources(&index, "arn:p1", &required, &req_resources);
        assert!(covered.contains("s3:PutObject"));
    }

    /// A managed policy with a resource-restricted statement that does NOT
    /// overlap with the required resource should NOT cover the action.
    #[test]
    fn resource_aware_non_overlapping_resource_excluded() {
        // Managed policy only grants s3:PutObject on sqs resources (nonsensical
        // but tests the filtering logic).
        let index = make_index_with_resources(&[(
            "arn:p1",
            &[(&["s3:PutObject"], &["arn:aws:sqs:us-east-1:*:*"])],
        )]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "s3:PutObject".to_string(),
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        );

        let covered =
            actions_covered_by_policy_with_resources(&index, "arn:p1", &required, &req_resources);
        assert!(covered.is_empty(), "should not cover action with non-overlapping resource");
    }

    /// A managed policy with `Resource: "arn:aws:s3:::*"` should overlap with
    /// required `"arn:aws:s3:::my-bucket/*"` (glob subsumption).
    #[test]
    fn resource_aware_glob_subsumption_matches() {
        let index = make_index_with_resources(&[(
            "arn:p1",
            &[(&["s3:PutObject"], &["arn:aws:s3:::*"])],
        )]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "s3:PutObject".to_string(),
            vec!["arn:aws:s3:::my-bucket/*".to_string()],
        );

        let covered =
            actions_covered_by_policy_with_resources(&index, "arn:p1", &required, &req_resources);
        assert!(covered.contains("s3:PutObject"));
    }

    /// When no required resources are specified for an action, any resource
    /// scope in the managed policy should be accepted.
    #[test]
    fn resource_aware_no_required_resources_accepts_any() {
        let index = make_index_with_resources(&[(
            "arn:p1",
            &[(&["s3:PutObject"], &["arn:aws:s3:::specific-bucket/*"])],
        )]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        // Empty map — no resource constraint for this action.
        let req_resources: HashMap<String, Vec<String>> = HashMap::new();

        let covered =
            actions_covered_by_policy_with_resources(&index, "arn:p1", &required, &req_resources);
        assert!(covered.contains("s3:PutObject"));
    }

    /// Regression test for run_004-897f3738: AmazonSageMakerCanvasForecastAccess
    /// allows s3:PutObject on "arn:aws:s3:::sagemaker-*/Canvas*" only.
    /// The script requires s3:PutObject on "arn:aws:s3:::*/*".
    /// The narrow managed-policy resource must NOT cover the broad required resource.
    #[test]
    fn resource_aware_narrow_sagemaker_does_not_cover_broad_s3() {
        let index = make_index_with_resources(&[(
            "arn:sagemaker",
            &[(
                &["s3:PutObject"],
                &[
                    "arn:aws:s3:::sagemaker-*/Canvas*",
                    "arn:aws:s3:::sagemaker-*/canvas*",
                ],
            )],
        )]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "s3:PutObject".to_string(),
            vec!["arn:aws:s3:::*/*".to_string()],
        );

        let covered = actions_covered_by_policy_with_resources(
            &index,
            "arn:sagemaker",
            &required,
            &req_resources,
        );
        assert!(
            covered.is_empty(),
            "sagemaker-*/Canvas* should NOT cover */* — policy is too narrow"
        );
    }

    /// When a broad policy (Resource: "*") and a narrow policy (sagemaker-only)
    /// both offer s3:PutObject, the set-cover must pick the broad one.
    #[test]
    fn resource_aware_set_cover_picks_broad_over_narrow() {
        let index = make_index_with_resources(&[
            (
                "arn:narrow",
                &[(
                    &["s3:PutObject"],
                    &["arn:aws:s3:::sagemaker-*/Canvas*"],
                )],
            ),
            (
                "arn:broad",
                &[(&["s3:PutObject", "s3:GetObject", "s3:DeleteObject"], &["*"])],
            ),
        ]);
        let required: HashSet<String> = ["s3:PutObject"].iter().map(|s| s.to_string()).collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "s3:PutObject".to_string(),
            vec!["arn:aws:s3:::*/*".to_string()],
        );

        let candidates = vec!["arn:narrow".to_string(), "arn:broad".to_string()];
        let result = greedy_set_cover_with_resources(&index, &candidates, &required, &req_resources);
        assert!(
            result.selected_arns.contains(&"arn:broad".to_string()),
            "should select the broad policy that actually covers the required resource"
        );
        assert!(
            !result.selected_arns.contains(&"arn:narrow".to_string()),
            "should NOT select the narrow sagemaker policy"
        );
        assert!(result.uncovered_actions.is_empty());
    }

    /// Regression test for run_001-0aa559b7: AmazonRedshiftDataFullAccess grants
    /// `redshift:GetClusterCredentials` on `dbname:*/*` and
    /// `dbuser:*/redshift_data_api_user`.  The script requires the action on
    /// `dbuser:*/adminuser` and `dbname:*/securitydb`.
    ///
    /// The managed policy covers `dbname` (via `dbname:*/*`) but NOT `dbuser`
    /// (it only allows `redshift_data_api_user`, not `adminuser`).  With the
    /// old ANY-to-ANY logic the `dbname` match alone was enough to declare
    /// coverage; the fix requires ALL required resources to be covered.
    #[test]
    fn resource_aware_partial_resource_match_not_covered() {
        let index = make_index_with_resources(&[(
            "arn:redshift-data-full",
            &[(
                &["redshift:GetClusterCredentials"],
                &[
                    "arn:aws:redshift:*:*:dbname:*/*",
                    "arn:aws:redshift:*:*:dbuser:*/redshift_data_api_user",
                ],
            )],
        )]);
        let required: HashSet<String> = ["redshift:GetClusterCredentials"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "redshift:GetClusterCredentials".to_string(),
            vec![
                "arn:aws:redshift:*:*:dbuser:*/adminuser".to_string(),
                "arn:aws:redshift:*:*:dbname:*/securitydb".to_string(),
            ],
        );

        let covered = actions_covered_by_policy_with_resources(
            &index,
            "arn:redshift-data-full",
            &required,
            &req_resources,
        );
        assert!(
            covered.is_empty(),
            "managed policy covers dbname but NOT dbuser:*/adminuser — should not be considered covered"
        );
    }

    /// Verify that when a managed policy covers ALL required resources, the
    /// action IS considered covered (positive counterpart of the above test).
    #[test]
    fn resource_aware_full_resource_match_is_covered() {
        let index = make_index_with_resources(&[(
            "arn:redshift-broad",
            &[(
                &["redshift:GetClusterCredentials"],
                &[
                    "arn:aws:redshift:*:*:dbname:*/*",
                    "arn:aws:redshift:*:*:dbuser:*/*",
                ],
            )],
        )]);
        let required: HashSet<String> = ["redshift:GetClusterCredentials"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut req_resources: HashMap<String, Vec<String>> = HashMap::new();
        req_resources.insert(
            "redshift:GetClusterCredentials".to_string(),
            vec![
                "arn:aws:redshift:*:*:dbuser:*/adminuser".to_string(),
                "arn:aws:redshift:*:*:dbname:*/securitydb".to_string(),
            ],
        );

        let covered = actions_covered_by_policy_with_resources(
            &index,
            "arn:redshift-broad",
            &required,
            &req_resources,
        );
        assert!(
            covered.contains("redshift:GetClusterCredentials"),
            "managed policy covers both dbname and dbuser — should be considered covered"
        );
    }
}
