//! Integration tests for Java AWS SDK method call extraction
//!
//! These tests cover extraction from both AWS SDK for Java v1 and v2.

use std::path::PathBuf;

use iam_policy_autopilot_policy_generation::{ExtractionEngine, Language, SourceFile};

#[tokio::test]
async fn test_java_sdk_v2_basic_extraction() {
    let java_source = r#"
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.ListBucketsRequest;
import software.amazon.awssdk.services.s3.model.ListBucketsResponse;

public class S3Example {
    public static void main(String[] args) {
        S3Client s3 = S3Client.builder().build();
        ListBucketsRequest request = ListBucketsRequest.builder().build();
        ListBucketsResponse response = s3.listBuckets(request);
        response.buckets().forEach(bucket -> System.out.println(bucket.name()));
        // test list objects v2
        var response = s3.listObjectsV2();
        response.contents().forEach(object -> System.out.println(object.key()));
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("TestV2Basic.java"),
        java_source.to_string(),
        Language::Java,
    );
    let engine = ExtractionEngine::new();
    let result = engine
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await;
    match result {
        Ok(extracted_methods) => {
            println!("✅ Java SDK v2 basic extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", call.name, call.possible_services);
            }

            let list_buckets_op = extracted_methods
                .methods
                .iter()
                .find(|call| call.name == "listBuckets")
                .expect("Should find listBuckets operation");
            assert_eq!(
                list_buckets_op.possible_services,
                vec!["s3"],
                "Should associate with S3 service"
            );
            let list_objects_v2_op = extracted_methods
                .methods
                .iter()
                .find(|call| call.name == "listObjectsV2")
                .expect("Should find listObjectsV2 operation");
            assert_eq!(
                list_objects_v2_op.possible_services,
                vec!["s3"],
                "Should associate with S3 service"
            );
        }
        Err(e) => {
            panic!("Extraction failed: {:?}", e);
        }
    }
}

#[tokio::test]
async fn test_java_sdk_v2_paginator_extraction() {
    let java_source = r#"
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.paginators.ListBucketsIterable;

public class S3Example {
    public static void main(String[] args) {
        S3Client s3 = S3Client.builder().build();

        ListBucketsIterable response = s3.listBucketsPaginator();
        response.buckets().forEach(bucket -> System.out.println(bucket.name()));
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("TestV2Paginator.java"),
        java_source.to_string(),
        Language::Java,
    );
    let engine = ExtractionEngine::new();
    let result = engine
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await;
    match result {
        Ok(extracted_methods) => {
            println!("✅ Java SDK v2 paginator extraction succeeded");
            println!("  Found {} method calls", extracted_methods.methods.len());

            let list_buckets_op = extracted_methods
                .methods
                .iter()
                .find(|call| call.name == "listBuckets")
                .expect("Should find listBuckets operation");
            assert_eq!(
                list_buckets_op.possible_services,
                vec!["s3"],
                "Should associate with S3 service"
            );
            for call in &extracted_methods.methods {
                println!("  - {} (service: {:?})", call.name, call.possible_services);
            }
        }
        Err(e) => {
            panic!("Extraction failed: {:?}", e);
        }
    }
}

#[tokio::test]
async fn test_java_sdk_v2_rds_acronym_operations() {
    let java_source = r#"
import software.amazon.awssdk.services.rds.RdsClient;

public class RdsExample {
    public static void main(String[] args) {
        RdsClient rds = RdsClient.builder().build();
        rds.createDBCluster();
        rds.createDBInstance();
        rds.createDBSnapshot();
        rds.deleteDBSnapshot();
        rds.describeDBClusters();
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("RdsExample.java"),
        java_source.to_string(),
        Language::Java,
    );
    let extracted_methods = ExtractionEngine::new()
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await
        .expect("Java SDK v2 RDS extraction should succeed");

    for expected_name in [
        "createDBCluster",
        "createDBInstance",
        "createDBSnapshot",
        "deleteDBSnapshot",
        "describeDBClusters",
    ] {
        let call = extracted_methods
            .methods
            .iter()
            .find(|call| call.name == expected_name)
            .unwrap_or_else(|| panic!("Should find {expected_name} operation"));
        assert_eq!(call.possible_services, vec!["rds"]);
    }
}

#[tokio::test]
async fn test_java_sdk_v2_acronym_paginator() {
    let java_source = r#"
import software.amazon.awssdk.services.rds.RdsClient;

public class RdsPaginatorExample {
    public static void main(String[] args) {
        RdsClient rds = RdsClient.builder().build();
        rds.describeDBClustersPaginator();
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("RdsPaginatorExample.java"),
        java_source.to_string(),
        Language::Java,
    );
    let extracted_methods = ExtractionEngine::new()
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await
        .expect("Java SDK v2 RDS paginator extraction should succeed");

    let call = extracted_methods
        .methods
        .iter()
        .find(|call| call.name == "describeDBClusters")
        .expect("Should resolve the acronym paginator to DescribeDBClusters");
    assert_eq!(call.possible_services, vec!["rds"]);
}

#[tokio::test]
async fn test_java_sdk_v2_acronym_waiter() {
    let java_source = r#"
import software.amazon.awssdk.services.acmpca.waiters.AcmPcaWaiter;

public class AcmPcaExample {
    public static void waitForCertificateAuthority(AcmPcaWaiter waiter) {
        waiter.waitUntilCertificateAuthorityCSRCreated();
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("AcmPcaExample.java"),
        java_source.to_string(),
        Language::Java,
    );
    let extracted_methods = ExtractionEngine::new()
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await
        .expect("Java SDK v2 ACM PCA waiter extraction should succeed");

    let call = extracted_methods
        .methods
        .iter()
        .find(|call| call.name == "getCertificateAuthorityCsr")
        .expect("Should resolve the acronym waiter to GetCertificateAuthorityCsr");
    assert_eq!(call.possible_services, vec!["acm-pca"]);
}

#[tokio::test]
async fn test_java_no_sdk_calls() {
    let java_source = r#"
public class NoSdkExample {
    public static void main(String[] args) {
        System.out.println("Hello, World!");
    }
}
    "#;
    let source_file = SourceFile::with_language(
        PathBuf::from("NoSdkExample.java"),
        java_source.to_string(),
        Language::Java,
    );
    let engine = ExtractionEngine::new();
    let result = engine
        .extract_sdk_method_calls(Language::Java, vec![source_file])
        .await;
    match result {
        Ok(extracted_methods) => {
            println!("✅ Java extraction (no SDK calls) succeeded");
            assert!(
                extracted_methods.methods.is_empty(),
                "Should find no method calls"
            );
        }
        Err(e) => {
            panic!("Extraction failed: {:?}", e);
        }
    }
}
