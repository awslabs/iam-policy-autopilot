//! Integration tests for policy generation with enrichment module
//!
//! These tests demonstrate the complete flow from enriched method calls
//! to generated IAM policies, ensuring proper integration between modules.

#[cfg(test)]
mod tests {
    use super::super::{Effect, Engine};
    use crate::enrichment::{Action, EnrichedSdkMethodCall, Resource};
    use crate::errors::ExtractorError;
    use crate::{Explanation, SdkMethodCall};

    fn create_test_sdk_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: None,
        }
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
                    vec![Resource::new(Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                    ]))],
                    vec![],
                    Explanation::default(),
                ),
                Action::new(
                    "s3:GetObjectVersion".to_string(),
                    vec![Resource::new(Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                    ]))],
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
                    vec![Resource::new(Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                    ]))],
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
                    vec![Resource::new(Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                    ]))],
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
            actions: vec![Action::new(
                "s3:GetObject".to_string(),
                vec![
                    Resource::new(Some(vec![
                        "arn:${Partition}:s3:${Region}:${Account}:accesspoint/${AccessPointName}"
                            .to_string(),
                    ])),
                    Resource::new(Some(vec![
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                    ])),
                ],
                vec![],
                Explanation::default(),
            )],
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
                vec![Resource::new(Some(vec![
                    "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
                ]))],
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
                vec![Resource::new(None)], // No ARN patterns
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
