//! TypeScript AWS SDK extractor implementation
//!
//! This module provides the TypeScriptExtractor that implements the Extractor trait.

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::TypeScript;
use async_trait::async_trait;
use std::collections::HashSet;

use crate::extraction::extractor::{Extractor, ExtractorResult};
use crate::extraction::javascript::scanner::ASTScanner;
use crate::extraction::javascript::shared::ExtractionUtils;
use crate::extraction::AstWithSourceFile;
use crate::{ServiceModelIndex, SourceFile};

/// TypeScript extractor for AWS SDK method calls
pub(crate) struct TypeScriptExtractor;

impl TypeScriptExtractor {
    /// Create a new TypeScript extractor instance
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for TypeScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Extractor for TypeScriptExtractor {
    async fn parse(&self, source_file: &SourceFile) -> ExtractorResult {
        // Create AST once and reuse it
        let ast_grep = TypeScript.ast_grep(&source_file.content);
        let ast = AstWithSourceFile::new(ast_grep, source_file.clone());

        // Create scanner with the pre-built AST
        let mut scanner = ASTScanner::new(ast.clone(), TypeScript.into());

        let scan_results = match scanner.scan_all() {
            Ok(results) => results,
            Err(e) => {
                // Log error and return empty results
                log::warn!("TypeScript scanning failed: {e}");
                return ExtractorResult::TypeScript(ast, Vec::new());
            }
        };

        // Extract operations from imported types and method calls using shared utilities
        let mut method_calls =
            ExtractionUtils::extract_operations_from_imports(&scan_results, &mut scanner);

        // Also extract operations from direct client method calls
        method_calls.extend(ExtractionUtils::extract_operations_from_method_calls(
            &scan_results,
        ));

        // Return TypeScript variant with the same AST
        ExtractorResult::TypeScript(ast, method_calls)
    }

    fn filter_map(
        &self,
        extractor_results: &mut [ExtractorResult],
        service_index: &ServiceModelIndex,
    ) {
        for extractor_result in extractor_results.iter_mut() {
            let method_calls = if let ExtractorResult::TypeScript(_ast, method_calls) =
                extractor_result
            {
                method_calls
            } else {
                // This shouldn't happen in TypeScript extractor
                log::warn!("Received non-TypeScript result during TypeScript method extraction.");
                continue;
            };

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
                    false
                }
            });

            // Then: Deduplicate by (operation_name, service) pairs
            // TypeScript SDK v3 may extract the same operation from multiple sources
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
        // TypeScript imports are unambiguous due to AWS SDK v3 modular structure
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

    fn create_source_file(source_code: &str) -> SourceFile {
        SourceFile::with_language(
            std::path::PathBuf::new(),
            source_code.to_string(),
            crate::Language::TypeScript,
        )
    }

    #[tokio::test]
    async fn test_parse_typescript_with_types() {
        let extractor = TypeScriptExtractor::new();
        let typescript_code = r#"
// TypeScript with type annotations and interfaces
import { DynamoDBClient, QueryCommand } from '@aws-sdk/client-dynamodb';
import { paginateQuery, QueryCommandInput } from '@aws-sdk/lib-dynamodb';

interface User {
  id: string;
  name: string;
  email: string;
}

type QueryParams = QueryCommandInput & {
  TableName: string;
};

const client: DynamoDBClient = new DynamoDBClient({ region: 'us-east-1' });

async function queryUsers(): Promise<User[]> {
  const params: QueryParams = {
    TableName: 'Users',
    KeyConditionExpression: 'pk = :pk',
    ExpressionAttributeValues: {
      ':pk': 'USER'
    }
  };

  const command = new QueryCommand(params);
  const result = await client.send(command);
  return result.Items as User[];
}
        "#;

        let result = extractor.parse(&create_source_file(typescript_code)).await;

        // Verify TypeScript AST is returned
        match result {
            ExtractorResult::TypeScript(_ast, method_calls) => {
                assert!(
                    !method_calls.is_empty(),
                    "Should find method calls from TypeScript imports"
                );

                // Should find Query operation from both QueryCommand and paginateQuery imports
                let query_ops: Vec<_> = method_calls
                    .iter()
                    .filter(|call| call.name == "Query")
                    .collect();

                assert!(
                    !query_ops.is_empty(),
                    "Should find Query operations from TypeScript imports"
                );

                // Verify service association
                for query_op in query_ops {
                    assert!(query_op.possible_services.contains(&"dynamodb".to_string()));
                }

                println!("✅ Found {} operations from TypeScript", method_calls.len());
                for call in &method_calls {
                    println!("  - {} (service: {:?})", call.name, call.possible_services);
                }
            }
            _ => {
                panic!("Should return TypeScript result");
            }
        }
    }

    #[tokio::test]
    async fn test_parse_typescript_with_generics() {
        let extractor = TypeScriptExtractor::new();
        let typescript_code = r#"
// TypeScript with generics and advanced type features
import { S3Client, GetObjectCommand, PutObjectCommand } from '@aws-sdk/client-s3';

interface S3Service<T> {
  get(key: string): Promise<T>;
  put(key: string, data: T): Promise<void>;
}

class MyS3Service<T> implements S3Service<T> {
  private client: S3Client;

  constructor(region: string) {
    this.client = new S3Client({ region });
  }

  async get(key: string): Promise<T> {
    const command = new GetObjectCommand({
      Bucket: 'my-bucket',
      Key: key
    });
    
    const result = await this.client.send(command);
    return JSON.parse(await result.Body?.transformToString() || '{}') as T;
  }

  async put(key: string, data: T): Promise<void> {
    const command = new PutObjectCommand({
      Bucket: 'my-bucket',
      Key: key,
      Body: JSON.stringify(data)
    });
    
    await this.client.send(command);
  }
}
        "#;

        let result = extractor.parse(&create_source_file(typescript_code)).await;

        // Verify TypeScript extraction with generics
        match result {
            ExtractorResult::TypeScript(_ast, method_calls) => {
                assert!(
                    !method_calls.is_empty(),
                    "Should find method calls from TypeScript with generics"
                );

                // Should find GetObject and PutObject operations (PascalCase)
                let get_object_op = method_calls
                    .iter()
                    .find(|call| call.name == "GetObject")
                    .expect("Should find GetObject operation");

                let put_object_op = method_calls
                    .iter()
                    .find(|call| call.name == "PutObject")
                    .expect("Should find PutObject operation");

                // Both should be associated with s3 service
                assert_eq!(get_object_op.possible_services, vec!["s3"]);
                assert_eq!(put_object_op.possible_services, vec!["s3"]);

                println!(
                    "✅ Found {} operations from TypeScript with generics",
                    method_calls.len()
                );
                for call in &method_calls {
                    println!("  - {} (service: {:?})", call.name, call.possible_services);
                }
            }
            _ => {
                panic!("Should return TypeScript result");
            }
        }
    }

    #[tokio::test]
    async fn test_fallback_to_all_services_when_import_service_invalid() {
        use crate::extraction::sdk_model::ServiceDiscovery;
        use crate::Language;

        let extractor = TypeScriptExtractor::new();

        // This code imports from a non-existent service but uses a valid operation
        let typescript_code = r#"
import { FakeClient, GetObjectCommand } from '@aws-sdk/client-fake-service';

const client = new FakeClient({ region: 'us-east-1' });
const command = new GetObjectCommand({ Bucket: 'test', Key: 'test.txt' });
        "#;

        // Parse the code
        let mut results = vec![extractor.parse(&create_source_file(typescript_code)).await];

        // Build service index with all services for testing
        let service_index = ServiceDiscovery::load_service_index(Language::TypeScript)
            .await
            .expect("Failed to load service index");

        // Apply filter_map which should trigger the fallback
        extractor.filter_map(&mut results, &service_index);

        // Verify the results
        match &results[0] {
            ExtractorResult::TypeScript(_ast, method_calls) => {
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
                panic!("Should return TypeScript result");
            }
        }
    }

    #[tokio::test]
    async fn test_waiter_disambiguation_with_multiple_services() {
        use crate::extraction::sdk_model::ServiceDiscovery;
        use crate::Language;

        let extractor = TypeScriptExtractor::new();

        // Code that imports Neptune client and uses DBInstanceAvailable waiter
        let typescript_code = r#"
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
        let mut results = vec![extractor.parse(&create_source_file(typescript_code)).await];

        // Load service index
        let service_index = ServiceDiscovery::load_service_index(Language::TypeScript)
            .await
            .expect("Failed to load service index");

        // Apply filter_map which includes waiter resolution
        extractor.filter_map(&mut results, &service_index);

        // Verify the results
        match &results[0] {
            ExtractorResult::TypeScript(_ast, method_calls) => {
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
            _ => panic!("Should return TypeScript result"),
        }
    }
}
