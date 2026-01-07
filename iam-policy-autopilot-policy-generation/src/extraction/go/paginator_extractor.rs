//! Paginator method extraction for Go AWS SDK using ast-grep
//!
//! This module handles extraction of Go AWS SDK v2 paginator patterns by detecting
//! paginator creation calls, which contain the meaningful parameters for IAM policy generation.

use crate::extraction::go::utils;
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::{Parameter, SdkMethodCall, SdkMethodCallMetadata};
use crate::Language;
use crate::ServiceModelIndex;
use ast_grep_language::Go;

/// Information about a discovered paginator creation call
#[derive(Debug, Clone)]
pub(crate) struct PaginatorInfo {
    /// Variable name assigned to the paginator (e.g., "paginator", "instancePaginator")
    // TODO: use the name of the paginator in analysis
    #[allow(dead_code)]
    pub variable_name: String,
    /// Operation name (e.g., "ListObjectsV2")
    pub paginator_type: String,
    /// Client receiver variable name (e.g., "client", "s3Client")
    pub client_receiver: String,
    /// Extracted arguments from paginator creation (input struct)
    pub creation_arguments: Vec<Parameter>,
    /// Line number where paginator was created
    pub creation_line: usize,
}

/// Information about a chained paginator call
#[derive(Debug, Clone)]
pub(crate) struct ChainedPaginatorCallInfo {
    /// Operation name (e.g., "ListObjectsV2")
    pub paginator_type: String,
    /// Client receiver variable name (e.g., "client", "s3Client")
    pub client_receiver: String,
    /// Extracted arguments from paginator creation (input struct)
    pub arguments: Vec<Parameter>,
    /// Line number where chained call was made
    #[allow(dead_code)]
    pub line: usize,
    /// Start position of the chained call node
    pub start_position: (usize, usize),
    /// End position of the chained call node
    pub end_position: (usize, usize),
}

/// Extractor for Go AWS SDK paginator patterns
///
/// This extractor discovers paginator patterns in Go code and creates synthetic
/// SdkMethodCall objects that represent the actual AWS operations being paginated.
///
/// Go paginator patterns:
/// 1. `paginator := service.NewListObjectsV2Paginator(client, &service.ListObjectsV2Input{...})`
/// 2. `service.NewListObjectsV2Paginator(client, &service.ListObjectsV2Input{...}).NextPage(ctx)` (chained)
pub(crate) struct GoPaginatorExtractor<'a> {
    service_index: &'a ServiceModelIndex,
}

impl<'a> GoPaginatorExtractor<'a> {
    /// Create a new Go paginator extractor
    pub(crate) fn new(service_index: &'a ServiceModelIndex) -> Self {
        Self { service_index }
    }

    /// Extract paginator method calls from the AST
    pub(crate) fn extract_paginator_method_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Vec<SdkMethodCall> {
        let mut synthetic_calls = Vec::new();

        // Create synthetic calls from paginator creations
        let paginators = self.find_paginator_creation_calls(ast);
        for paginator in &paginators {
            let synthetic_call = self.create_synthetic_call_from_creation(paginator);
            synthetic_calls.push(synthetic_call);
        }

        // Create synthetic calls from chained paginator calls
        let chained_calls = self.find_chained_paginator_calls(ast);
        for chained_call in chained_calls {
            let synthetic_call = self.create_chained_synthetic_call(&chained_call);
            synthetic_calls.push(synthetic_call);
        }

        synthetic_calls
    }

    /// Find all paginator creation calls (NewXxxPaginator functions)
    fn find_paginator_creation_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Vec<PaginatorInfo> {
        let root = ast.root();
        let mut paginators = Vec::new();

        // Pattern: $VAR := $PACKAGE.$FUNCTION($$$ARGS) where FUNCTION contains "New" and "Paginator"
        let paginator_pattern = "$VAR := $PACKAGE.$FUNCTION($$$ARGS)";

        for node_match in root.find_all(paginator_pattern) {
            if let Some(paginator_info) = self.parse_paginator_creation_call(&node_match) {
                paginators.push(paginator_info);
            }
        }

        paginators
    }

    /// Find all chained paginator calls
    fn find_chained_paginator_calls(
        &self,
        ast: &ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Vec<ChainedPaginatorCallInfo> {
        let root = ast.root();
        let mut chained_calls = Vec::new();

        // Pattern: $PACKAGE.$FUNCTION($$$ARGS).NextPage($$$NEXT_ARGS)
        let chained_pattern = "$PACKAGE.$FUNCTION($$$ARGS).NextPage($$$NEXT_ARGS)";

        for node_match in root.find_all(chained_pattern) {
            if let Some(chained_info) = self.parse_chained_paginator_call(&node_match) {
                chained_calls.push(chained_info);
            }
        }

        chained_calls
    }

    /// Parse a paginator creation call
    fn parse_paginator_creation_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Option<PaginatorInfo> {
        let env = node_match.get_env();

        // Extract variable name
        let variable_name = env.get_match("VAR")?.text().to_string();

        // Extract function name
        let function_name = env.get_match("FUNCTION")?.text();

        // Check if this is a paginator creation call (contains "New" and "Paginator")
        if !function_name.contains("New") || !function_name.ends_with("Paginator") {
            return None;
        }

        // Extract client parameter from arguments (first argument)
        let args_nodes = env.get_multiple_matches("ARGS");
        let client_receiver = if let Some(first_arg) = args_nodes.first() {
            first_arg.text().to_string()
        } else {
            return None; // Paginator creation should have at least one argument (the client)
        };

        // Extract creation arguments (skip client, get input struct)
        let creation_arguments = if args_nodes.len() > 1 {
            utils::extract_arguments(&args_nodes[1..])
        } else {
            Vec::new()
        };

        // Extract operation name from function name (remove "New" prefix and "Paginator" suffix)
        // e.g., "NewListObjectsV2Paginator" -> "ListObjectsV2"
        let operation_name = function_name
            .strip_prefix("New")
            .and_then(|s| s.strip_suffix("Paginator"));

        if let Some(operation_name) = operation_name {
            let creation_line = node_match.get_node().start_pos().line() + 1;

            return Some(PaginatorInfo {
                variable_name,
                paginator_type: operation_name.to_string(),
                client_receiver,
                creation_arguments,
                creation_line,
            });
        }

        None
    }

    /// Parse a chained paginator call
    fn parse_chained_paginator_call(
        &self,
        node_match: &ast_grep_core::NodeMatch<ast_grep_core::tree_sitter::StrDoc<Go>>,
    ) -> Option<ChainedPaginatorCallInfo> {
        let env = node_match.get_env();

        // Extract function name
        let function_name = env.get_match("FUNCTION")?.text();

        // Check if this is a paginator creation call
        if !function_name.contains("New") || !function_name.ends_with("Paginator") {
            return None;
        }

        // Extract operation name from function name (remove "New" prefix and "Paginator" suffix)
        let paginator_type = function_name
            .strip_prefix("New")
            .and_then(|s| s.strip_suffix("Paginator"))?;
        let paginator_type = paginator_type.to_string();

        // Extract client parameter from creation arguments (first argument)
        let args_nodes = env.get_multiple_matches("ARGS");
        let client_receiver = if let Some(first_arg) = args_nodes.first() {
            first_arg.text().to_string()
        } else {
            return None;
        };

        // Extract creation arguments (skip client, get input struct)
        let creation_arguments = if args_nodes.len() > 1 {
            utils::extract_arguments(&args_nodes[1..])
        } else {
            Vec::new()
        };

        // Get position information
        let node = node_match.get_node();
        let start = node.start_pos();
        let end = node.end_pos();

        Some(ChainedPaginatorCallInfo {
            paginator_type,
            client_receiver,
            arguments: creation_arguments,
            line: start.line() + 1,
            start_position: (start.line() + 1, start.column(node) + 1),
            end_position: (end.line() + 1, end.column(node) + 1),
        })
    }

    /// Create a synthetic SdkMethodCall from paginator creation
    fn create_synthetic_call_from_creation(&self, paginator_info: &PaginatorInfo) -> SdkMethodCall {
        // paginator_type already contains the clean operation name (e.g., "ListObjectsV2")
        let operation_name = &paginator_info.paginator_type;

        // Convert to method name using Go language conventions
        let method_name = ServiceDiscovery::operation_to_method_name(operation_name, Language::Go);

        // Look up all services that provide this method
        let possible_services =
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_name) {
                service_refs
                    .iter()
                    .map(|service_ref| service_ref.service_name.clone())
                    .collect()
            } else {
                Vec::new()
            };

        SdkMethodCall {
            name: method_name,
            possible_services,
            metadata: Some(SdkMethodCallMetadata {
                parameters: paginator_info.creation_arguments.clone(),
                return_type: None,
                start_position: (paginator_info.creation_line, 1),
                end_position: (paginator_info.creation_line, 1),
                receiver: Some(paginator_info.client_receiver.clone()),
            }),
        }
    }

    /// Create a synthetic SdkMethodCall from a chained paginator call
    fn create_chained_synthetic_call(
        &self,
        chained_call: &ChainedPaginatorCallInfo,
    ) -> SdkMethodCall {
        // paginator_type already contains the clean operation name (e.g., "ListObjectsV2")
        let operation_name = &chained_call.paginator_type;

        // Convert to method name using Go language conventions
        let method_name = ServiceDiscovery::operation_to_method_name(operation_name, Language::Go);

        // Look up all services that provide this method
        let possible_services =
            if let Some(service_refs) = self.service_index.method_lookup.get(&method_name) {
                service_refs
                    .iter()
                    .map(|service_ref| service_ref.service_name.clone())
                    .collect()
            } else {
                Vec::new()
            };

        SdkMethodCall {
            name: method_name,
            possible_services,
            metadata: Some(SdkMethodCallMetadata {
                parameters: chained_call.arguments.clone(),
                return_type: None,
                start_position: chained_call.start_position,
                end_position: chained_call.end_position,
                receiver: Some(chained_call.client_receiver.clone()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::Go;
    use std::collections::HashMap;

    fn create_test_ast(
        source_code: &str,
    ) -> ast_grep_core::AstGrep<ast_grep_core::tree_sitter::StrDoc<Go>> {
        Go.ast_grep(source_code)
    }

    fn create_test_service_index() -> ServiceModelIndex {
        use crate::extraction::sdk_model::{
            Operation, SdkServiceDefinition, ServiceMetadata, ServiceMethodRef, Shape,
        };

        let mut services = HashMap::new();
        let mut method_lookup = HashMap::new();

        // Create S3 service with ListObjectsV2 operation
        let mut s3_operations = HashMap::new();
        let mut s3_shapes = HashMap::new();

        s3_operations.insert(
            "ListObjectsV2".to_string(),
            Operation {
                name: "ListObjectsV2".to_string(),
                input: Some(crate::extraction::sdk_model::ShapeReference {
                    shape: "ListObjectsV2Request".to_string(),
                }),
            },
        );

        let mut list_objects_members = HashMap::new();
        list_objects_members.insert(
            "Bucket".to_string(),
            crate::extraction::sdk_model::ShapeReference {
                shape: "String".to_string(),
            },
        );

        s3_shapes.insert(
            "ListObjectsV2Request".to_string(),
            Shape {
                type_name: "structure".to_string(),
                members: list_objects_members,
                required: Some(vec![]),
            },
        );

        services.insert(
            "s3".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2006-03-01".to_string(),
                    service_id: "S3".to_string(),
                },
                operations: s3_operations,
                shapes: s3_shapes,
            },
        );

        // Add EC2 and DynamoDB services for other tests
        let mut ec2_operations = HashMap::new();
        ec2_operations.insert(
            "DescribeInstances".to_string(),
            Operation {
                name: "DescribeInstances".to_string(),
                input: None,
            },
        );

        services.insert(
            "ec2".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2016-11-15".to_string(),
                    service_id: "EC2".to_string(),
                },
                operations: ec2_operations,
                shapes: HashMap::new(),
            },
        );

        let mut dynamodb_operations = HashMap::new();
        dynamodb_operations.insert(
            "Scan".to_string(),
            Operation {
                name: "Scan".to_string(),
                input: None,
            },
        );

        services.insert(
            "dynamodb".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2012-08-10".to_string(),
                    service_id: "DynamoDB".to_string(),
                },
                operations: dynamodb_operations,
                shapes: HashMap::new(),
            },
        );

        // Add GameLift service with DescribeInstances operation
        let mut gamelift_operations = HashMap::new();
        gamelift_operations.insert(
            "DescribeInstances".to_string(),
            Operation {
                name: "DescribeInstances".to_string(),
                input: None,
            },
        );

        services.insert(
            "gamelift".to_string(),
            SdkServiceDefinition {
                version: Some("2.0".to_string()),
                metadata: ServiceMetadata {
                    api_version: "2015-10-01".to_string(),
                    service_id: "GameLift".to_string(),
                },
                operations: gamelift_operations,
                shapes: HashMap::new(),
            },
        );

        // Add services with TagResource operation
        for (service_name, api_version, service_id) in [
            ("accessanalyzer", "2019-11-01", "AccessAnalyzer"),
            ("aiops", "2020-12-01", "AIOps"),
            ("amp", "2020-08-01", "AMP"),
        ] {
            let mut operations = HashMap::new();
            operations.insert(
                "TagResource".to_string(),
                Operation {
                    name: "TagResource".to_string(),
                    input: None,
                },
            );

            services.insert(
                service_name.to_string(),
                SdkServiceDefinition {
                    version: Some("2.0".to_string()),
                    metadata: ServiceMetadata {
                        api_version: api_version.to_string(),
                        service_id: service_id.to_string(),
                    },
                    operations,
                    shapes: HashMap::new(),
                },
            );
        }

        // Add method lookup entries (Go uses PascalCase method names)
        method_lookup.insert(
            "ListObjectsV2".to_string(),
            vec![ServiceMethodRef {
                service_name: "s3".to_string(),
                operation_name: "ListObjectsV2".to_string(),
            }],
        );

        method_lookup.insert(
            "DescribeInstances".to_string(),
            vec![
                ServiceMethodRef {
                    service_name: "ec2".to_string(),
                    operation_name: "DescribeInstances".to_string(),
                },
                ServiceMethodRef {
                    service_name: "gamelift".to_string(),
                    operation_name: "DescribeInstances".to_string(),
                },
            ],
        );

        method_lookup.insert(
            "Scan".to_string(),
            vec![ServiceMethodRef {
                service_name: "dynamodb".to_string(),
                operation_name: "Scan".to_string(),
            }],
        );

        ServiceModelIndex {
            services,
            method_lookup,
            waiter_lookup: HashMap::new(),
        }
    }

    #[test]
    fn test_basic_paginator() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"
	"fmt"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := s3.NewFromConfig(cfg)

	// Basic paginator creation and usage
	paginator := s3.NewListObjectsV2Paginator(client, &s3.ListObjectsV2Input{
		Bucket: &[]string{"my-bucket"}[0],
	})

	for paginator.HasMorePages() {
		page, err := paginator.NextPage(context.TODO())
		if err != nil {
			fmt.Printf("Error: %v\n", err)
			break
		}
		fmt.Printf("Found %d objects\n", len(page.Contents))
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for ListObjectsV2
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "ListObjectsV2");
        assert_eq!(paginator_calls[0].possible_services, vec!["s3"]);
    }

    #[test]
    fn test_paginator_creation_without_nextpage() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := ec2.NewFromConfig(cfg)

	// Paginator creation without NextPage() call - still generates synthetic call
	paginator := ec2.NewDescribeInstancesPaginator(client, &ec2.DescribeInstancesInput{
		MaxResults: &[]int32{10}[0],
	})
	
	// Paginator created but not used - should still generate synthetic call with creation params
	_ = paginator
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for DescribeInstances using creation parameters
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "DescribeInstances");

        // Should have parameters from paginator creation
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        assert!(!metadata.parameters.is_empty());
    }

    #[test]
    fn test_paginator_with_multiple_nextpage_calls() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/dynamodb"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := dynamodb.NewFromConfig(cfg)

	// Single paginator with multiple NextPage calls
	paginator := dynamodb.NewScanPaginator(client, &dynamodb.ScanInput{
		TableName: &[]string{"my-table"}[0],
	})

	// First NextPage call
	page1, err := paginator.NextPage(context.TODO())
	if err != nil {
		return
	}
	_ = page1

	// Second NextPage call - only one synthetic call generated from creation
	page2, err := paginator.NextPage(context.TODO())
	if err != nil {
		return
	}
	_ = page2
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call from paginator creation
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "Scan");
        assert_eq!(paginator_calls[0].possible_services, vec!["dynamodb"]);
    }

    #[test]
    fn test_chained_paginator() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := s3.NewFromConfig(cfg)

	// Chained paginator call - creation and usage in one line
	page, err := s3.NewListObjectsV2Paginator(client, &s3.ListObjectsV2Input{
		Bucket: &[]string{"test-bucket"}[0],
		Prefix: &[]string{"logs/"}[0],
	}).NextPage(context.TODO())
	
	if err != nil {
		return
	}
	_ = page
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for the chained paginator
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "ListObjectsV2");
        assert_eq!(paginator_calls[0].possible_services, vec!["s3"]);

        // Should have parameters from the paginator creation
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        assert!(!metadata.parameters.is_empty());
    }

    #[test]
    fn test_hasmorepages_loop() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := ec2.NewFromConfig(cfg)

	// Typical paginator usage pattern with HasMorePages loop
	paginator := ec2.NewDescribeInstancesPaginator(client, &ec2.DescribeInstancesInput{
		MaxResults: &[]int32{10}[0],
	})

	for paginator.HasMorePages() {
		page, err := paginator.NextPage(context.TODO())
		if err != nil {
			break
		}
		
		for _, reservation := range page.Reservations {
			for _, instance := range reservation.Instances {
				_ = instance.InstanceId
			}
		}
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for DescribeInstances
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "DescribeInstances");

        // Should have parameters from the paginator creation
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        assert!(!metadata.parameters.is_empty());
    }

    #[test]
    fn test_if_assignment_paginator() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"
	"fmt"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := s3.NewFromConfig(cfg)

	// Paginator with if-statement assignment pattern
	paginator := s3.NewListObjectsV2Paginator(client, &s3.ListObjectsV2Input{
		Bucket: &[]string{"my-bucket"}[0],
		Prefix: &[]string{"logs/"}[0],
	})

	// If-statement assignment - common Go error handling pattern
	if page, err := paginator.NextPage(context.TODO()); err == nil {
		fmt.Printf("Found %d objects in first page\n", len(page.Contents))
		
		// Another if-statement assignment in the same scope
		if page2, err := paginator.NextPage(context.TODO()); err == nil {
			fmt.Printf("Found %d objects in second page\n", len(page2.Contents))
		} else {
			fmt.Printf("Error getting second page: %v\n", err)
		}
	} else {
		fmt.Printf("Error getting first page: %v\n", err)
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call from paginator creation
        assert_eq!(paginator_calls.len(), 1);

        for call in &paginator_calls {
            assert_eq!(call.name, "ListObjectsV2");
            assert_eq!(call.possible_services, vec!["s3"]);

            // Should have parameters from the paginator creation
            let metadata = call.metadata.as_ref().unwrap();
            assert!(!metadata.parameters.is_empty());
        }
    }

    #[test]
    fn test_switch_assignment_paginator() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"
	"fmt"
	"log"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := s3.NewFromConfig(cfg)

	// Paginator with switch-statement assignment pattern
	paginator := s3.NewListObjectsV2Paginator(client, &s3.ListObjectsV2Input{
		Bucket: &[]string{"my-bucket"}[0],
		Prefix: &[]string{"data/"}[0],
	})

	// Switch-statement assignment - Go error handling pattern
	switch page, err := paginator.NextPage(context.TODO()); {
	case err != nil:
		log.Printf("Error getting first page: %v", err)
	default:
		fmt.Printf("Found %d objects in first page\n", len(page.Contents))
		
		// Another switch-statement assignment
		switch page2, err := paginator.NextPage(context.TODO()); {
		case err != nil:
			log.Printf("Error getting second page: %v", err)
		default:
			fmt.Printf("Found %d objects in second page\n", len(page2.Contents))
		}
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call from paginator creation
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "ListObjectsV2");
        assert_eq!(paginator_calls[0].possible_services, vec!["s3"]);

        // Should have parameters from the paginator creation
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        assert!(!metadata.parameters.is_empty());
    }

    #[test]
    fn test_variable_input_paginator() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := s3.NewFromConfig(cfg)

	// Input struct stored in variable - not inline
	input := &s3.ListObjectsV2Input{
		Bucket: &[]string{"my-bucket"}[0],
		Prefix: &[]string{"logs/"}[0],
	}

	// Paginator creation with variable input
	paginator := s3.NewListObjectsV2Paginator(client, input)

	// Use the paginator
	for paginator.HasMorePages() {
		page, err := paginator.NextPage(context.TODO())
		if err != nil {
			break
		}
		_ = page
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for ListObjectsV2 even with variable input
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "ListObjectsV2");
        assert_eq!(paginator_calls[0].possible_services, vec!["s3"]);

        // Should have parameters - variable references are captured as unresolved parameters
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        // We extract variable names as parameters (this is correct behavior)
        assert!(!metadata.parameters.is_empty());

        // Should have the input variable parameter (client is skipped in creation_arguments)
        assert_eq!(metadata.parameters.len(), 1);
        if let Parameter::Positional { value, .. } = &metadata.parameters[0] {
            if let crate::extraction::ParameterValue::Unresolved(var_name) = value {
                assert_eq!(var_name, "input");
            }
        }
    }

    #[test]
    fn test_ambiguous_variable_input_multiple_services() {
        let service_index = create_test_service_index();
        let extractor = GoPaginatorExtractor::new(&service_index);

        let source_code = r#"
package main

import (
	"context"

	"github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/ec2"
)

func main() {
	cfg, err := config.LoadDefaultConfig(context.TODO())
	if err != nil {
		panic(err)
	}

	client := ec2.NewFromConfig(cfg)

	// Input struct stored in variable - ambiguous operation exists in multiple services
	input := &ec2.DescribeInstancesInput{
		MaxResults: &[]int32{10}[0],
	}

	// Paginator creation with variable input - should generate calls for all possible services
	paginator := ec2.NewDescribeInstancesPaginator(client, input)

	// Use the paginator
	for paginator.HasMorePages() {
		page, err := paginator.NextPage(context.TODO())
		if err != nil {
			break
		}
		_ = page
	}
}
"#;

        let ast = create_test_ast(source_code);
        let paginator_calls = extractor.extract_paginator_method_calls(&ast);

        // Should extract one call for DescribeInstances
        assert_eq!(paginator_calls.len(), 1);
        assert_eq!(paginator_calls[0].name, "DescribeInstances");

        // Should include ALL possible services (false positives preferred over filtering out)
        let mut services = paginator_calls[0].possible_services.clone();
        services.sort();
        assert_eq!(services, vec!["ec2", "gamelift"]);

        // Should have the input variable parameter
        let metadata = paginator_calls[0].metadata.as_ref().unwrap();
        assert_eq!(metadata.parameters.len(), 1);
        if let Parameter::Positional { value, .. } = &metadata.parameters[0] {
            if let crate::extraction::ParameterValue::Unresolved(var_name) = value {
                assert_eq!(var_name, "input");
            }
        }
    }
}
