use iam_policy_autopilot_policy_generation::{ExtractionEngine, Language, SourceFile};
use std::path::PathBuf;

#[tokio::test]
async fn test_non_literal_service_call_kept() {
    let src = "import boto3\nclient = boto3.client(svc)\nclient.get_object(Bucket='b', Key='k')\n";
    let sf = SourceFile::with_language(PathBuf::from("app.py"), src.to_string(), Language::Python);
    let extracted = ExtractionEngine::new()
        .extract_sdk_method_calls(Language::Python, vec![sf])
        .await
        .expect("extraction should succeed");
    assert!(extracted.methods.iter().any(|m| m.name == "get_object"));
}
