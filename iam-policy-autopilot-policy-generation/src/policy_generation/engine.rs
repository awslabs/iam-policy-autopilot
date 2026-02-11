//! Policy generation engine implementation
//!
//! This module provides the main Engine for generating IAM policies from enriched method calls.
//! The engine processes EnrichedSdkMethodCall instances and creates corresponding IAM policies
//! with proper ARN pattern replacement.

use std::collections::BTreeMap;

use super::merge::{PolicyMerger, PolicyMergerConfig};
use super::utils::{ArnParser, ConditionValueProcessor};
use super::{IamPolicy, Statement};
use crate::api::model::GeneratePoliciesResult;
use crate::enrichment::{Action, Condition, EnrichedSdkMethodCall, Explanations};
use crate::errors::{ExtractorError, Result};
use crate::policy_generation::{PolicyType, PolicyWithMetadata};
use crate::Explanation;

/// Policy generation engine that converts enriched method calls into IAM policies
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Engine<'a> {
    /// ARN pattern parser for placeholder replacement
    arn_parser: ArnParser<'a>,
    /// Condition value processor for placeholder replacement
    condition_processor: ConditionValueProcessor<'a>,
    /// Policy merger for optimizing statements
    policy_merger: PolicyMerger,
}

impl<'a> Engine<'a> {
    /// Create a new policy generation engine with AWS context
    #[must_use]
    pub fn new(partition: &'a str, region: &'a str, account: &'a str) -> Self {
        Self::with_merger_config(partition, region, account, PolicyMergerConfig::default())
    }

    /// Create a new policy generation engine with AWS context and merger configuration
    #[must_use]
    pub fn with_merger_config(
        partition: &'a str,
        region: &'a str,
        account: &'a str,
        merger_config: PolicyMergerConfig,
    ) -> Self {
        Self {
            arn_parser: ArnParser::new(partition, region, account),
            condition_processor: ConditionValueProcessor::new(partition, region, account),
            policy_merger: PolicyMerger::with_config(merger_config),
        }
    }

    /// Generate IAM policies from enriched method calls
    ///
    /// Creates one IAM policy per EnrichedSdkMethodCall, with each Action becoming
    /// a separate statement within the policy. ARN patterns are processed to replace
    /// placeholder variables with actual values or wildcards.
    fn generate_individual_policies(
        &self,
        enriched_calls: &[EnrichedSdkMethodCall],
    ) -> Result<Vec<PolicyWithMetadata>> {
        let mut policies = Vec::new();

        for enriched_call in enriched_calls {
            let policy = self.generate_policy_for_call(enriched_call)?;
            policies.push(policy);
        }

        Ok(policies)
    }

    /// Generate a single IAM policy for an enriched method call
    fn generate_policy_for_call(
        &self,
        enriched_call: &EnrichedSdkMethodCall,
    ) -> Result<PolicyWithMetadata> {
        let mut policy = IamPolicy::new();

        for (index, action) in enriched_call.actions.iter().enumerate() {
            let statement = self.generate_statement_for_action(action, enriched_call, index)?;
            policy.add_statement(statement);
        }

        // Ensure we have at least one statement
        if policy.statements.is_empty() {
            return Err(ExtractorError::policy_generation(format!(
                "No statements generated for method call: {}",
                enriched_call.method_name
            )));
        }

        let policy_with_metadata = PolicyWithMetadata {
            policy,
            policy_type: PolicyType::Identity,
        };

        Ok(policy_with_metadata)
    }

    /// Generate a policy statement for a single action
    fn generate_statement_for_action(
        &self,
        action: &Action,
        enriched_call: &EnrichedSdkMethodCall,
        index: usize,
    ) -> Result<Statement> {
        // Process resources to get ARN patterns
        let resources = self.process_action_resources(action)?;

        // Create the statement
        let mut statement = Statement::allow(vec![action.name.clone()], resources);

        let conditions = self.process_action_conditions(action)?;

        statement = statement.with_conditions(conditions.clone());

        // Generate a descriptive SID
        let sid = self.generate_statement_id(enriched_call, action, index);
        statement = statement.with_sid(sid);

        Ok(statement)
    }

    /// Process resources for an action to extract and process ARN patterns
    pub(crate) fn process_action_resources(&self, action: &Action) -> Result<Vec<String>> {
        let mut processed_resources = Vec::new();

        for resource in &action.resources {
            if let Some(arn_patterns) = &resource.arn_patterns {
                // Process each ARN pattern
                log::debug!(
                    "process_action_resources: unprocessed ARN patterns: {}, {:?}",
                    action.name,
                    arn_patterns
                );
                let processed_patterns = self.arn_parser.process_arn_patterns(arn_patterns)?;
                log::debug!(
                    "process_action_resources: processed ARN patterns: {}, {:?}",
                    action.name,
                    processed_patterns
                );
                processed_resources.extend(processed_patterns);
            } else {
                // No ARN patterns available, use wildcard
                processed_resources.push("*".to_string());
            }
        }

        // If no resources were processed, default to wildcard
        if processed_resources.is_empty() {
            processed_resources.push("*".to_string());
        }

        // Remove subsumed resources to avoid redundant permissions
        let optimized_resources = self
            .policy_merger
            .remove_subsumed_resources(processed_resources)?;

        Ok(optimized_resources)
    }

    /// Process conditions for an action to replace placeholder variables
    pub(crate) fn process_action_conditions(&self, action: &Action) -> Result<Vec<Condition>> {
        let mut processed_conditions = Vec::new();

        for condition in &action.conditions {
            // Process each condition value to replace placeholders
            let (processed_values, wildcards_introduced) = self
                .condition_processor
                .process_condition_values(&condition.values)?;

            // Change operator to ...Like if wildcards were introduced
            let operator = if wildcards_introduced {
                condition.operator.to_like_version()
            } else {
                condition.operator.clone()
            };

            processed_conditions.push(Condition {
                operator,
                key: condition.key.clone(),
                values: processed_values,
            });
        }

        Ok(processed_conditions)
    }

    /// Generate a descriptive statement ID
    fn generate_statement_id(
        &self,
        enriched_call: &EnrichedSdkMethodCall,
        action: &Action,
        index: usize,
    ) -> String {
        // Extract action name without service prefix for cleaner SID
        let action_name = if let Some(colon_pos) = action.name.find(':') {
            &action.name[colon_pos + 1..]
        } else {
            &action.name
        };

        // Create SID: Allow{Service}{ActionName}{Index}
        format!(
            "Allow{}{}{}",
            enriched_call.service.to_uppercase(),
            action_name,
            if index > 0 {
                index.to_string()
            } else {
                String::new()
            }
        )
    }

    /// Merge multiple policies into optimized policies with size limits
    ///
    /// This method combines all statements from the input policies and groups them
    /// by mergeable resources to create optimized policies that avoid overly
    /// permissive resource grants. If the merged result would exceed IAM size limits,
    /// multiple policies are created as needed.
    ///
    /// # Arguments
    /// * `policies` - Slice of IAM policies to merge
    ///
    /// # Returns
    /// A vector of merged IAM policies, split as needed to stay within size limits
    ///
    /// # Errors
    /// Returns an error if policy merging fails
    pub(crate) fn merge_policies(
        &self,
        policies: &[PolicyWithMetadata],
    ) -> Result<Vec<PolicyWithMetadata>> {
        match policies.first() {
            None => Ok(vec![]),
            Some(first) if policies.iter().any(|p| p.policy_type != first.policy_type) => Err(
                ExtractorError::policy_generation("Cannot merge policies with different types"),
            ),
            Some(first) => {
                let policies = policies
                    .iter()
                    .map(|p| p.policy.clone())
                    .collect::<Vec<_>>();
                let merged = self.policy_merger.merge_policies(&policies)?;
                Ok(merged
                    .iter()
                    .map(|policy| PolicyWithMetadata {
                        policy: policy.clone(),
                        policy_type: first.policy_type,
                    })
                    .collect::<Vec<_>>())
            }
        }
    }

    /// Generate IAM policies with explanations from enriched method calls
    ///
    /// This method generates policies and collects explanations for why each action
    /// was added. Explanations are deduplicated to avoid redundant entries.
    ///
    /// # Arguments
    /// * `enriched_calls` - Slice of enriched method calls to process
    ///
    /// # Returns
    /// A PolicyGenerationResult containing policies and deduplicated explanations
    ///
    /// # Errors
    /// Returns an error if policy generation fails
    pub fn generate_policies(
        &self,
        enriched_calls: &[EnrichedSdkMethodCall],
    ) -> Result<GeneratePoliciesResult> {
        let policies = self.generate_individual_policies(enriched_calls)?;

        // Collect explanations
        let explanations = extract_explanations(enriched_calls);

        Ok(GeneratePoliciesResult {
            policies,
            explanations: Some(explanations),
        })
    }
}

fn extract_explanations(enriched_calls: &[EnrichedSdkMethodCall<'_>]) -> Explanations {
    let mut explanations: BTreeMap<String, Explanation> = BTreeMap::new();

    // Collect and merge explanations for each action name
    for call in enriched_calls {
        for action in &call.actions {
            explanations
                .entry(action.name.clone())
                .and_modify(|existing_explanation| {
                    existing_explanation.merge(action.explanation.clone());
                })
                .or_insert_with(|| action.explanation.clone());
        }
    }

    Explanations::new(explanations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Explanation, SdkMethodCall};

    use super::super::Effect;
    use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
    use crate::errors::ExtractorError;

    fn create_test_engine() -> Engine<'static> {
        Engine::new("aws", "us-east-1", "123456789012")
    }

    fn create_test_sdk_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
    }

    #[test]
    fn test_generate_policy_single_action() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                    ]),
                )],
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        assert_eq!(result.policies.len(), 1);

        let policy = &result.policies[0].policy;
        assert_eq!(policy.version, "2012-10-17");
        assert_eq!(policy.statements.len(), 1);

        let statement = &policy.statements[0];
        assert_eq!(statement.effect, Effect::Allow);
        assert_eq!(statement.action, vec!["s3:GetObject"]);
        assert_eq!(statement.resource, vec!["arn:aws:s3:::*/*"]);
        assert_eq!(statement.sid, Some("AllowS3GetObject".to_string()));
    }

    #[test]
    fn test_generate_policy_multiple_actions() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![
                Action::new(
                    "s3:GetObject".to_string(),
                    vec![Resource::new(
                        "object".to_string(),
                        Some(vec![
                            "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                        ]),
                    )],
                    vec![],
                    Explanation::default(),
                ),
                Action::new(
                    "s3:GetObjectVersion".to_string(),
                    vec![Resource::new(
                        "object".to_string(),
                        Some(vec![
                            "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                        ]),
                    )],
                    vec![],
                    Explanation::default(),
                ),
            ],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        assert_eq!(result.policies.len(), 1);

        let policy = &result.policies[0].policy;
        assert_eq!(policy.statements.len(), 2);

        // Check first statement
        let statement1 = &policy.statements[0];
        assert_eq!(statement1.action, vec!["s3:GetObject"]);
        assert_eq!(statement1.sid, Some("AllowS3GetObject".to_string()));

        // Check second statement
        let statement2 = &policy.statements[1];
        assert_eq!(statement2.action, vec!["s3:GetObjectVersion"]);
        assert_eq!(statement2.sid, Some("AllowS3GetObjectVersion1".to_string()));
    }

    #[test]
    fn test_generate_policy_no_arn_patterns() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "list_buckets".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:ListAllMyBuckets".to_string(),
                vec![Resource::new("*".to_string(), None)],
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        assert_eq!(result.policies.len(), 1);

        let policy = &result.policies[0].policy;
        let statement = &policy.statements[0];
        assert_eq!(statement.resource, vec!["*"]);
    }

    #[test]
    fn test_generate_policy_multiple_resources() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![
                Action::new(
                    "s3:GetObject".to_string(),
                    vec![
                        Resource::new(
                            "object".to_string(),
                            Some(vec!["arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()])
                        ),
                        Resource::new(
                            "accesspoint".to_string(),
                            Some(vec!["arn:${Partition}:s3:${Region}:${Account}:accesspoint/${AccessPointName}".to_string()])
                        )
                    ],
                    vec![],
                    Explanation::default(),
                )
            ],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        assert_eq!(result.policies.len(), 1);

        let policy = &result.policies[0].policy;
        let statement = &policy.statements[0];
        assert_eq!(
            statement.resource,
            vec![
                "arn:aws:s3:::*/*",
                "arn:aws:s3:us-east-1:123456789012:accesspoint/*"
            ]
        );
    }

    #[test]
    fn test_generate_statement_id() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![],
            sdk_method_call: &sdk_call,
        };

        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![],
            Explanation::default(),
        );

        // Test first action (index 0)
        let sid1 = engine.generate_statement_id(&enriched_call, &action, 0);
        assert_eq!(sid1, "AllowS3GetObject");

        // Test second action (index 1)
        let sid2 = engine.generate_statement_id(&enriched_call, &action, 1);
        assert_eq!(sid2, "AllowS3GetObject1");
    }

    #[test]
    fn test_generate_policies_empty_input() {
        let engine = create_test_engine();
        let result = engine.generate_policies(&[]).unwrap();
        assert!(result.policies.is_empty());
    }

    #[test]
    fn test_generate_policy_no_actions() {
        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        let enriched_call = EnrichedSdkMethodCall {
            method_name: "empty_call".to_string(),
            service: "s3".to_string(),
            actions: vec![],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]);
        assert!(result.is_err());

        if let Err(ExtractorError::PolicyGeneration { message, .. }) = result {
            assert!(message.contains("No statements generated"));
        } else {
            panic!("Expected PolicyGeneration error");
        }
    }

    #[test]
    fn test_merge_policies() {
        let engine = create_test_engine();

        // Create two policies with equivalent resources that should be merged
        let mut policy1 = IamPolicy::new();
        policy1.add_statement(create_test_statement(
            vec!["s3:GetObject"],
            vec!["arn:aws:s3:::bucket/*"],
        ));
        let policy1 = PolicyWithMetadata {
            policy: policy1,
            policy_type: PolicyType::Identity,
        };

        let mut policy2 = IamPolicy::new();
        policy2.add_statement(create_test_statement(
            vec!["s3:PutObject"],
            vec!["arn:aws:s3:::bucket/*"],
        ));
        let policy2 = PolicyWithMetadata {
            policy: policy2,
            policy_type: PolicyType::Identity,
        };

        let merged = engine.merge_policies(&[policy1, policy2]).unwrap();
        assert_eq!(merged.len(), 1);
        let policy = &merged[0].policy;

        // Should be merged into a single statement
        assert_eq!(policy.statements.len(), 1);
        let statement = &policy.statements[0];
        assert_eq!(statement.action.len(), 2);
        assert!(statement.action.contains(&"s3:GetObject".to_string()));
        assert!(statement.action.contains(&"s3:PutObject".to_string()));
        assert_eq!(statement.resource, vec!["arn:aws:s3:::bucket/*"]);
    }

    #[test]
    fn test_merge_policies_empty() {
        let engine = create_test_engine();

        let merged = engine.merge_policies(&[]).unwrap();
        assert!(merged.is_empty());
    }

    fn create_test_statement(actions: Vec<&str>, resources: Vec<&str>) -> Statement {
        Statement::allow(
            actions.into_iter().map(String::from).collect(),
            resources.into_iter().map(String::from).collect(),
        )
    }

    #[test]
    fn test_process_action_resources_removes_subsumed() {
        let engine = create_test_engine();

        // Create an action with subsumed resources (the example case from requirements)
        let action = Action::new(
            "events:PutRule".to_string(),
            vec![Resource::new(
                "rule".to_string(),
                Some(vec![
                    "arn:${Partition}:events:${Region}:${Account}:rule/*".to_string(),
                    "arn:${Partition}:events:${Region}:${Account}:rule/*/*".to_string(),
                ]),
            )],
            vec![],
            Explanation::default(),
        );

        let processed_resources = engine.process_action_resources(&action).unwrap();

        // Should only contain the more general resource (rule/*), not the subsumed one (rule/*/*)
        assert_eq!(processed_resources.len(), 1);
        assert_eq!(
            processed_resources[0],
            "arn:aws:events:us-east-1:123456789012:rule/*"
        );
    }

    #[test]
    fn test_process_action_resources_preserves_incomparable() {
        let engine = create_test_engine();

        // Create an action with incomparable resources (different services)
        let action = Action::new(
            "multi:Action".to_string(),
            vec![
                Resource::new(
                    "s3-bucket".to_string(),
                    Some(vec!["arn:${Partition}:s3:::${BucketName}/*".to_string()]),
                ),
                Resource::new(
                    "dynamodb-table".to_string(),
                    Some(vec![
                        "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"
                            .to_string(),
                    ]),
                ),
            ],
            vec![],
            Explanation::default(),
        );

        let processed_resources = engine.process_action_resources(&action).unwrap();

        // Should contain both resources since they're incomparable (different services)
        assert_eq!(processed_resources.len(), 2);
        assert!(processed_resources.contains(&"arn:aws:s3:::*/*".to_string()));
        assert!(processed_resources
            .contains(&"arn:aws:dynamodb:us-east-1:123456789012:table/*".to_string()));
    }

    #[test]
    fn test_process_action_conditions() {
        let engine = create_test_engine();

        // Create an action with conditions containing placeholders
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![
                Condition {
                    operator: crate::enrichment::Operator::StringEquals,
                    key: "s3:ExistingObjectTag/Environment".to_string(),
                    values: vec!["s3.${region}.amazonaws.com".to_string()],
                },
                Condition {
                    operator: crate::enrichment::Operator::StringEquals,
                    key: "aws:RequestedRegion".to_string(),
                    values: vec!["${region}".to_string(), "us-west-${unknown}".to_string()],
                },
            ],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 2);

        // Check first condition
        let condition1 = &processed_conditions[0];
        assert_eq!(condition1.key, "s3:ExistingObjectTag/Environment");
        assert_eq!(condition1.values, vec!["s3.us-east-1.amazonaws.com"]);

        // Check second condition
        let condition2 = &processed_conditions[1];
        assert_eq!(condition2.key, "aws:RequestedRegion");
        assert_eq!(condition2.values, vec!["us-east-1", "us-west-*"]);
    }

    #[test]
    fn test_process_action_conditions_no_placeholders() {
        let engine = create_test_engine();

        // Create an action with conditions without placeholders
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: "s3:ExistingObjectTag/Environment".to_string(),
                values: vec!["production".to_string(), "staging".to_string()],
            }],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(condition.key, "s3:ExistingObjectTag/Environment");
        assert_eq!(condition.values, vec!["production", "staging"]);
    }

    #[test]
    fn test_process_action_conditions_empty() {
        let engine = create_test_engine();

        // Create an action with no conditions
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 0);
    }

    #[test]
    fn test_process_action_conditions_with_wildcards() {
        let engine = create_test_engine();

        // Create an action with conditions that will introduce wildcards
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: "s3:ExistingObjectTag/Environment".to_string(),
                values: vec!["s3.${unknown}.amazonaws.com".to_string()],
            }],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(condition.key, "s3:ExistingObjectTag/Environment");
        assert_eq!(condition.values, vec!["s3.*.amazonaws.com"]);
        // Operator should be changed to StringLike because wildcards were introduced
        assert_eq!(condition.operator, crate::enrichment::Operator::StringLike);
    }

    #[test]
    fn test_process_action_conditions_mixed_wildcards() {
        let engine = create_test_engine();

        // Create an action with conditions where some introduce wildcards and some don't
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![
                Condition {
                    operator: crate::enrichment::Operator::StringEquals,
                    key: "aws:RequestedRegion".to_string(),
                    values: vec!["${region}".to_string()], // Known placeholder, no wildcards
                },
                Condition {
                    operator: crate::enrichment::Operator::StringEquals,
                    key: "s3:ExistingObjectTag/Environment".to_string(),
                    values: vec!["s3.${unknown}.amazonaws.com".to_string()], // Unknown placeholder, introduces wildcards
                },
            ],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 2);

        // First condition should keep StringEquals (no wildcards introduced)
        let condition1 = &processed_conditions[0];
        assert_eq!(condition1.key, "aws:RequestedRegion");
        assert_eq!(condition1.values, vec!["us-east-1"]);
        assert_eq!(
            condition1.operator,
            crate::enrichment::Operator::StringEquals
        );

        // Second condition should change to StringLike (wildcards introduced)
        let condition2 = &processed_conditions[1];
        assert_eq!(condition2.key, "s3:ExistingObjectTag/Environment");
        assert_eq!(condition2.values, vec!["s3.*.amazonaws.com"]);
        assert_eq!(condition2.operator, crate::enrichment::Operator::StringLike);
    }

    #[test]
    fn test_process_action_conditions_multiple_values_with_wildcards() {
        let engine = create_test_engine();

        // Create an action with a condition that has multiple values, some introducing wildcards
        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: "aws:RequestedRegion".to_string(),
                values: vec![
                    "${region}".to_string(),          // Known placeholder, no wildcards
                    "us-west-${unknown}".to_string(), // Unknown placeholder, introduces wildcards
                ],
            }],
            Explanation::default(),
        );

        let processed_conditions = engine.process_action_conditions(&action).unwrap();

        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(condition.key, "aws:RequestedRegion");
        assert_eq!(condition.values, vec!["us-east-1", "us-west-*"]);
        // Operator should be changed to StringLike because at least one value introduced wildcards
        assert_eq!(condition.operator, crate::enrichment::Operator::StringLike);
    }

    #[test]
    fn test_stringlike_operator_when_partition_region_account_is_wildcard() {
        // Test when partition is "*" - should produce StringLike
        let engine_wildcard_partition = Engine::new("*", "us-east-1", "123456789012");

        let action = Action::new(
            "s3:GetObject".to_string(),
            vec![],
            vec![Condition {
                operator: crate::enrichment::Operator::StringEquals,
                key: "s3:ExistingObjectTag/Environment".to_string(),
                values: vec!["arn:${partition}:s3:${region}:${account}:bucket/test".to_string()],
            }],
            Explanation::default(),
        );

        let processed_conditions = engine_wildcard_partition
            .process_action_conditions(&action)
            .unwrap();
        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(
            condition.values,
            vec!["arn:*:s3:us-east-1:123456789012:bucket/test"]
        );
        assert_eq!(condition.operator, crate::enrichment::Operator::StringLike);

        // Test when region is "*" - should produce StringLike
        let engine_wildcard_region = Engine::new("aws", "*", "123456789012");

        let processed_conditions = engine_wildcard_region
            .process_action_conditions(&action)
            .unwrap();
        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(
            condition.values,
            vec!["arn:aws:s3:*:123456789012:bucket/test"]
        );
        assert_eq!(condition.operator, crate::enrichment::Operator::StringLike);

        // Test when account is "*" - should produce StringLike
        let engine_wildcard_account = Engine::new("aws", "us-east-1", "*");

        let processed_conditions = engine_wildcard_account
            .process_action_conditions(&action)
            .unwrap();
        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(condition.values, vec!["arn:aws:s3:us-east-1:*:bucket/test"]);
        assert_eq!(condition.operator, crate::enrichment::Operator::StringLike);

        // Test when all are specific values - should keep StringEquals
        let engine_no_wildcards = Engine::new("aws", "us-east-1", "123456789012");

        let processed_conditions = engine_no_wildcards
            .process_action_conditions(&action)
            .unwrap();
        assert_eq!(processed_conditions.len(), 1);

        let condition = &processed_conditions[0];
        assert_eq!(
            condition.values,
            vec!["arn:aws:s3:us-east-1:123456789012:bucket/test"]
        );
        assert_eq!(
            condition.operator,
            crate::enrichment::Operator::StringEquals
        );
    }

    #[test]
    fn test_generate_policies_with_explanations() {
        use crate::enrichment::{Explanation, Operation, OperationSource, Reason};
        use std::sync::Arc;

        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        // Create enriched call with explanations
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                    ]),
                )],
                vec![],
                Explanation {
                    reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                        "s3".to_string(),
                        "get_object".to_string(),
                        OperationSource::Provided,
                    ))])],
                },
            )],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();

        // Verify policies were generated
        assert_eq!(result.policies.len(), 1);

        // Verify explanations were collected
        if let Some(explanation) = result
            .explanations
            .as_ref()
            .and_then(|explanations| explanations.explanation_for_action.get("s3:GetObject"))
        {
            assert_eq!(explanation.reasons.len(), 1);
            assert_eq!(explanation.reasons[0].operations.len(), 1);
        } else {
            panic!("Must have an explanation for s3:GetObject");
        }
    }

    #[test]
    fn test_explanation_deduplication() {
        use crate::enrichment::{Explanation, Operation, OperationSource, Reason};
        use std::sync::Arc;

        let engine = create_test_engine();
        let sdk_call1 = create_test_sdk_call();
        let sdk_call2 = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        // Create two enriched calls with duplicate explanations
        let enriched_call1 = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                    ]),
                )],
                vec![],
                Explanation {
                    reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                        "s3".to_string(),
                        "get_object".to_string(),
                        OperationSource::Provided,
                    ))])],
                },
            )],
            sdk_method_call: &sdk_call1,
        };

        let enriched_call2 = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                    ]),
                )],
                vec![],
                Explanation {
                    reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                        "s3".to_string(),
                        "get_object".to_string(),
                        OperationSource::Provided,
                    ))])],
                },
            )],
            sdk_method_call: &sdk_call2,
        };

        let result = engine
            .generate_policies(&[enriched_call1, enriched_call2])
            .unwrap();

        // Verify explanations were grouped by action with deduplicated reasons
        if let Some(explanation) = result
            .explanations
            .as_ref()
            .and_then(|explanations| explanations.explanation_for_action.get("s3:GetObject"))
        {
            assert_eq!(
                explanation.reasons.len(),
                1,
                "Duplicate reasons should be deduplicated"
            );
        } else {
            panic!("Must have an explanation for s3:GetObject");
        }
    }

    #[test]
    fn test_explanation_with_fas_expansion() {
        use crate::enrichment::{
            operation_fas_map::FasContext, Explanation, Operation, OperationSource, Reason,
        };
        use std::sync::Arc;

        let engine = create_test_engine();
        let sdk_call = create_test_sdk_call();

        // Create enriched call with FAS expansion
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![
                Action::new(
                    "s3:GetObject".to_string(),
                    vec![Resource::new(
                        "object".to_string(),
                        Some(vec![
                            "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                        ]),
                    )],
                    vec![],
                    Explanation {
                        reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                            "s3".to_string(),
                            "get_object".to_string(),
                            OperationSource::Provided,
                        ))])],
                    },
                ),
                Action::new(
                    "kms:Decrypt".to_string(),
                    vec![Resource::new(
                        "key".to_string(),
                        Some(vec![
                            "arn:${Partition}:kms:${Region}:${Account}:key/${KeyId}".to_string(),
                        ]),
                    )],
                    vec![],
                    Explanation {
                        reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                            "kms".to_string(),
                            "Decrypt".to_string(),
                            OperationSource::Fas(vec![FasContext::new(
                                "kms:ViaService".to_string(),
                                vec!["s3.us-east-1.amazonaws.com".to_string()],
                            )]),
                        ))])],
                    },
                ),
            ],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();

        // Verify explanations include FAS expansion
        assert_eq!(
            result
                .explanations
                .as_ref()
                .unwrap()
                .explanation_for_action
                .len(),
            2
        );

        // Check the FAS-expanded action
        let kms_explanation = result
            .explanations
            .as_ref()
            .unwrap()
            .explanation_for_action
            .get("kms:Decrypt")
            .expect("Should have kms:Decrypt explanation");
        assert_eq!(kms_explanation.reasons.len(), 1);
        assert_eq!(kms_explanation.reasons[0].operations.len(), 1);

        // Check that the operation has FAS context
        let operation = &kms_explanation.reasons[0].operations[0];
        assert_eq!(operation.name, "Decrypt");
        assert_eq!(operation.service, "kms");
        match &operation.source {
            OperationSource::Fas(context) => {
                assert_eq!(context.len(), 1);
                assert_eq!(context[0].key, "kms:ViaService");
                assert_eq!(context[0].values, vec!["s3.us-east-1.amazonaws.com"]);
            }
            _ => panic!("Expected FAS operation source"),
        }
    }

    #[test]
    fn test_explanation_with_possible_false_positive() {
        use crate::enrichment::{Explanation, Operation, OperationSource, Reason};
        use std::sync::Arc;

        let engine = create_test_engine();
        let sdk_call = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string(), "s3-object-lambda".to_string()],
            metadata: None,
        };

        // Create enriched call with multiple possible services (false positive flag)
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                    ]),
                )],
                vec![],
                Explanation {
                    reasons: vec![Reason::new(vec![Arc::new(Operation::new(
                        "s3".to_string(),
                        "get_object".to_string(),
                        OperationSource::Provided,
                    ))])],
                },
            )],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();

        assert_eq!(
            result
                .explanations
                .as_ref()
                .unwrap()
                .explanation_for_action
                .len(),
            1
        );
    }
}
