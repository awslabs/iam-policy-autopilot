//! Integration tests for Go AWS SDK v2 feature packages
//!
//! This test suite validates that the IAM Policy Autopilot correctly identifies
//! and generates permissions for virtual operations in AWS Go SDK v2 feature packages.
//! These tests are designed to FAIL initially, demonstrating the need for enhanced
//! support for these SDK features.
//!
//! Based on go-analysis.json which documents operations requiring IAM permissions.

use iam_policy_autopilot_policy_generation::{
    EnrichmentEngine, ExtractionEngine, Language, PolicyGenerationEngine, SourceFile,
};
use std::path::PathBuf;

/// Test S3 Manager Uploader.Upload operation
///
/// This operation requires multiple permissions depending on file size:
/// - s3:PutObject (always)
/// - s3:CreateMultipartUpload (for large files)
/// - s3:UploadPart (for multipart uploads)
/// - s3:CompleteMultipartUpload (to finalize)
/// - s3:AbortMultipartUpload (on failure)
#[tokio::test]
async fn test_s3_manager_uploader_upload() {
    let go_code = r#"
package main

import (
    "context"
    "os"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/feature/s3/manager"
)

func main() {
    cfg, _ := config.LoadDefaultConfig(context.TODO())
    client := s3.NewFromConfig(cfg)
    
    // Create uploader with 10 MB part size
    uploader := manager.NewUploader(client, func(u *manager.Uploader) {
        u.PartSize = 10 * 1024 * 1024
    })
    
    file, _ := os.Open("large-file.zip")
    defer file.Close()
    
    // Upload file - requires multiple S3 permissions
    result, err := uploader.Upload(context.TODO(), &s3.PutObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("large-file.zip"),
        Body:   file,
    })
}
"#;

    let source_file = SourceFile::with_language(
        PathBuf::from("test_s3_uploader.go"),
        go_code.to_string(),
        Language::Go,
    );

    let extraction_engine = ExtractionEngine::new();
    let extracted = extraction_engine
        .extract_sdk_method_calls(Language::Go, vec![source_file])
        .await
        .expect("Extraction should succeed");

    assert!(
        !extracted.methods.is_empty(),
        "Should extract Uploader.Upload method call"
    );

    let mut enrichment_engine = EnrichmentEngine::new(false).unwrap();
    let enriched = enrichment_engine
        .enrich_methods(&extracted.methods)
        .await
        .expect("Enrichment should succeed");

    let policy_engine = PolicyGenerationEngine::new("aws", "us-east-1", "123456789012");
    let policies = policy_engine
        .generate_policies(&enriched)
        .expect("Policy generation should succeed");

    // Verify the policy contains all required S3 permissions for multipart upload
    let policy_json = serde_json::to_string(&policies).unwrap();

    // The S3 Upload feature uses multiple operations, but according to AWS service reference,
    // they all map to s3:PutObject and related permissions, not separate CreateMultipartUpload, etc.
    assert!(
        policy_json.contains("s3:PutObject"),
        "Policy should contain s3:PutObject permission"
    );
    assert!(
        policy_json.contains("s3:AbortMultipartUpload"),
        "Policy should contain s3:AbortMultipartUpload permission for error handling"
    );

    // Verify we extracted all 5 operations (even though they map to similar IAM actions)
    assert_eq!(
        extracted.methods.len(),
        5,
        "Should extract 5 method calls for Upload feature"
    );
}

/// Test S3 Manager Downloader.Download operation
///
/// This operation requires:
/// - s3:GetObject (for each part or single download)
#[tokio::test]
async fn test_s3_manager_downloader_download() {
    let go_code = r#"
package main

import (
    "context"
    "os"
    "github.com/aws/aws-sdk-go-v2/aws"
    "github.com/aws/aws-sdk-go-v2/config"
    "github.com/aws/aws-sdk-go-v2/service/s3"
    "github.com/aws/aws-sdk-go-v2/feature/s3/manager"
)

func main() {
    cfg, _ := config.LoadDefaultConfig(context.TODO())
    client := s3.NewFromConfig(cfg)
    
    downloader := manager.NewDownloader(client)
    
    file, _ := os.Create("downloaded-file.zip")
    defer file.Close()
    
    // Download file with concurrent part downloads
    numBytes, err := downloader.Download(context.TODO(), file, &s3.GetObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("large-file.zip"),
    })
}
"#;

    let source_file = SourceFile::with_language(
        PathBuf::from("test_s3_downloader.go"),
        go_code.to_string(),
        Language::Go,
    );

    let extraction_engine = ExtractionEngine::new();
    let extracted = extraction_engine
        .extract_sdk_method_calls(Language::Go, vec![source_file])
        .await
        .expect("Extraction should succeed");

    let mut enrichment_engine = EnrichmentEngine::new(false).unwrap();
    let enriched = enrichment_engine
        .enrich_methods(&extracted.methods)
        .await
        .expect("Enrichment should succeed");

    let policy_engine = PolicyGenerationEngine::new("aws", "us-east-1", "123456789012");
    let policies = policy_engine
        .generate_policies(&enriched)
        .expect("Policy generation should succeed");

    let policy_json = serde_json::to_string(&policies).unwrap();

    assert!(
        policy_json.contains("s3:GetObject"),
        "Policy should contain s3:GetObject permission for downloads"
    );
}
