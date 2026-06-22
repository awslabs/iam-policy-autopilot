//! Regression tests for chained and nested boto3 sub-resource action calls.
//!
//! boto3 resource actions can be invoked on an inline sub-resource constructor, on a
//! variable bound to one, and on chains that navigate one or more sub-resources before
//! the action. All of the following must resolve to `s3:PutObject`:
//!
//! ```python
//! s3 = boto3.resource("s3")
//! # assigned single-level
//! bucket = s3.Bucket("b"); bucket.put_object(Key="k", Body=data)
//! # chained single-level
//! s3.Bucket("b").put_object(Key="k", Body=data)
//! # nested chained
//! s3.Bucket("b").Object("k").put(Body=data)
//! # nested, assigned mid-chain
//! obj = s3.Bucket("b").Object("k"); obj.put(Body=data)
//! ```
//!
//! The resource model injects the required identifiers (`Bucket`, `Key`) from the
//! resource's identifiers, so the call is valid even though only `Key`/`Body` appear at
//! the action call site. Navigating a (sub-)resource without invoking an action is NOT
//! an API call and must not produce any permissions.

use iam_policy_autopilot_policy_generation::{ExtractionEngine, Language, SourceFile};
use rstest::rstest;
use std::path::PathBuf;

/// Extract method calls as sorted `(service, method)` pairs for exact comparison.
/// A call with multiple possible services yields one pair per service.
async fn extract(src: &str) -> Vec<(String, String)> {
    let engine = ExtractionEngine::new();
    let sf = SourceFile::with_language(PathBuf::from("app.py"), src.to_string(), Language::Python);
    let extracted = engine
        .extract_sdk_method_calls(Language::Python, vec![sf])
        .await
        .expect("extraction should succeed");
    let mut ops: Vec<(String, String)> = extracted
        .methods
        .iter()
        .flat_map(|m| {
            m.possible_services
                .iter()
                .map(move |service| (service.clone(), m.name.clone()))
        })
        .collect();
    ops.sort();
    ops
}

/// Each case asserts the *exact* set of extracted `(service, method)` operations — no
/// missing and no spurious entries. The fix is keyed off resource type, so it
/// generalizes across services.
#[rstest]
#[case::assigned_s3_put_object(
    r#"
import boto3

def handle():
    body = "data"
    s3 = boto3.resource("s3")
    bucket = s3.Bucket("my-bucket")
    bucket.put_object(Key="k", Body=body)
"#,
    &[("s3", "put_object")]
)]
#[case::chained_s3_put_object(
    r#"
import boto3

def handle():
    body = "data"
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").put_object(Key="k", Body=body)
"#,
    &[("s3", "put_object")]
)]
#[case::chained_dynamodb_get_item(
    r#"
import boto3

def handle():
    dynamodb = boto3.resource("dynamodb")
    dynamodb.Table("my-table").get_item(Key={"id": "1"})
"#,
    &[("dynamodb", "get_item")]
)]
#[case::nested_bucket_object_put(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").Object("my-key").put(Body="data")
"#,
    &[("s3", "put_object")]
)]
#[case::assigned_nested_chain_then_action(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    obj = s3.Bucket("my-bucket").Object("my-key")
    obj.put(Body="data")
"#,
    &[("s3", "put_object")]
)]
#[case::assigned_nested_chain_two_actions(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    obj = s3.Bucket("my-bucket").Object("my-key")
    obj.put(Body="data")
    obj.delete()
"#,
    &[("s3", "delete_object"), ("s3", "put_object")]
)]
// Assigning a (sub-)resource without invoking any action must not, on its own,
// produce permissions — navigation is not an API call.
#[case::assignment_only_no_action(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    obj = s3.Bucket("my-bucket").Object("my-key")
"#,
    &[]
)]
// Depth-3 chain: verifies identifier accumulation recurses beyond two levels.
// Object("b","k") -> MultipartUpload("id") -> Part(1) -> upload(...) carries
// BucketName/ObjectKey from Object, UploadId from MultipartUpload, and PartNumber from
// the Part navigation into the UploadPart operation.
#[case::deep_chain_multipart_upload_part(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    s3.Object("my-bucket", "my-key").MultipartUpload("upload-id").Part(1).upload(Body="data")
"#,
    &[("s3", "upload_part")]
)]
// Accessor name differs from the target resource type (`Acl` -> type `BucketAcl`),
// verifying that sub-resources are keyed by accessor, not by type.
#[case::accessor_differs_from_type_bucket_acl(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").Acl().put(ACL="private")
"#,
    &[("s3", "put_bucket_acl")]
)]
// A chain through an unknown sub-resource accessor must resolve to nothing — a chain is
// resolved fully or not at all, never partially or speculatively (no over-approximation).
#[case::unknown_sub_resource_accessor(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").Sprocket("x").put(Body="data")
"#,
    &[]
)]
// A known nested resource with an unknown leaf action must also resolve to nothing.
#[case::unknown_leaf_action(
    r#"
import boto3

def handle():
    s3 = boto3.resource("s3")
    s3.Bucket("my-bucket").Object("my-key").frobnicate(X="data")
"#,
    &[]
)]
#[tokio::test]
async fn subresource_action_resolves(#[case] src: &str, #[case] expected_ops: &[(&str, &str)]) {
    let actual = extract(src).await;
    let expected: Vec<(String, String)> = expected_ops
        .iter()
        .map(|(service, method)| (service.to_string(), method.to_string()))
        .collect();
    assert_eq!(
        actual, expected,
        "extracted operations should be exactly {expected:?}, got: {actual:?}"
    );
}
