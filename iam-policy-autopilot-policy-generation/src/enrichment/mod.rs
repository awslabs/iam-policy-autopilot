//! Enrichment module for loading and managing enrichment data
//!
//! This module provides functionality to load operation action maps
//! and Service Definition Files (SDFs) from the filesystem with caching
//! capabilities for performance optimization.
//!
//! This module also contains the enriched method call data structures
//! that represent method calls enriched with IAM metadata from operation
//! action maps and Service Definition Files.

use crate::SdkMethodCall;
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

/// Represents an IAM action enriched with resource and condition information
///
/// This structure combines OperationAction action data with Service Reference resource information to provide
/// complete IAM policy metadata for a single action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Action {
    /// The IAM action name (e.g., "s3:GetObject")
    pub(crate) name: String,
    /// List of resources this action applies to, enriched with ARN patterns
    pub(crate) resources: Vec<Resource>,
    /// List of conditions we are adding
    pub(crate) conditions: Vec<Condition>,
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

impl Action {
    /// Create a new enriched action
    ///
    /// # Arguments
    /// * `name` - The IAM action name
    /// * `resources` - List of enriched resources
    /// * `conditions` - List of conditions
    #[must_use]
    pub(crate) fn new(name: String, resources: Vec<Resource>, conditions: Vec<Condition>) -> Self {
        Self {
            name,
            resources,
            conditions,
        }
    }
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
