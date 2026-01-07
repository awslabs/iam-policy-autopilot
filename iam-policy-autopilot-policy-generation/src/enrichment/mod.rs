//! Enrichment module for loading and managing enrichment data
//!
//! This module provides functionality to load operation action maps
//! and Service Definition Files (SDFs) from the filesystem with caching
//! capabilities for performance optimization.
//!
//! This module also contains the enriched method call data structures
//! that represent method calls enriched with IAM metadata from operation
//! action maps and Service Definition Files.

use std::collections::HashSet;

use crate::{extraction::SdkMethodCallMetadata, SdkMethodCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) mod engine;
pub(crate) mod operation_fas_map;
pub(crate) mod resource_matcher;
pub(crate) mod service_reference;

pub use engine::Engine;
pub(crate) use operation_fas_map::load_operation_fas_map;
pub(crate) use resource_matcher::ResourceMatcher;
pub(crate) use service_reference::RemoteServiceReferenceLoader as ServiceReferenceLoader;

const FAS_URL: &str =
    "https://docs.aws.amazon.com/IAM/latest/UserGuide/access_forward_access_sessions.html";

/// Represents Forward Access Session (FAS) expansion information
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct FasInfo {
    /// Explanation URL for Forward Access Sessions
    pub explanation: &'static str,
    /// The chain of operations in the FAS expansion
    pub expansion: Vec<String>,
}

impl FasInfo {
    /// Create a new FasInfo with the standard AWS documentation URL
    #[must_use]
    pub fn new(expansion: Vec<String>) -> Self {
        Self {
            explanation: FAS_URL,
            expansion,
        }
    }
}

/// Represents the reason why an action was added to a policy
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct Reason {
    /// The original operation that was extracted
    pub initial_operation: Operation,
    /// Source of the operation
    pub source: OperationSource,
    /// Optional FAS expansion information
    pub fas: Option<FasInfo>,
}

impl Reason {
    pub(crate) fn new(
        call: &SdkMethodCall,
        original_service_name: &str,
        fas: Option<FasInfo>,
    ) -> Self {
        let initial_operation =
            Operation::new(call.name.clone(), original_service_name.to_string());
        match &call.metadata {
            None => Self {
                initial_operation,
                source: OperationSource::Provided,
                fas,
            },
            Some(metadata) => Self {
                initial_operation,
                source: OperationSource::Extracted(metadata.clone()),
                fas,
            },
        }
    }
}

#[derive(derive_new::new, Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct Operation {
    /// Name of the operation
    pub name: String,
    /// Name of the service
    pub service: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[serde(untagged)]
pub enum OperationSource {
    /// Operation extracted from source files
    #[serde(rename_all = "PascalCase")]
    Extracted(SdkMethodCallMetadata),
    /// Operation provided (no metadata available)
    #[serde(rename_all = "PascalCase")]
    Provided,
}

// impl OperationSource {
//     pub(crate) fn from_call(call: &SdkMethodCall, service: &str) -> Self {
//         match &call.metadata {
//             None => Self::Provided {
//                 name: call.name.clone(),
//                 service: service.to_string(),
//             },
//             Some(metadata) => Self::Extracted {
//                 name: call.name.clone(),
//                 service: service.to_string(),
//                 expr: metadata.expr.clone(),
//                 location: Location::new(
//                     metadata.file_path.clone(),
//                     metadata.start_position,
//                     metadata.end_position,
//                 ),
//             },
//         }
//     }
// }

/// Represents an explanation for why an action was added to a policy
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Explanation {
    /// The reasons this action was added (can have multiple reasons for the same action)
    pub reasons: Vec<Reason>,
}

impl Explanation {
    pub(crate) fn merge(&mut self, other: Explanation) {
        let reasons_set = self.reasons.iter().cloned().collect::<HashSet<_>>();
        for new_reason in other.reasons {
            if reasons_set.contains(&new_reason) {
                continue;
            }
            self.reasons.push(new_reason);
        }
    }
}

/// Represents an enriched method call with actions that need permissions
#[derive(Debug, Clone, Serialize, PartialEq)]
#[non_exhaustive]
pub struct EnrichedSdkMethodCall<'a> {
    /// The original method name from the parsed call
    pub(crate) method_name: String,
    /// The service this enriched call applies to
    pub(crate) service: String,
    /// Actions which need permissions for executing the method call
    pub(crate) actions: Vec<Action>,
    /// The initial SDK method call
    pub(crate) sdk_method_call: &'a SdkMethodCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, JsonSchema)]
pub enum Operator {
    StringEquals,
    StringLike,
}

impl Operator {
    pub(crate) fn to_like_version(&self) -> Self {
        match self {
            Self::StringEquals | Self::StringLike => Self::StringLike,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, JsonSchema)]
pub(crate) struct Condition {
    pub operator: Operator,
    pub key: String,
    pub values: Vec<String>,
}

/// Trait for context types that can be converted to conditions
pub(crate) trait Context {
    fn key(&self) -> &str;
    fn values(&self) -> &[String];
}

/// Represents an IAM action enriched with resource and condition information
///
/// This structure combines OperationAction action data with Service Reference resource information to provide
/// complete IAM policy metadata for a single action.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Action {
    /// The IAM action name (e.g., "s3:GetObject")
    pub(crate) name: String,
    /// List of resources this action applies to, enriched with ARN patterns
    pub(crate) resources: Vec<Resource>,
    /// List of conditions we are adding
    pub(crate) conditions: Vec<Condition>,
    /// Optional explanation why this action has been added
    pub(crate) explanation: Explanation,
}

impl Action {
    /// Create a new enriched action
    ///
    /// # Arguments
    /// * `name` - The IAM action name
    /// * `resources` - List of enriched resources
    /// * `conditions` - List of conditions
    /// * `explanation` - Explanation why the action has been added
    #[must_use]
    pub(crate) fn new(
        name: String,
        resources: Vec<Resource>,
        conditions: Vec<Condition>,
        explanation: Explanation,
    ) -> Self {
        Self {
            name,
            resources,
            conditions,
            explanation,
        }
    }
}

/// Represents a resource enriched with ARN pattern and metadata
///
/// This structure combines OperationAction resource data with Service Reference ARN patterns and additional
/// metadata to provide complete resource information for IAM policies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Resource {
    /// The resource type name (e.g., "bucket", "object", "*")
    pub(crate) name: String,
    /// ARN patterns from Service Reference data, if available
    pub(crate) arn_patterns: Option<Vec<String>>,
}

impl Resource {
    /// Create a new enriched resource
    #[must_use]
    pub(crate) fn new(name: String, arn_patterns: Option<Vec<String>>) -> Self {
        Self { name, arn_patterns }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enriched_resource_creation() {
        let resource = Resource::new(
            "object".to_string(),
            Some(vec!["arn:aws:s3:::bucket/*".to_string()]),
        );

        assert_eq!(resource.name, "object");
        assert_eq!(
            resource.arn_patterns,
            Some(vec!["arn:aws:s3:::bucket/*".to_string()])
        );
    }
}

#[cfg(test)]
pub(crate) mod mock_remote_service_reference {
    use crate::enrichment::service_reference::RemoteServiceReferenceLoader;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) async fn mock_server_service_reference_response(
        mock_server: &MockServer,
        service_name: &str,
        service_reference_raw: serde_json::Value,
    ) {
        let mock_server_url = mock_server.uri();

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", mock_server_url)},
                {"service": service_name, "url": format!("{}/{}.json", mock_server_url, service_name)}
            ])))
            .with_priority(1)
            .mount(mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/{}.json", service_name)))
            .respond_with(ResponseTemplate::new(200).set_body_json(service_reference_raw))
            .mount(mock_server)
            .await
    }

    pub(crate) async fn setup_mock_server_with_loader_without_operation_to_action_mapping(
    ) -> (MockServer, RemoteServiceReferenceLoader) {
        let mock_server = MockServer::start().await;
        let mock_server_url = mock_server.uri();

        // Mock the mapping endpoint
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", mock_server_url)}
            ])))
            .mount(&mock_server)
            .await;

        // Mock the service reference endpoint
        Mock::given(method("GET"))
            .and(path("/s3.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": "s3",
                "Actions": [
                    {
                        "Name": "AbortMultipartUpload",
                        "Resources": [
                            {
                            "Name": "accesspointobject"
                            },
                            {
                            "Name": "object"
                            }
                        ],
                    },
                    {
                        "Name": "GetObject",
                        "Resources": [
                            {
                                "Name": "bucket"
                            },
                            {
                                "Name": "object"
                            }
                        ]
                    }
                ],
                "Resources": [
                    {
                    "Name": "bucket",
                    "ARNFormats": [
                        "arn:${Partition}:s3:::${BucketName}"
                    ]
                    },
                    {
                    "Name": "object",
                    "ARNFormats": [
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}"
                    ]
                    }
                ]
            })))
            .mount(&mock_server)
            .await;

        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(mock_server_url);

        (mock_server, loader)
    }

    pub(crate) async fn setup_mock_server_with_loader() -> (MockServer, RemoteServiceReferenceLoader)
    {
        // Add small delay to avoid port conflicts in parallel tests
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        let mock_server = MockServer::start().await;
        let mock_server_url = mock_server.uri();

        // Mock the mapping endpoint
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", mock_server_url)}
            ])))
            .mount(&mock_server)
            .await;

        // Mock the service reference endpoint
        Mock::given(method("GET"))
            .and(path("/s3.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": "s3",
                "Actions": [
                    {
                        "Name": "AbortMultipartUpload",
                        "ActionConditionKeys": [
                            "s3:AccessGrantsInstanceArn",
                            "s3:ResourceAccount",
                            "s3:TlsVersion",
                            "s3:authType",
                            "s3:signatureAge",
                            "s3:signatureversion",
                            "s3:x-amz-content-sha256"
                        ],
                        "Annotations": {
                            "Properties": {
                            "IsList": false,
                            "IsPermissionManagement": false,
                            "IsTaggingOnly": false,
                            "IsWrite": true
                            }
                        },
                        "Resources": [
                            {
                            "Name": "accesspointobject"
                            },
                            {
                            "Name": "object"
                            }
                        ],
                        "SupportedBy": {
                            "IAM Access Analyzer Policy Generation": false,
                            "IAM Action Last Accessed": false
                        }
                    },
                    {
                        "Name": "GetObject",
                        "Resources": [
                            {
                                "Name": "bucket"
                            },
                            {
                                "Name": "object"
                            }
                        ]
                    }
                ],
                "Operations": [
                    {
                        "Name" : "GetObject",
                        "AuthorizedActions" :
                        [
                            {
                                "Name" : "GetObject",
                                "Service" : "s3"
                            },
                            {
                                "Name" : "GetObject",
                                "Service" : "s3-object-lambda"
                            },
                            {
                                "Name" : "GetObjectLegalHold",
                                "Service" : "s3"
                            },
                            {
                                "Name" : "GetObjectRetention",
                                "Service" : "s3"
                            },
                            {
                                "Name" : "GetObjectTagging",
                                "Service" : "s3"
                            },
                            {
                                "Name" : "GetObjectVersion",
                                "Service" : "s3"
                            }
                        ],
                        "SDK" :
                        [
                            {
                                "Name" : "s3",
                                "Method" : "get_object",
                                "Package" : "Boto3"
                            }
                        ]
                    }
                ],
                "Resources": [
                    {
                    "Name": "bucket",
                    "ARNFormats": [
                        "arn:${Partition}:s3:::${BucketName}"
                    ]
                    },
                    {
                    "Name": "object",
                    "ARNFormats": [
                        "arn:${Partition}:s3:::${BucketName}/${ObjectName}"
                    ]
                    }
                ]
            })))
            .mount(&mock_server)
            .await;

        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(mock_server_url);

        (mock_server, loader)
    }
}

#[cfg(test)]
mod location_tests {
    use super::*;
    use crate::Location;
    use std::path::PathBuf;

    #[test]
    fn test_location_to_gnu_string() {
        let location = Location::new(PathBuf::from("src/main.rs"), (10, 5), (10, 79));

        assert_eq!(location.to_gnu_format(), "src/main.rs:10.5-10.79");
    }

    #[test]
    fn test_location_to_gnu_string_multiline() {
        let location = Location::new(PathBuf::from("src/lib.rs"), (10, 5), (15, 20));

        assert_eq!(location.to_gnu_format(), "src/lib.rs:10.5-15.20");
    }

    #[test]
    fn test_location_serialization() {
        let location = Location::new(PathBuf::from("test.py"), (42, 15), (42, 80));

        let json = serde_json::to_string(&location).unwrap();
        assert_eq!(json, "\"test.py:42.15-42.80\"");
    }

    #[test]
    fn test_location_serialization_multiline() {
        let location = Location::new(PathBuf::from("example.go"), (100, 1), (105, 50));

        let json = serde_json::to_string(&location).unwrap();
        assert_eq!(json, "\"example.go:100.1-105.50\"");
    }

    fn mock_sdk_method_call() -> SdkMethodCall {
        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: Some(SdkMethodCallMetadata {
                parameters: vec![],
                return_type: None,
                expr: "s3.get_object(Bucket='my-bucket')".to_string(),
                location: Location::new(PathBuf::from("test.py"), (10, 5), (10, 79)),
                receiver: Some("s3".to_string()),
            }),
        }
    }

    #[test]
    fn test_reason_extracted_with_location() {
        let call = mock_sdk_method_call();

        let reason = Reason::new(&call, "s3", None);

        match reason.source {
            OperationSource::Extracted(metadata) => {
                assert_eq!(reason.initial_operation.name, "get_object");
                assert_eq!(reason.initial_operation.service, "s3");
                assert_eq!(metadata.expr, "s3.get_object(Bucket='my-bucket')");
                assert_eq!(metadata.location.to_gnu_format(), "test.py:10.5-10.79");
            }
            _ => panic!("Expected Extracted variant"),
        }
    }

    #[test]
    fn test_reason_extracted_serialization() {
        let call = mock_sdk_method_call();

        let reason = Reason::new(&call, "s3", None);
        let json = serde_json::to_string(&reason).unwrap();

        // Verify the location is serialized as a string in GNU format
        assert!(json.contains("\"Location\":\"test.py:10.5-10.79\""));
    }
}
