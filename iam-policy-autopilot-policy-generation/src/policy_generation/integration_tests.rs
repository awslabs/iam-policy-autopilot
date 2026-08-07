//! Integration tests for policy generation with enrichment module
//!
//! These tests demonstrate the complete flow from enriched method calls
//! to generated IAM policies, ensuring proper integration between modules.

#[cfg(test)]
mod tests {
    use super::super::{Effect, Engine};
    use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
    use crate::errors::ExtractorError;
    use crate::policy_generation::merge::PolicyMergerConfig;
    use crate::policy_generation::PolicyWithMetadata;
    use crate::{Explanation, SdkMethodCall};

    fn create_test_sdk_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
    }

    /// Security invariant: generated policies must NEVER contain a wildcard
    /// in any Action entry — neither a bare `"*"` nor an embedded wildcard
    /// like `"s3:*"` or `"dynamodb:Get*"`. Actions must always be fully
    /// enumerated (e.g., `"s3:GetObject"`).
    ///
    /// Resource wildcards (`"*"` in Resource) are ALLOWED and intentionally
    /// out of scope here.
    fn assert_no_wildcard_actions(policies: &[PolicyWithMetadata]) {
        // service:OperationName — a lowercase service prefix followed by an
        // alphanumeric operation name. No '*' can match anywhere.
        let strict_action_pattern = regex::Regex::new(r"^[a-z0-9-]+:[A-Za-z0-9]+$")
            .expect("static action pattern must compile");

        let mut actions_checked = 0usize;
        for policy_with_metadata in policies {
            for statement in &policy_with_metadata.policy.statements {
                assert!(
                    !statement.action.is_empty(),
                    "statement {:?} has an empty Action list",
                    statement.sid
                );
                for action in &statement.action {
                    assert!(
                        !action.contains('*'),
                        "wildcard found in Action {action:?} of statement {:?} — \
                         actions must be fully enumerated",
                        statement.sid
                    );
                    assert!(
                        strict_action_pattern.is_match(action),
                        "Action {action:?} of statement {:?} does not match the strict \
                         service:OperationName pattern",
                        statement.sid
                    );
                    actions_checked += 1;
                }
            }
        }
        assert!(
            actions_checked > 0,
            "no actions were checked — the test input produced no statements"
        );
    }

    /// Build an enriched call with a single action and standard ARN patterns.
    fn enriched_call_with_actions<'a>(
        service: &str,
        method_name: &str,
        actions: Vec<Action>,
        sdk_call: &'a SdkMethodCall,
    ) -> EnrichedSdkMethodCall<'a> {
        EnrichedSdkMethodCall {
            method_name: method_name.to_string(),
            service: service.to_string(),
            actions,
            sdk_method_call: sdk_call,
        }
    }

    /// Build a fixture spanning many SDK calls across multiple services,
    /// covering ARN-pattern resources, multi-resource actions, and
    /// wildcard-resource fallbacks (no ARN patterns).
    fn multi_service_fixture(sdk_call: &SdkMethodCall) -> Vec<EnrichedSdkMethodCall<'_>> {
        let arn_action = |name: &str, arn: &str| {
            Action::new(
                name.to_string(),
                vec![Resource::new(
                    "resource".to_string(),
                    Some(vec![arn.to_string()]),
                )],
                vec![],
                Explanation::default(),
            )
        };
        // Action whose resource has no ARN patterns → Resource "*" fallback
        let wildcard_resource_action = |name: &str| {
            Action::new(
                name.to_string(),
                vec![Resource::new("*".to_string(), None)],
                vec![],
                Explanation::default(),
            )
        };

        vec![
            enriched_call_with_actions(
                "s3",
                "get_object",
                vec![
                    arn_action(
                        "s3:GetObject",
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}",
                    ),
                    arn_action(
                        "s3:GetObjectVersion",
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}",
                    ),
                ],
                sdk_call,
            ),
            enriched_call_with_actions(
                "s3",
                "put_object",
                vec![arn_action(
                    "s3:PutObject",
                    "arn:${Partition}:s3:::${BucketName}/${ObjectName}",
                )],
                sdk_call,
            ),
            enriched_call_with_actions(
                "s3",
                "list_buckets",
                vec![wildcard_resource_action("s3:ListAllMyBuckets")],
                sdk_call,
            ),
            enriched_call_with_actions(
                "dynamodb",
                "get_item",
                vec![arn_action(
                    "dynamodb:GetItem",
                    "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}",
                )],
                sdk_call,
            ),
            enriched_call_with_actions(
                "dynamodb",
                "put_item",
                vec![arn_action(
                    "dynamodb:PutItem",
                    "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}",
                )],
                sdk_call,
            ),
            enriched_call_with_actions(
                "kms",
                "decrypt",
                vec![arn_action(
                    "kms:Decrypt",
                    "arn:${Partition}:kms:${Region}:${Account}:key/${KeyId}",
                )],
                sdk_call,
            ),
            enriched_call_with_actions(
                "ec2",
                "describe_instances",
                vec![wildcard_resource_action("ec2:DescribeInstances")],
                sdk_call,
            ),
            enriched_call_with_actions(
                "lambda",
                "invoke",
                vec![arn_action(
                    "lambda:InvokeFunction",
                    "arn:${Partition}:lambda:${Region}:${Account}:function:${FunctionName}",
                )],
                sdk_call,
            ),
            enriched_call_with_actions(
                "sqs",
                "send_message",
                vec![arn_action(
                    "sqs:SendMessage",
                    "arn:${Partition}:sqs:${Region}:${Account}:${QueueName}",
                )],
                sdk_call,
            ),
        ]
    }

    /// End-to-end regression test: even when the analyzed code uses EVERY
    /// action a service defines, the generated policy still enumerates each
    /// action individually instead of collapsing to a service wildcard.
    ///
    /// Unlike the fixture-based tests below (which feed pre-enriched calls
    /// directly into the generation engine), this test runs the real
    /// enrichment pipeline against a mocked Service Reference endpoint whose
    /// catalog for the service is exhaustive and small. Every catalog action
    /// is exercised by an SDK call, so a hypothetical "all actions used →
    /// emit service:*" optimization anywhere in enrichment, generation, or
    /// merging would fail this test.
    #[tokio::test]
    async fn test_exhaustive_action_usage_never_collapses_to_service_wildcard() {
        use std::collections::HashMap;
        use std::sync::Arc;

        use crate::enrichment::{
            mock_remote_service_reference, ResourceMatcher, ServiceReferenceLoader,
        };
        use crate::service_configuration::ServiceConfiguration;
        use crate::SdkType;

        // The COMPLETE action catalog of the mocked service: (boto3 method,
        // operation name, expected IAM action)
        let catalog = [
            ("get_object", "GetObject", "s3:GetObject"),
            ("put_object", "PutObject", "s3:PutObject"),
            ("delete_object", "DeleteObject", "s3:DeleteObject"),
            ("list_bucket", "ListBucket", "s3:ListBucket"),
            ("create_bucket", "CreateBucket", "s3:CreateBucket"),
            ("delete_bucket", "DeleteBucket", "s3:DeleteBucket"),
        ];

        // Build a Service Reference document where `catalog` is the FULL set
        // of actions the service supports
        let actions_json: Vec<_> = catalog
            .iter()
            .map(|(_, operation, _)| {
                serde_json::json!({
                    "Name": operation,
                    "Resources": [{ "Name": "bucket" }]
                })
            })
            .collect();
        let operations_json: Vec<_> = catalog
            .iter()
            .map(|(_, operation, _)| {
                serde_json::json!({
                    "Name": operation,
                    "AuthorizedActions": [{ "Name": operation, "Service": "s3" }]
                })
            })
            .collect();

        let mock_server = wiremock::MockServer::start().await;
        mock_remote_service_reference::mock_server_service_reference_response(
            &mock_server,
            "s3",
            serde_json::json!({
                "Name": "s3",
                "Actions": actions_json,
                "Resources": [
                    {
                        "Name": "bucket",
                        "ARNFormats": ["arn:${Partition}:s3:::${BucketName}"]
                    }
                ],
                "Operations": operations_json,
            }),
        )
        .await;

        let loader = ServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(mock_server.uri());

        let matcher = ResourceMatcher::new(
            Arc::new(ServiceConfiguration {
                rename_services_operation_action_map: HashMap::new(),
                rename_services_service_reference: HashMap::new(),
                smithy_botocore_service_name_mapping: HashMap::new(),
                resource_overrides: HashMap::new(),
            }),
            HashMap::new(),
            SdkType::Boto3,
            crate::DEFAULT_RESOURCE_CUTOFF,
        );

        // Use EVERY action in the catalog
        let sdk_calls: Vec<SdkMethodCall> = catalog
            .iter()
            .map(|(method, _, _)| SdkMethodCall {
                name: (*method).to_string(),
                possible_services: vec!["s3".to_string()],
                metadata: None,
            })
            .collect();

        let mut enriched_calls = Vec::new();
        for sdk_call in &sdk_calls {
            enriched_calls.extend(matcher.enrich_method_call(sdk_call, &loader).await.unwrap());
        }
        assert_eq!(
            enriched_calls.len(),
            catalog.len(),
            "every catalog action must be exercised by an enriched call"
        );

        // Generate and merge with both merger configurations
        for allow_cross_service_merging in [false, true] {
            let engine = Engine::with_merger_config(
                "aws",
                "us-east-1",
                "123456789012",
                PolicyMergerConfig {
                    allow_cross_service_merging,
                },
            );

            let result = engine.generate_policies(&enriched_calls).unwrap();
            let merged = engine.merge_policies(&result.policies).unwrap();

            assert_no_wildcard_actions(&merged);

            // Every catalog action must appear verbatim in the merged output
            let all_actions: Vec<&String> = merged
                .iter()
                .flat_map(|p| &p.policy.statements)
                .flat_map(|s| &s.action)
                .collect();
            for (_, _, expected_action) in &catalog {
                assert!(
                    all_actions.iter().any(|a| a == expected_action),
                    "action {expected_action} missing from merged output \
                     (allow_cross_service_merging={allow_cross_service_merging}); \
                     got: {all_actions:?}"
                );
            }
            assert_eq!(
                all_actions.len(),
                catalog.len(),
                "merged output must contain exactly the catalog actions, \
                 no more and no fewer \
                 (allow_cross_service_merging={allow_cross_service_merging})"
            );
        }
    }

    /// Property: a generation run over many SDK calls across multiple
    /// services never emits a wildcard Action, with or without policy
    /// merging (including cross-service merging via minimize_policy_size).
    /// The merge cases guard the merge path in merge.rs, where resource
    /// wildcard logic could conceivably be extended to actions.
    #[rstest::rstest]
    #[case::generation_only(None)]
    #[case::merged(Some(false))]
    #[case::merged_cross_service(Some(true))]
    fn test_actions_are_never_wildcards(#[case] merging: Option<bool>) {
        let sdk_call = create_test_sdk_call();

        let engine = Engine::with_merger_config(
            "aws",
            "us-east-1",
            "123456789012",
            PolicyMergerConfig {
                allow_cross_service_merging: merging.unwrap_or_default(),
            },
        );
        let enriched_calls = multi_service_fixture(&sdk_call);

        let result = engine.generate_policies(&enriched_calls).unwrap();
        let policies = match merging {
            None => result.policies,
            Some(_) => engine.merge_policies(&result.policies).unwrap(),
        };

        assert!(!policies.is_empty(), "fixture must produce policies");
        assert_no_wildcard_actions(&policies);
    }

    /// Regression test: actions with no resolvable ARN produce Resource "*"
    /// fallbacks, and that resource wildcard must never leak into the
    /// Action list.
    #[test]
    fn test_resource_wildcard_fallback_does_not_leak_into_actions() {
        let engine = Engine::new("aws", "us-east-1", "123456789012");
        let sdk_call = create_test_sdk_call();

        let enriched_call = enriched_call_with_actions(
            "s3",
            "list_buckets",
            vec![
                // Resource with no ARN patterns → per-resource "*" fallback
                Action::new(
                    "s3:ListAllMyBuckets".to_string(),
                    vec![Resource::new("*".to_string(), None)],
                    vec![],
                    Explanation::default(),
                ),
                // Action with an empty resource list → empty-list "*" fallback
                Action::new(
                    "s3:HeadBucket".to_string(),
                    vec![],
                    vec![],
                    Explanation::default(),
                ),
            ],
            &sdk_call,
        );

        let result = engine.generate_policies(&[enriched_call]).unwrap();

        // Both fallback paths must produce a wildcard *resource*...
        let policy = &result.policies[0].policy;
        assert_eq!(policy.statements.len(), 2);
        for statement in &policy.statements {
            assert_eq!(statement.resource, vec!["*"]);
        }

        // ...while actions stay fully enumerated.
        assert_no_wildcard_actions(&result.policies);

        // Both wildcard-resource statements must be surfaced as warnings
        // so consumers can call them out for review
        assert_eq!(result.warnings.len(), 2);
        for (index, warning) in result.warnings.iter().enumerate() {
            assert_eq!(warning.location.policy_index, 0);
            assert_eq!(warning.location.statement_index, index);
        }
    }

    /// Statements fully scoped to specific ARNs must produce no warnings.
    #[test]
    fn test_scoped_statements_produce_no_warnings() {
        let engine = Engine::new("aws", "us-east-1", "123456789012");
        let sdk_call = create_test_sdk_call();

        let enriched_call = enriched_call_with_actions(
            "dynamodb",
            "get_item",
            vec![Action::new(
                "dynamodb:GetItem".to_string(),
                vec![Resource::new(
                    "table".to_string(),
                    Some(vec![
                        "arn:${Partition}:dynamodb:${Region}:${Account}:table/${TableName}"
                            .to_string(),
                    ]),
                )],
                vec![],
                Explanation::default(),
            )],
            &sdk_call,
        );

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_complete_policy_generation_flow() {
        // Create policy generation engine
        let engine = Engine::new("aws", "us-east-1", "123456789012");

        // Create test enriched method call (simulating enrichment engine output)
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

        // Generate policies
        let result = engine.generate_policies(&[enriched_call]).unwrap();

        // Verify results
        assert_eq!(result.policies.len(), 1);
        let policy = &result.policies[0].policy;

        // Check policy structure
        assert_eq!(policy.version, "2012-10-17");
        assert_eq!(policy.statements.len(), 2);

        // Check first statement
        let stmt1 = &policy.statements[0];
        assert_eq!(stmt1.effect, Effect::Allow);
        assert_eq!(stmt1.action, vec!["s3:GetObject"]);
        assert_eq!(stmt1.resource, vec!["arn:aws:s3:::*/*"]);
        assert_eq!(stmt1.sid, Some("AllowS3GetObject".to_string()));

        // Check second statement
        let stmt2 = &policy.statements[1];
        assert_eq!(stmt2.effect, Effect::Allow);
        assert_eq!(stmt2.action, vec!["s3:GetObjectVersion"]);
        assert_eq!(stmt2.resource, vec!["arn:aws:s3:::*/*"]);
        assert_eq!(stmt2.sid, Some("AllowS3GetObjectVersion1".to_string()));
    }

    #[test]
    fn test_multiple_enriched_calls_generate_multiple_policies() {
        let engine = Engine::new("aws", "us-west-2", "987654321098");

        let sdk_call1 = SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        let sdk_call2 = SdkMethodCall {
            name: "put_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        };

        let enriched_calls = vec![
            EnrichedSdkMethodCall {
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
                sdk_method_call: &sdk_call1,
            },
            EnrichedSdkMethodCall {
                method_name: "put_object".to_string(),
                service: "s3".to_string(),
                actions: vec![Action::new(
                    "s3:PutObject".to_string(),
                    vec![Resource::new(
                        "object".to_string(),
                        Some(vec![
                            "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                        ]),
                    )],
                    vec![],
                    Explanation::default(),
                )],
                sdk_method_call: &sdk_call2,
            },
        ];

        let result = engine.generate_policies(&enriched_calls).unwrap();

        // Should generate one policy per enriched call
        assert_eq!(result.policies.len(), 2);

        // Check first policy
        let policy1 = &result.policies[0].policy;
        assert_eq!(policy1.statements.len(), 1);
        assert_eq!(policy1.statements[0].action, vec!["s3:GetObject"]);
        assert_eq!(policy1.statements[0].resource, vec!["arn:aws:s3:::*/*"]);

        // Check second policy
        let policy2 = &result.policies[1].policy;
        assert_eq!(policy2.statements.len(), 1);
        assert_eq!(policy2.statements[0].action, vec!["s3:PutObject"]);
        assert_eq!(policy2.statements[0].resource, vec!["arn:aws:s3:::*/*"]);
    }

    #[test]
    fn test_complex_arn_patterns_with_different_aws_contexts() {
        // Test with China partition
        let engine = Engine::new("aws-cn", "cn-north-1", "123456789012");

        let sdk_call = create_test_sdk_call();
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![
                Action::new(
                    "s3:GetObject".to_string(),
                    vec![
                        Resource::new(
                            "accesspoint".to_string(),
                            Some(vec![
                                "arn:${Partition}:s3:${Region}:${Account}:accesspoint/${AccessPointName}".to_string()
                            ])
                        ),
                        Resource::new(
                            "object".to_string(),
                            Some(vec![
                                "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string()
                            ])
                        )
                    ],
                    vec![],
                    Explanation::default(),
                )
            ],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        let policy = &result.policies[0].policy;
        let statement = &policy.statements[0];

        // Verify ARN patterns are correctly processed for China partition
        assert_eq!(
            statement.resource,
            vec![
                "arn:aws-cn:s3:cn-north-1:123456789012:accesspoint/*",
                "arn:aws-cn:s3:::*/*"
            ]
        );
    }

    #[test]
    fn test_policy_json_serialization() {
        let engine = Engine::new("aws", "us-east-1", "123456789012");

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
        let policy = &result.policies[0];

        // Test JSON serialization
        let json = serde_json::to_string_pretty(policy).unwrap();

        // Verify JSON structure (flexible formatting)
        assert!(json.contains("\"Version\": \"2012-10-17\""));
        assert!(json.contains("\"Effect\": \"Allow\""));
        assert!(json.contains("\"s3:GetObject\""));
        assert!(json.contains("\"arn:aws:s3:::*/*\""));
        assert!(json.contains("\"Sid\": \"AllowS3GetObject\""));
    }

    #[test]
    fn test_invalid_arn_pattern_handling() {
        let engine = Engine::new("aws", "us-east-1", "123456789012");

        let sdk_call = create_test_sdk_call();
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "get_object".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![Resource::new(
                    "object".to_string(),
                    Some(vec![
                        "arn:${Partition}:s3:${}:bucket/${ObjectName}".to_string()
                    ]), // Invalid empty placeholder
                )],
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        };

        // Should fail due to empty placeholder
        let result = engine.generate_policies(&[enriched_call]);
        assert!(result.is_err());

        if let Err(ExtractorError::PolicyGeneration { message, .. }) = result {
            assert!(message.contains("empty placeholder"));
        } else {
            panic!("Expected PolicyGeneration error for invalid ARN pattern");
        }
    }

    #[test]
    fn test_no_arn_patterns_fallback_to_wildcard() {
        let engine = Engine::new("aws", "us-east-1", "123456789012");

        let sdk_call = create_test_sdk_call();
        let enriched_call = EnrichedSdkMethodCall {
            method_name: "list_buckets".to_string(),
            service: "s3".to_string(),
            actions: vec![Action::new(
                "s3:ListAllMyBuckets".to_string(),
                vec![Resource::new("*".to_string(), None)], // No ARN patterns
                vec![],
                Explanation::default(),
            )],
            sdk_method_call: &sdk_call,
        };

        let result = engine.generate_policies(&[enriched_call]).unwrap();
        let policy = &result.policies[0].policy;
        let statement = &policy.statements[0];

        // Should fallback to wildcard resource
        assert_eq!(statement.resource, vec!["*"]);
        assert_eq!(statement.action, vec!["s3:ListAllMyBuckets"]);
    }
}
