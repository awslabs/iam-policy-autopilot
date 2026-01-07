//! JavaScript AWS SDK extractor implementation
//!
//! This module provides the JavaScriptExtractor that implements the Extractor trait.

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::JavaScript;
use async_trait::async_trait;
use std::collections::HashSet;

use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::javascript::scanner::ASTScanner;
use crate::extraction::javascript::shared::ExtractionUtils;
use crate::ServiceModelIndex;

/// JavaScript extractor for AWS SDK method calls
pub(crate) struct JavaScriptExtractor;

impl JavaScriptExtractor {
    /// Create a new JavaScript extractor instance
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for JavaScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Extractor for JavaScriptExtractor {
    async fn parse(&self, source_code: &str) -> ExtractorResult {
        // Create AST once and reuse it
        let ast = JavaScript.ast_grep(source_code);

        // Create scanner with the pre-built AST
        let mut scanner = ASTScanner::new(ast.clone(), JavaScript.into());

        let scan_results = match scanner.scan_all() {
            Ok(results) => results,
            Err(e) => {
                // Log error and return empty results
                log::warn!("JavaScript scanning failed: {}", e);
                return ExtractorResult::JavaScript(ast, Vec::new());
            }
        };

        // Extract operations from imported types and method calls using shared utilities
        let mut method_calls =
            ExtractionUtils::extract_operations_from_imports(&scan_results, &mut scanner);

        // Also extract operations from direct client method calls
        method_calls.extend(ExtractionUtils::extract_operations_from_method_calls(
            &scan_results,
        ));

        // Return JavaScript variant with the same AST
        ExtractorResult::JavaScript(ast, method_calls)
    }

    fn filter_map(
        &self,
        extractor_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    ) {
        for extractor_result in extractor_results.iter_mut() {
            let method_calls = match extractor_result {
                ExtractorResult::JavaScript(_ast, method_calls) => method_calls,
                _ => {
                    // This shouldn't happen in JavaScript extractor
                    log::warn!(
                        "Received non-JavaScript result during JavaScript method extraction."
                    );
                    continue;
                }
            };

            // First: Resolve waiter names to actual operations
            // For each call, check if it's a waiter name and replace with the actual operation
            for call in method_calls.iter_mut() {
                if let Some(service_methods) = service_index.waiter_lookup.get(&call.name) {
                    let matching_method = service_methods
                        .iter()
                        .find(|sm| call.possible_services.contains(&sm.service_name));

                    if let Some(method) = matching_method {
                        call.name = method.operation_name.clone();
                    } else {
                        log::warn!(
                            "Waiter '{}' found in services {:?} but imported from {:?}",
                            call.name,
                            service_methods
                                .iter()
                                .map(|sm| &sm.service_name)
                                .collect::<Vec<_>>(),
                            call.possible_services
                        );
                    }
                }
            }

            // Second: Validate method calls against service index
            method_calls.retain_mut(|call| {
                // Check if this method name exists in the SDK
                if let Some(service_refs) = service_index.method_lookup.get(&call.name) {
                    // Get valid services for this method from the service index
                    let valid_services: HashSet<String> = service_refs.iter()
                        .map(|service_ref| service_ref.service_name.clone())
                        .collect();

                    // Filter possible_services to only include services that actually contain this method
                    call.possible_services.retain(|service| valid_services.contains(service));

                    // FALLBACK: If no services matched from import, use all valid services for this operation
                    if call.possible_services.is_empty() {
                        log::debug!(
                            "Import-derived service(s) don't contain operation '{}'. Using all {} valid service(s) as fallback.",
                            call.name,
                            valid_services.len()
                        );
                        call.possible_services = valid_services.into_iter().collect();
                    }

                    // Keep method call - it now has at least one valid service
                    true
                } else {
                    // Method name doesn't exist in SDK - filter it out
                    log::warn!("Filtering out {}", call.name);
                    false
                }
            });

            // Then: Deduplicate by (operation_name, service) pairs
            // JavaScript SDK v3 may extract the same operation from multiple sources
            // (e.g., QueryCommandInput and paginateQuery both infer Query operation)
            let mut seen = HashSet::new();
            method_calls.retain(|call| {
                // Create a key from operation name and all possible services
                let key = (call.name.clone(), call.possible_services.clone());
                seen.insert(key)
            });
        }
    }

    fn disambiguate(
        &self,
        _extractor_results: &mut [ExtractorResult],
        _service_index: &ServiceModelIndex,
    ) {
        // JavaScript imports are unambiguous due to AWS SDK v3 modular structure
        // @aws-sdk/client-{service} imports clearly indicate the target service
        // No disambiguation needed - pass through all operations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::Parameter;
    use std::collections::HashMap;

    #[test]
    fn test_extract_service_from_sublibrary() {
        // Test using shared utilities
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("client-s3"),
            Some("s3".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("lib-dynamodb"),
            Some("dynamodb".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("client-lambda"),
            Some("lambda".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("other"),
            None
        );
    }

    #[test]
    fn test_convert_arguments_to_parameters() {
        // Test using shared utilities
        let mut arguments = HashMap::new();
        arguments.insert("Bucket".to_string(), "my-bucket".to_string());
        arguments.insert("Key".to_string(), "my-key".to_string());

        let parameters = ExtractionUtils::convert_arguments_to_parameters(&arguments);
        assert_eq!(parameters.len(), 2);

        // Check that both parameters are keyword parameters
        for param in &parameters {
            match param {
                Parameter::Keyword {
                    name,
                    value,
                    position,
                    ..
                } => {
                    assert!(name == "Bucket" || name == "Key");
                    assert!(value.as_string() == "my-bucket" || value.as_string() == "my-key");
                    assert!(*position < 2);
                }
                _ => panic!("Expected keyword parameter"),
            }
        }
    }

    #[tokio::test]
    async fn test_parse_import_via_require() {
        let extractor = JavaScriptExtractor::new();
        let source_code = r#"
// Import via require function, interaction via send()
const { S3Client, CreateBucketCommand } = require("@aws-sdk/client-s3");
const s3Client = new S3Client({ region: "us-east-1" });
async function createMyBucket() {
  const command = new CreateBucketCommand({ Bucket: "my-bucket-name" });
  try {
    const data = await s3Client.send(command);
    console.log("Bucket created:", data);
  } catch (error) {
    console.error("Error creating bucket:", error);
  }
}
createMyBucket();
        "#;

        let result = extractor.parse(source_code).await;
        let method_calls = result.method_calls_ref();

        // Should infer CreateBucket operation from CreateBucketCommand import
        assert!(
            !method_calls.is_empty(),
            "Should find method calls from imported Command types"
        );

        // Should find CreateBucket operation inferred from CreateBucketCommand
        let create_bucket_op = method_calls
            .iter()
            .find(|call| call.name == "CreateBucket")
            .expect("Should find CreateBucket operation from CreateBucketCommand import");

        // Should be associated with s3 service (from client-s3 sublibrary)
        assert_eq!(
            create_bucket_op.possible_services,
            vec!["s3"],
            "Should associate with s3 service"
        );

        println!(
            "✅ Found {} operations inferred from imports",
            method_calls.len()
        );
        for call in method_calls {
            let empty_params = Vec::new();
            let params = call
                .metadata
                .as_ref()
                .map(|m| &m.parameters)
                .unwrap_or(&empty_params);
            println!(
                "  - {} (service: {:?}, params: {} args)",
                call.name,
                call.possible_services,
                params.len()
            );
        }
    }

    #[tokio::test]
    async fn test_parse_import_via_require_and_paginator() {
        let extractor = JavaScriptExtractor::new();
        let source_code = r#"
// Paginator
const { DynamoDBClient, paginateListTables } = require("@aws-sdk/client-dynamodb");

async function getAllDynamoDBTables() {
    const client = new DynamoDBClient({ region: "an-aws-region" });
    const paginatorConfig = { client };

    let allTableNames = [];

    try {
        for await (const page of paginateListTables(paginatorConfig, {})) {
            if (page.TableNames) {
                allTableNames = allTableNames.concat(page.TableNames);
            }
        }
        console.log("All DynamoDB Table Names:", allTableNames);
        return allTableNames;
    } catch (error) {
        console.error("Error listing tables:", error);
        throw error;
    }
}

getAllDynamoDBTables();
        "#;

        let result = extractor.parse(source_code).await;
        let method_calls = result.method_calls_ref();

        // Should infer ListTables operation from paginateListTables import
        assert!(
            !method_calls.is_empty(),
            "Should find operations from paginate function imports"
        );

        // Should find ListTables operation inferred from paginateListTables
        let list_tables_op = method_calls
            .iter()
            .find(|call| call.name == "ListTables")
            .expect("Should find ListTables operation from paginateListTables import");

        // Should be associated with dynamodb service (from client-dynamodb sublibrary)
        assert_eq!(
            list_tables_op.possible_services,
            vec!["dynamodb"],
            "Should associate with dynamodb service"
        );

        println!(
            "✅ Found {} operations inferred from paginate imports",
            method_calls.len()
        );
    }

    #[tokio::test]
    async fn test_parse_low_level_client_access() {
        let extractor = JavaScriptExtractor::new();
        let source_code = r#"
// stream pattern
import { S3 } from "@aws-sdk/client-s3";

const client = new S3({region: REGION});

const anotherBucket = "another-bucket"
const getObjectResult = await client.getObject({
  Bucket: anotherBucket,
  Key: "another-key",
});

// env-specific stream with added mixin methods.
const bodyStream = getObjectResult.Body;

// one-time transform.
const bodyAsString = await bodyStream.transformToString();

// throws an error on 2nd call, stream cannot be rewound.
const __error__ = await bodyStream.transformToString();
        "#;

        let result = extractor.parse(source_code).await;
        let method_calls = result.method_calls_ref();

        // Should find GetObject operation from direct client method call
        // Note: This currently may not work since scanner.method_calls is empty in scan_all()

        if method_calls.is_empty() {
            panic!("method_calls must not be empty")
        }

        // Should find GetObject operation from client.getObject() call
        let get_object_op = method_calls
            .iter()
            .find(|call| call.name == "GetObject")
            .expect("Should find GetObject operation from client.getObject() call");

        // Should be associated with s3 service (from client-s3 sublibrary)
        assert_eq!(
            get_object_op.possible_services,
            vec!["s3"],
            "Should associate with s3 service"
        );

        println!(
            "✅ Found {} operations from direct client method calls",
            method_calls.len()
        );
    }

    #[tokio::test]
    async fn test_fallback_to_all_services_when_import_service_invalid() {
        use crate::extraction::sdk_model::ServiceDiscovery;
        use crate::Language;

        let extractor = JavaScriptExtractor::new();

        // This code imports from a non-existent service but uses a valid operation
        let javascript_code = r#"
const { FakeClient, GetObjectCommand } = require("@aws-sdk/client-fake-service");

const client = new FakeClient({ region: 'us-east-1' });
const command = new GetObjectCommand({ Bucket: 'test', Key: 'test.txt' });
        "#;

        // Parse the code
        let mut results = vec![extractor.parse(javascript_code).await];

        // Build service index with all services for testing
        let service_index = ServiceDiscovery::load_service_index(Language::JavaScript)
            .await
            .expect("Failed to load service index");

        // Apply filter_map which should trigger the fallback
        extractor.filter_map(&mut results, &service_index);

        // Verify the results
        match &results[0] {
            ExtractorResult::JavaScript(_ast, method_calls) => {
                // Should find GetObject operation
                let get_object_calls: Vec<_> = method_calls
                    .iter()
                    .filter(|call| call.name == "GetObject")
                    .collect();

                assert!(
                    !get_object_calls.is_empty(),
                    "Should find GetObject operation"
                );

                // The fallback should have populated possible_services with all services that have GetObject
                // (both s3 and glacier have GetObject operation)
                for call in get_object_calls {
                    assert!(
                        !call.possible_services.is_empty(),
                        "Fallback should populate possible_services with valid services"
                    );

                    // Should contain at least s3 (GetObject is in s3)
                    assert!(
                        call.possible_services.contains(&"s3".to_string()),
                        "Should include s3 as a valid service for GetObject"
                    );

                    println!(
                        "✅ Fallback worked! GetObject associated with services: {:?}",
                        call.possible_services
                    );
                }
            }
            _ => {
                panic!("Should return JavaScript result");
            }
        }
    }

    #[tokio::test]
    async fn test_waiter_disambiguation_with_multiple_services() {
        use crate::extraction::sdk_model::ServiceDiscovery;
        use crate::Language;

        let extractor = JavaScriptExtractor::new();

        // Code that imports Neptune client and uses DBInstanceAvailable waiter
        let code = r#"
    import { NeptuneClient, waitUntilDBInstanceAvailable } from '@aws-sdk/client-neptune';

    const client = new NeptuneClient({ region: 'us-east-1' });

    async function waitForInstance() {
        await waitUntilDBInstanceAvailable(
            { client, maxWaitTime: 300 },
            { DBInstanceIdentifier: 'my-neptune-instance' }
        );
    }
        "#;

        // Parse the code
        let mut results = vec![extractor.parse(code).await];

        // Load service index
        let service_index = ServiceDiscovery::load_service_index(Language::JavaScript)
            .await
            .expect("Failed to load service index");

        // Apply filter_map which includes waiter resolution
        extractor.filter_map(&mut results, &service_index);

        // Verify the results
        match &results[0] {
            ExtractorResult::JavaScript(_ast, method_calls) => {
                // Find the DBInstanceAvailable call
                let db_instance_call = method_calls
                    .iter()
                    .find(|call| call.name == "DescribeDBInstances")
                    .expect("Should find DescribeDBInstances operation after waiter resolution");

                // CRITICAL: Should be associated with Neptune, not RDS or DocumentDB
                assert!(
                    db_instance_call
                        .possible_services
                        .contains(&"neptune".to_string()),
                    "DBInstanceAvailable waiter should resolve to Neptune service, got: {:?}",
                    db_instance_call.possible_services
                );

                // Should NOT contain RDS or DocumentDB
                assert!(
                    !db_instance_call
                        .possible_services
                        .contains(&"rds".to_string()),
                    "Should not incorrectly resolve to RDS"
                );
                assert!(
                    !db_instance_call
                        .possible_services
                        .contains(&"docdb".to_string()),
                    "Should not incorrectly resolve to DocumentDB"
                );
            }
            _ => panic!("Should return JavaScript result"),
        }
    }
}
