use super::PolicyMerger;
use crate::{
    errors::Result,
    policy_generation::{IamPolicy, Statement},
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
};

pub(crate) type ServiceActionCatalogs = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug, Clone)]
struct WildcardCandidate {
    statement_index: usize,
    service: String,
    prefix_len: usize,
    wildcard: String,
    replaced_actions: Vec<String>,
    added_actions: usize,
}

#[derive(Debug)]
struct WildcardSimulation {
    candidate: WildcardCandidate,
    savings: usize,
    policies: Vec<IamPolicy>,
}

/// Sort key for choosing the least-permissive useful wildcard.
///
/// `compare_preference` returns `Ordering::Less` when `self` should be applied
/// before `other`.
struct WildcardRanking<'a> {
    added_actions: usize,
    savings: usize,
    prefix_len: usize,
    statement_index: usize,
    service: &'a str,
    wildcard: &'a str,
}

impl WildcardSimulation {
    fn ranking(&self) -> WildcardRanking<'_> {
        WildcardRanking {
            added_actions: self.candidate.added_actions,
            savings: self.savings,
            prefix_len: self.candidate.prefix_len,
            statement_index: self.candidate.statement_index,
            service: &self.candidate.service,
            wildcard: &self.candidate.wildcard,
        }
    }
}

impl WildcardRanking<'_> {
    fn compare_preference(&self, other: &Self) -> Ordering {
        // Compare added-actions-per-character-saved exactly, without floating point.
        (self.added_actions as u128)
            .saturating_mul(other.savings as u128)
            .cmp(&(other.added_actions as u128).saturating_mul(self.savings as u128))
            // Prefer fewer added actions when the ratios are equal.
            .then_with(|| self.added_actions.cmp(&other.added_actions))
            // Prefer greater savings, then the narrowest wildcard.
            .then_with(|| other.savings.cmp(&self.savings))
            .then_with(|| other.prefix_len.cmp(&self.prefix_len))
            // Final stable keys make equivalent choices deterministic.
            .then_with(|| self.statement_index.cmp(&other.statement_index))
            .then_with(|| self.service.cmp(other.service))
            .then_with(|| self.wildcard.cmp(other.wildcard))
    }
}

impl PolicyMerger {
    /// Minimize the combined packed policy size by greedily applying action
    /// wildcard candidates with the fewest additional permissions per
    /// character saved.
    pub(crate) fn optimize_total_output_size(
        &self,
        policies: &[IamPolicy],
        catalogs: &ServiceActionCatalogs,
        target_size: usize,
    ) -> Result<Vec<IamPolicy>> {
        let mut statements = self.collect_all_statements(policies);
        Self::sort_statements_by_service(&mut statements);
        // Repack exact statements first so savings include policy-document overhead.
        let mut current_policies = self.pack_statements(statements.clone())?;
        let mut current_size = self.total_policy_size(&current_policies)?;

        while current_size > target_size {
            let mut best: Option<WildcardSimulation> = None;

            for candidate in Self::wildcard_candidates(&statements, catalogs) {
                // Repack every candidate because wildcarding can change policy boundaries.
                let mut simulated_statements = statements.clone();
                Self::apply_wildcard_candidate(&mut simulated_statements, &candidate);
                let simulated_policies = self.pack_statements(simulated_statements)?;
                let simulated_size = self.total_policy_size(&simulated_policies)?;
                let Some(savings) = current_size.checked_sub(simulated_size).filter(|s| *s > 0)
                else {
                    continue;
                };
                let simulation = WildcardSimulation {
                    candidate,
                    savings,
                    policies: simulated_policies,
                };

                if best
                    .as_ref()
                    .is_none_or(|existing| Self::is_better_simulation(&simulation, existing))
                {
                    best = Some(simulation);
                }
            }

            let Some(best) = best else {
                // No remaining wildcard reduces the packed output.
                break;
            };
            Self::apply_wildcard_candidate(&mut statements, &best.candidate);
            Self::sort_statements_by_service(&mut statements);
            current_size -= best.savings;
            current_policies = best.policies;
        }

        Ok(current_policies)
    }

    pub(crate) fn total_policy_size(&self, policies: &[IamPolicy]) -> Result<usize> {
        policies
            .iter()
            .map(|policy| self.calculate_policy_size(policy))
            .sum()
    }

    fn wildcard_candidates(
        statements: &[Statement],
        catalogs: &ServiceActionCatalogs,
    ) -> Vec<WildcardCandidate> {
        let mut candidates = Vec::new();

        for (statement_index, statement) in statements.iter().enumerate() {
            let existing_actions: HashSet<&str> =
                statement.action.iter().map(String::as_str).collect();
            let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();

            // Candidates never cross statement, service, or leading-word boundaries.
            for action in &statement.action {
                let Some((service, operation)) = action.split_once(':') else {
                    continue;
                };
                if operation.contains('*')
                    || operation.is_empty()
                    || !operation.chars().all(|ch| ch.is_ascii_alphanumeric())
                {
                    continue;
                }
                groups
                    .entry((
                        service.to_string(),
                        Self::leading_action_word(operation).to_string(),
                    ))
                    .or_default()
                    .push(action.clone());
            }

            for ((service, _), replaced_actions) in groups {
                if replaced_actions.len() < 2 {
                    continue;
                }
                let Some(catalog) = catalogs.get(&service) else {
                    continue;
                };
                let operations: Vec<&str> = replaced_actions
                    .iter()
                    .filter_map(|action| action.split_once(':').map(|(_, operation)| operation))
                    .collect();
                // Require every replaced action to be represented in the service catalog.
                if operations
                    .iter()
                    .any(|operation| !catalog.contains(*operation))
                {
                    continue;
                }
                let Some(prefix) = Self::common_word_aligned_prefix(&operations) else {
                    continue;
                };
                let wildcard = format!("{service}:{prefix}*");
                if wildcard == format!("{service}:*")
                    || Self::serialized_action_list_size(std::slice::from_ref(&wildcard))
                        >= Self::serialized_action_list_size(&replaced_actions)
                {
                    continue;
                }

                // Count catalog actions newly covered by the proposed wildcard.
                let added_actions = catalog
                    .iter()
                    .filter(|operation| operation.starts_with(prefix))
                    .map(|operation| format!("{service}:{operation}"))
                    .filter(|action| !existing_actions.contains(action.as_str()))
                    .count();

                candidates.push(WildcardCandidate {
                    statement_index,
                    service,
                    prefix_len: prefix.len(),
                    wildcard,
                    replaced_actions,
                    added_actions,
                });
            }
        }

        candidates
    }

    fn apply_wildcard_candidate(statements: &mut [Statement], candidate: &WildcardCandidate) {
        let statement = &mut statements[candidate.statement_index];
        let replaced: HashSet<&str> = candidate
            .replaced_actions
            .iter()
            .map(String::as_str)
            .collect();
        statement
            .action
            .retain(|action| !replaced.contains(action.as_str()));
        statement.action.push(candidate.wildcard.clone());
        statement.action.sort();
        statement.action.dedup();
    }

    fn is_better_simulation(candidate: &WildcardSimulation, existing: &WildcardSimulation) -> bool {
        candidate.ranking().compare_preference(&existing.ranking()) == Ordering::Less
    }

    /// Return the leading PascalCase word of an IAM operation name.
    fn leading_action_word(operation: &str) -> &str {
        let boundaries = Self::action_word_boundaries(operation);
        boundaries
            .first()
            .map_or(operation, |boundary| &operation[..*boundary])
    }

    /// Find the longest common prefix that ends at an action-word boundary.
    ///
    /// When one complete action is itself the common prefix (for example,
    /// `GetObject` and `GetObjectVersion`), that complete action is retained as
    /// the prefix even though it is not a boundary inside the shorter string.
    fn common_word_aligned_prefix<'a>(operations: &[&'a str]) -> Option<&'a str> {
        let first = *operations.first()?;
        let common_len = operations
            .iter()
            .skip(1)
            .fold(first.len(), |len, operation| {
                first
                    .as_bytes()
                    .iter()
                    .zip(operation.as_bytes())
                    .take(len)
                    .take_while(|(left, right)| left == right)
                    .count()
            });

        if common_len == 0 {
            return None;
        }

        let prefix_len = if operations
            .iter()
            .any(|operation| operation.len() == common_len)
        {
            common_len
        } else {
            Self::action_word_boundaries(first)
                .into_iter()
                .rfind(|boundary| *boundary <= common_len)?
        };

        (prefix_len > 0).then(|| &first[..prefix_len])
    }

    /// Byte offsets before each subsequent PascalCase word.
    fn action_word_boundaries(operation: &str) -> Vec<usize> {
        let bytes = operation.as_bytes();
        let mut boundaries = Vec::new();

        for index in 1..bytes.len() {
            let previous = bytes[index - 1];
            let current = bytes[index];
            let next = bytes.get(index + 1).copied();

            let lower_to_upper = previous.is_ascii_lowercase() && current.is_ascii_uppercase();
            let acronym_to_word = previous.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|byte| byte.is_ascii_lowercase());
            let digit_transition = current.is_ascii_digit() && !previous.is_ascii_digit();

            if lower_to_upper || acronym_to_word || digit_transition {
                boundaries.push(index);
            }
        }

        boundaries
    }

    /// Compact JSON size contributed by the strings and separators in an
    /// Action array. Brackets are omitted because they are constant.
    fn serialized_action_list_size(actions: &[String]) -> usize {
        actions.iter().map(|action| action.len() + 2).sum::<usize>()
            + actions.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statement(actions: Vec<&str>, resources: Vec<&str>) -> Statement {
        Statement::allow(
            actions.into_iter().map(String::from).collect(),
            resources.into_iter().map(String::from).collect(),
        )
    }

    fn catalog(service: &str, actions: &[&str]) -> ServiceActionCatalogs {
        BTreeMap::from([(
            service.to_string(),
            actions.iter().map(|action| (*action).to_string()).collect(),
        )])
    }

    #[test]
    fn test_action_wildcarding_only_runs_when_optimization_is_requested() {
        let statements = vec![
            statement(vec!["s3:GetObject"], vec!["arn:aws:s3:::bucket/*"]),
            statement(vec!["s3:GetObjectVersion"], vec!["arn:aws:s3:::bucket/*"]),
        ];

        let merger = PolicyMerger::new();
        let default_result = merger.merge_statements(&statements).unwrap();
        assert_eq!(
            default_result[0].statements[0].action,
            vec!["s3:GetObject", "s3:GetObjectVersion"]
        );

        let target = merger.total_policy_size(&default_result).unwrap() - 1;
        let collapsed = merger
            .optimize_total_output_size(
                &default_result,
                &catalog("s3", &["GetObject", "GetObjectVersion"]),
                target,
            )
            .unwrap();
        assert_eq!(collapsed[0].statements[0].action, vec!["s3:GetObject*"]);
    }

    #[test]
    fn test_action_wildcarding_uses_word_boundaries() {
        let merger = PolicyMerger::new();
        let statements = vec![
            statement(vec!["ec2:DescribeInstances"], vec!["*"]),
            statement(vec!["ec2:DescribeVolumes"], vec!["*"]),
            statement(vec!["ec2:RunInstances"], vec!["*"]),
        ];

        let exact = merger.merge_statements(&statements).unwrap();
        let collapsed = merger
            .optimize_total_output_size(
                &exact,
                &catalog(
                    "ec2",
                    &["DescribeInstances", "DescribeVolumes", "RunInstances"],
                ),
                0,
            )
            .unwrap();
        assert_eq!(
            collapsed[0].statements[0].action,
            vec!["ec2:Describe*", "ec2:RunInstances"]
        );
    }

    #[test]
    fn test_action_wildcarding_uses_broad_prefix_for_nested_prefixes() {
        let merger = PolicyMerger::new();
        let statements = vec![
            statement(vec!["s3:GetObject"], vec!["*"]),
            statement(vec!["s3:GetObjectAttributes"], vec!["*"]),
            statement(vec!["s3:GetObjectVersion"], vec!["*"]),
            statement(vec!["s3:GetBucketAcl"], vec!["*"]),
        ];

        let exact = merger.merge_statements(&statements).unwrap();
        let collapsed = merger
            .optimize_total_output_size(
                &exact,
                &catalog(
                    "s3",
                    &[
                        "GetObject",
                        "GetObjectAttributes",
                        "GetObjectVersion",
                        "GetBucketAcl",
                    ],
                ),
                0,
            )
            .unwrap();
        assert_eq!(collapsed[0].statements[0].action, vec!["s3:Get*"]);
    }

    #[test]
    fn test_action_wildcarding_never_creates_service_wildcard() {
        let merger = PolicyMerger::new();
        let statements = vec![
            statement(vec!["s3:GetObject"], vec!["*"]),
            statement(vec!["s3:PutObject"], vec!["*"]),
        ];

        let exact = merger.merge_statements(&statements).unwrap();
        let collapsed = merger
            .optimize_total_output_size(&exact, &catalog("s3", &["GetObject", "PutObject"]), 0)
            .unwrap();
        assert_eq!(
            collapsed[0].statements[0].action,
            vec!["s3:GetObject", "s3:PutObject"]
        );
    }

    #[test]
    fn test_action_wildcarding_skips_missing_catalogs() {
        let merger = PolicyMerger::new();
        let exact = merger
            .merge_statements(&[
                statement(vec!["s3:GetObject"], vec!["*"]),
                statement(vec!["s3:GetObjectVersion"], vec!["*"]),
            ])
            .unwrap();
        let result = merger
            .optimize_total_output_size(&exact, &ServiceActionCatalogs::new(), 0)
            .unwrap();
        assert_eq!(result, exact);

        let incomplete = catalog("s3", &["GetObject"]);
        let result = merger
            .optimize_total_output_size(&exact, &incomplete, 0)
            .unwrap();
        assert_eq!(result, exact);
    }

    #[test]
    fn test_action_wildcarding_prefers_fewer_added_actions_per_character() {
        let merger = PolicyMerger::new();
        let exact = merger
            .merge_statements(&[
                statement(vec!["s3:GetObject"], vec!["*"]),
                statement(vec!["s3:GetObjectVersion"], vec!["*"]),
                statement(vec!["s3:ListBucket"], vec!["*"]),
                statement(vec!["s3:ListBuckets"], vec!["*"]),
            ])
            .unwrap();
        let target = merger.total_policy_size(&exact).unwrap() - 1;
        let result = merger
            .optimize_total_output_size(
                &exact,
                &catalog(
                    "s3",
                    &[
                        "GetObject",
                        "GetObjectAcl",
                        "GetObjectAttributes",
                        "GetObjectTagging",
                        "GetObjectVersion",
                        "ListBucket",
                        "ListBuckets",
                    ],
                ),
                target,
            )
            .unwrap();
        let actions = &result[0].statements[0].action;
        assert!(actions.contains(&"s3:ListBucket*".to_string()));
        assert!(actions.contains(&"s3:GetObject".to_string()));
        assert!(actions.contains(&"s3:GetObjectVersion".to_string()));
    }

    #[test]
    fn test_action_wildcarding_keeps_exact_actions_when_already_under_target() {
        let merger = PolicyMerger::new();
        let exact = merger
            .merge_statements(&[
                statement(vec!["s3:GetObject"], vec!["*"]),
                statement(vec!["s3:GetObjectVersion"], vec!["*"]),
            ])
            .unwrap();
        let exact_size = merger.total_policy_size(&exact).unwrap();
        let result = merger
            .optimize_total_output_size(
                &exact,
                &catalog("s3", &["GetObject", "GetObjectVersion"]),
                exact_size,
            )
            .unwrap();
        assert_eq!(result, exact);
    }

    #[derive(Clone)]
    struct RankingTestInput {
        added_actions: usize,
        savings: usize,
        prefix_len: usize,
        statement_index: usize,
        service: &'static str,
        wildcard: &'static str,
    }

    impl Default for RankingTestInput {
        fn default() -> Self {
            Self {
                added_actions: 2,
                savings: 10,
                prefix_len: 3,
                statement_index: 1,
                service: "s3",
                wildcard: "s3:Get*",
            }
        }
    }

    fn simulation(input: RankingTestInput) -> WildcardSimulation {
        WildcardSimulation {
            candidate: WildcardCandidate {
                statement_index: input.statement_index,
                service: input.service.to_string(),
                prefix_len: input.prefix_len,
                wildcard: input.wildcard.to_string(),
                replaced_actions: vec![],
                added_actions: input.added_actions,
            },
            savings: input.savings,
            policies: vec![],
        }
    }

    #[test]
    fn test_wildcard_simulation_ties_use_stable_statement_order() {
        let earlier = simulation(RankingTestInput {
            statement_index: 0,
            ..RankingTestInput::default()
        });
        let later = simulation(RankingTestInput {
            statement_index: 1,
            ..RankingTestInput::default()
        });

        assert!(PolicyMerger::is_better_simulation(&earlier, &later));
        assert!(!PolicyMerger::is_better_simulation(&later, &earlier));
    }

    #[test]
    fn test_wildcard_ranking_tie_breakers() {
        let cases = [
            (
                "lower added-actions-per-character ratio",
                RankingTestInput {
                    added_actions: 1,
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
            (
                "fewer added actions",
                RankingTestInput {
                    added_actions: 1,
                    savings: 5,
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
            (
                "greater savings",
                RankingTestInput {
                    added_actions: 0,
                    savings: 11,
                    ..RankingTestInput::default()
                },
                RankingTestInput {
                    added_actions: 0,
                    ..RankingTestInput::default()
                },
            ),
            (
                "longer prefix",
                RankingTestInput {
                    prefix_len: 4,
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
            (
                "earlier statement",
                RankingTestInput {
                    statement_index: 0,
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
            (
                "lexicographically earlier service",
                RankingTestInput {
                    service: "ec2",
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
            (
                "lexicographically earlier wildcard",
                RankingTestInput {
                    wildcard: "s3:Delete*",
                    ..RankingTestInput::default()
                },
                RankingTestInput::default(),
            ),
        ];

        for (description, preferred_input, other_input) in cases {
            let preferred = simulation(preferred_input);
            let other = simulation(other_input);
            assert_eq!(
                preferred.ranking().compare_preference(&other.ranking()),
                Ordering::Less,
                "{description}"
            );
            assert_eq!(
                other.ranking().compare_preference(&preferred.ranking()),
                Ordering::Greater,
                "{description}"
            );
        }
    }
}
