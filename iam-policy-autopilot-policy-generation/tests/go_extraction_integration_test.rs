//! Integration test for Go extraction and policy generation
//!
//! This test demonstrates the complete flow from Go source code to IAM policy generation,
//! using the same Go program as in test_go_method_call_parsing but extending it to include
//! enrichment and policy generation through the public API.

use iam_policy_autopilot_policy_generation::{
    EnrichmentEngine, ExtractionEngine, Language, PolicyGenerationEngine, SourceFile,
};
use std::path::PathBuf;

/// The same Go program used in test_go_method_call_parsing
const GO_AWS_SDK_CODE: &str = r#"
package main

import (
    "context"
    "log"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/s3"
)

func main() {
    // Load the Shared AWS Configuration (~/.aws/config)
    cfg, err := config.LoadDefaultConfig(context.TODO())
    if err != nil {
        log.Fatal(err)
    }

    // Create an Amazon S3 service client
    client := s3.NewFromConfig(cfg)

    // Get the first page of results for ListObjectsV2 for a bucket
    output, err := client.ListObjectsV2(context.TODO(), &s3.ListObjectsV2Input{
        Bucket: aws.String("amzn-s3-demo-bucket"),
    })
    if err != nil {
        log.Fatal(err)
    }

    log.Println("first page results")
    for _, object := range output.Contents {
        log.Printf("key=%s size=%d", aws.ToString(object.Key), *object.Size)
    }
}
"#;

#[tokio::test]
async fn test_go_extraction_to_policy_generation_integration() {
    println!("Starting Go extraction to policy generation integration test...");

    // Step 1: Create source file from Go code
    println!("Step 1: Creating source file from Go code...");
    let source_file = SourceFile::with_language(
        PathBuf::from("test_s3.go"),
        GO_AWS_SDK_CODE.to_string(),
        Language::Go,
    );

    println!(
        "Created source file: {} ({} bytes)",
        source_file.language,
        source_file.content.len()
    );

    // Step 2: Extract SDK method calls using the extraction engine
    println!("\nStep 2: Extracting SDK method calls using extraction engine...");

    let extraction_engine = ExtractionEngine::new();

    match extraction_engine
        .extract_sdk_method_calls(Language::Go, vec![source_file])
        .await
    {
        Ok(extracted_methods) => {
            println!(
                "Extracted {} method calls:",
                extracted_methods.methods.len()
            );

            // Verify we extracted some method calls
            assert!(
                !extracted_methods.methods.is_empty(),
                "Should extract at least one method call"
            );

            // Step 3: Enrich method calls with IAM actions and resources
            println!("\nStep 3: Enriching method calls with IAM metadata...");

            let mut enrichment_engine = EnrichmentEngine::new(false).unwrap();

            match enrichment_engine
                .enrich_methods(&extracted_methods.methods)
                .await
            {
                Ok(enriched_calls) => {
                    println!("Enriched {} method calls:", enriched_calls.len());

                    // Check if enrichment worked
                    if enriched_calls.is_empty() {
                        panic!("No method calls were enriched (likely due to missing config files in test environment)");
                    }

                    // Step 4: Generate IAM policies
                    println!("\nStep 4: Generating IAM policies...");

                    let policy_engine =
                        PolicyGenerationEngine::new("aws", "us-east-1", "123456789012");

                    match policy_engine.generate_policies(&enriched_calls) {
                        Ok(policies) => {
                            println!("Generated {} IAM policies:", policies.len());

                            // Verify policy generation worked
                            assert!(!policies.is_empty(), "Should generate at least one policy");

                            // Step 5: Test policy merging
                            println!("\nStep 5: Testing policy merging...");

                            if policies.len() > 1 {
                                match policy_engine.merge_policies(&policies) {
                                    Ok(merged_policy) => {
                                        println!(
                                            "Successfully merged {} policies into one",
                                            policies.len()
                                        );

                                        // Test JSON serialization of merged policy
                                        match serde_json::to_string_pretty(&merged_policy) {
                                            Ok(json) => {
                                                println!(
                                                    "Merged policy JSON ({} bytes)",
                                                    json.len()
                                                );

                                                // Verify JSON structure
                                                assert!(
                                                    json.contains("\"Version\""),
                                                    "JSON should contain Version field"
                                                );
                                                assert!(
                                                    json.contains("\"Statement\""),
                                                    "JSON should contain Statement field"
                                                );
                                            }
                                            Err(e) => {
                                                panic!(
                                                    "Failed to serialize merged policy to JSON: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        panic!("Policy merging failed: {}", e);
                                    }
                                }
                            } else {
                                println!("Only one policy generated, skipping merge test");
                            }

                            // Step 6: Test JSON serialization of individual policies
                            println!("\nStep 6: Testing JSON serialization...");

                            for (i, policy) in policies.iter().enumerate() {
                                match serde_json::to_string_pretty(policy) {
                                    Ok(json) => {
                                        println!("Policy {} JSON ({} bytes)", i + 1, json.len());

                                        // Verify JSON structure
                                        assert!(
                                            json.contains("\"Version\""),
                                            "JSON should contain Version field"
                                        );
                                        assert!(
                                            json.contains("\"Statement\""),
                                            "JSON should contain Statement field"
                                        );
                                        assert!(
                                            json.contains("\"Effect\""),
                                            "JSON should contain Effect field"
                                        );
                                        assert!(
                                            json.contains("\"Action\""),
                                            "JSON should contain Action field"
                                        );
                                        assert!(
                                            json.contains("\"Resource\""),
                                            "JSON should contain Resource field"
                                        );
                                    }
                                    Err(e) => {
                                        panic!("Failed to serialize policy to JSON: {}", e);
                                    }
                                }
                            }

                            println!("\n✅ Integration test completed successfully!");
                            println!("Summary:");
                            println!(
                                "  - Extracted {} method calls from Go code",
                                extracted_methods.methods.len()
                            );
                            println!(
                                "  - Enriched {} method calls with IAM metadata",
                                enriched_calls.len()
                            );
                            println!("  - Generated {} IAM policies", policies.len());
                        }
                        Err(e) => {
                            panic!("Policy generation failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    panic!("Enrichment failed: {}", e);
                }
            }
        }
        Err(e) => {
            panic!("Extraction failed: {}", e);
        }
    }
}

#[tokio::test]
async fn test_go_source_file_creation() {
    println!("Testing Go source file creation...");

    let source_file = SourceFile::with_language(
        PathBuf::from("test.go"),
        GO_AWS_SDK_CODE.to_string(),
        Language::Go,
    );

    // Verify source file structure
    assert_eq!(source_file.language, Language::Go);
    assert!(!source_file.content.is_empty());
    assert!(source_file.content.contains("ListObjectsV2"));
    assert!(source_file.content.contains("package main"));
}
