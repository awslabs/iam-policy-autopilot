//! Enrichment module for loading and managing enrichment data
//!
//! This module provides functionality to load operation action maps
//! and Service Definition Files (SDFs) from the filesystem with caching
//! capabilities for performance optimization.
//!
//! This module also contains the enriched method call data structures
//! that represent method calls enriched with IAM metadata from operation
//! action maps and Service Definition Files.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use crate::{
    enrichment::operation_fas_map::{FasContext, FasOperation},
    extraction::SdkMethodCallMetadata,
    service_configuration::ServiceConfiguration,
    SdkMethodCall, SdkType,
};
use convert_case::{Case, Casing};
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

/// Represents the reason why an action was added to a policy
#[derive(derive_new::new, Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct Reason {
    /// The original operation that was extracted
    pub operations: Vec<Arc<Operation>>,
}

#[derive(Debug, Clone, Serialize, Eq, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct Operation {
    /// Name of the service
    pub service: String,
    /// Name of the operation
    pub name: String,
    /// Source of the operation,
    pub source: OperationSource,
    /// Disallow struct construction, need to use Self::from_call or Operation::from(FasOperation)
    #[serde(skip)]
    _private: (),
}

impl Operation {
    #[cfg(test)]
    /// Convenience constructor for tests
    pub(crate) fn new(service: String, name: String, source: OperationSource) -> Self {
        Self {
            service,
            name,
            source,
            _private: (),
        }
    }

    pub(crate) fn service_operation_name(&self) -> String {
        format!("{}:{}", self.service, self.name)
    }

    pub(crate) fn context(&self) -> &[FasContext] {
        match &self.source {
            OperationSource::Fas(context) => context,
            _ => &[],
        }
    }

    pub(crate) async fn from_call(
        call: &SdkMethodCall,
        original_service_name: &str,
        service_cfg: &ServiceConfiguration,
        sdk: SdkType,
        service_reference_loader: &ServiceReferenceLoader,
    ) -> crate::errors::Result<Self> {
        let service = service_cfg
            .rename_service_service_reference(original_service_name)
            .to_string();
        #[allow(unknown_lints, convert_case_pascal)]
        let name = if sdk == SdkType::Boto3 {
            // Try to load service reference and look up the boto3 method mapping
            service_reference_loader
                .load(&service)
                .await?
                .and_then(|service_ref| {
                    log::debug!("Looking up method {}", call.name);
                    service_ref
                        .boto3_method_to_operation
                        .get(&call.name)
                        .map(|op| {
                            log::debug!("got {op:?}");
                            op.split(':').nth(1).unwrap_or(op).to_string()
                        })
                })
                // Fallback to PascalCase conversion if mapping not found
                // This should not be reachable, but if for some reason we cannot use the SDF,
                // we try converting to PascalCase, knowing that this is flawed in some cases:
                // think `AddRoleToDBInstance` (actual name)
                //   vs. `AddRoleToDbInstance` (converted name)
                .unwrap_or_else(|| call.name.to_case(Case::Pascal))
        } else {
            // For non-Boto3 SDKs we use the extracted name as-is
            call.name.clone()
        };

        Ok(match &call.metadata {
            None => Self {
                service,
                name,
                source: OperationSource::Provided,
                _private: (),
            },
            Some(metadata) => Self {
                service,
                name,
                source: OperationSource::Extracted(metadata.clone()),
                _private: (),
            },
        })
    }
}

impl From<FasOperation> for Operation {
    fn from(fas_op: FasOperation) -> Self {
        Self {
            service: fas_op.service,
            name: fas_op.operation,
            source: OperationSource::Fas(fas_op.context),
            _private: (),
        }
    }
}

// Custom PartialEq and Hash implementations for Operation:

// We consider operations to be equal when they would produce the same action in a policy.
// I.e., same operation and same context used for the condition. Directly relevant to FAS expansion.
impl PartialEq for Operation {
    fn eq(&self, other: &Self) -> bool {
        self.service == other.service
            && self.name == other.name
            && self.context() == other.context()
    }
}

impl std::hash::Hash for Operation {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.service.hash(state);
        self.name.hash(state);
        self.context().hash(state);
    }
}

/// Custom serializer for extracted metadata that flattens the structure
fn serialize_extracted_metadata<S>(
    metadata: &SdkMethodCallMetadata,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(2))?;
    map.serialize_entry("Expr", &metadata.expr)?;
    map.serialize_entry("Location", &metadata.location)?;
    map.end()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum OperationSource {
    /// Operation extracted from source files
    Extracted(SdkMethodCallMetadata),
    /// Operation provided (no metadata available)
    Provided,
    /// Operation comes from FAS expansion
    Fas(Vec<FasContext>),
}

impl Serialize for OperationSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Extracted(metadata) => serialize_extracted_metadata(metadata, serializer),
            Self::Provided => serializer.serialize_str("Provided"),
            Self::Fas(_) => serializer.serialize_str("FAS"),
        }
    }
}

/// Explanations for why actions have been included in a policy, with documentation for
/// concepts leading to inclusion (such as FAS expansion)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Explanations {
    /// Explanation for inclusion of an action
    pub explanation_for_action: BTreeMap<String, Explanation>,
    /// Documentation of concepts used in the explanation for an action
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub documentation: Vec<&'static str>,
}

impl Explanations {
    const FAS: &str =
        "The explanation contains an operation added due to Forward Access Sessions (FAS). See https://docs.aws.amazon.com/IAM/latest/UserGuide/access_forward_access_sessions.html.";

    pub(crate) fn new(explanations: BTreeMap<String, Explanation>) -> Self {
        let mut documentation: Vec<&'static str> = vec![];
        for explanation in explanations.values() {
            for reason in &explanation.reasons {
                for op in &reason.operations {
                    match op.source {
                        OperationSource::Extracted(_) | OperationSource::Provided => (),
                        OperationSource::Fas(_) => documentation.push(Self::FAS),
                    }
                }
            }
        }
        documentation.dedup();
        Self {
            explanation_for_action: explanations,
            documentation,
        }
    }
}

/// Represents an explanation for why an action was added to a policy
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash, JsonSchema, Default)]
// Don't print the `"Reasons":` key, treat this as just a JSON array.
#[serde(transparent)]
pub struct Explanation {
    /// The reasons this action was added (can have multiple reasons for the same action)
    pub reasons: Vec<Reason>,
}

impl Explanation {
    pub(crate) fn merge(&mut self, other: Self) {
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
    use crate::enrichment::operation_fas_map::FasContext;

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

    #[test]
    fn test_operation_custom_equality_same_operation_different_sources() {
        // Test that operations with same service, name, and context are equal regardless of source
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let op2 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(Vec::new()), // Empty context
        );

        // Should be equal because they have same service, name, and context (both empty)
        assert_eq!(op1, op2);
        assert_eq!(op2, op1); // Symmetric
    }

    #[test]
    fn test_operation_custom_equality_different_contexts() {
        // Test that operations with different contexts are NOT equal
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided, // Empty context
        );

        let context = vec![FasContext::new(
            "kms:ViaService".to_string(),
            vec!["s3.us-east-1.amazonaws.com".to_string()],
        )];
        let op2 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(context),
        );

        // Should NOT be equal because they have different contexts
        assert_ne!(op1, op2);
        assert_ne!(op2, op1); // Symmetric
    }

    #[test]
    fn test_operation_custom_equality_same_contexts() {
        // Test that operations with same contexts are equal
        let context1 = vec![FasContext::new(
            "kms:ViaService".to_string(),
            vec!["s3.us-east-1.amazonaws.com".to_string()],
        )];
        let context2 = vec![FasContext::new(
            "kms:ViaService".to_string(),
            vec!["s3.us-east-1.amazonaws.com".to_string()],
        )];

        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(context1),
        );

        let op2 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(context2),
        );

        // Should be equal because they have same service, name, and context
        assert_eq!(op1, op2);
        assert_eq!(op2, op1); // Symmetric
    }

    #[test]
    fn test_operation_custom_equality_different_services() {
        // Test that operations with different services are NOT equal
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let op2 = Operation::new(
            "kms".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        // Should NOT be equal because they have different services
        assert_ne!(op1, op2);
        assert_ne!(op2, op1); // Symmetric
    }

    #[test]
    fn test_operation_custom_equality_different_names() {
        // Test that operations with different names are NOT equal
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let op2 = Operation::new(
            "s3".to_string(),
            "PutObject".to_string(),
            OperationSource::Provided,
        );

        // Should NOT be equal because they have different operation names
        assert_ne!(op1, op2);
        assert_ne!(op2, op1); // Symmetric
    }

    #[test]
    fn test_operation_custom_hash_consistency() {
        // Test that equal operations have the same hash
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let op2 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(Vec::new()), // Empty context
        );

        // Equal operations should have the same hash
        assert_eq!(op1, op2);

        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher1 = DefaultHasher::new();
        op1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        op2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2, "Equal operations should have the same hash");
    }

    #[test]
    fn test_operation_custom_hash_different_for_unequal() {
        // Test that unequal operations typically have different hashes
        let op1 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Provided,
        );

        let context = vec![FasContext::new(
            "kms:ViaService".to_string(),
            vec!["s3.us-east-1.amazonaws.com".to_string()],
        )];
        let op2 = Operation::new(
            "s3".to_string(),
            "GetObject".to_string(),
            OperationSource::Fas(context),
        );

        // Unequal operations should typically have different hashes
        assert_ne!(op1, op2);

        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher1 = DefaultHasher::new();
        op1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        op2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        // Note: Hash collisions are possible but unlikely for this test case
        assert_ne!(
            hash1, hash2,
            "Unequal operations should typically have different hashes"
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
    use crate::{
        enrichment::mock_remote_service_reference::setup_mock_server_with_loader_without_operation_to_action_mapping,
        service_configuration::load_service_configuration, Location,
    };
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
        let metadata = SdkMethodCallMetadata::new(
            "s3.get_object(Bucket='my-bucket')".to_string(),
            Location::new(PathBuf::from("test.py"), (10, 5), (10, 79)),
        )
        .with_receiver("s3".to_string());

        SdkMethodCall {
            name: "get_object".to_string(),
            possible_services: vec!["s3".to_string()],
            metadata: Some(metadata),
        }
    }

    #[tokio::test]
    async fn test_reason_extracted_with_location() {
        let service_cfg = load_service_configuration().unwrap();
        let (_, service_reference_loader) =
            setup_mock_server_with_loader_without_operation_to_action_mapping().await;
        let call = mock_sdk_method_call();

        let reason = Reason::new(vec![Arc::new(
            Operation::from_call(
                &call,
                "s3",
                &service_cfg,
                SdkType::Boto3,
                &service_reference_loader,
            )
            .await
            .unwrap(),
        )]);

        assert_eq!(reason.operations[0].name, "GetObject");
        assert_eq!(reason.operations[0].service, "s3");
        match &reason.operations[0].source {
            OperationSource::Extracted(metadata) => {
                assert_eq!(metadata.expr, "s3.get_object(Bucket='my-bucket')");
                assert_eq!(metadata.location.to_gnu_format(), "test.py:10.5-10.79");
            }
            _ => panic!("Expected Extracted variant"),
        }

        let json = serde_json::to_string(&reason).unwrap();
        // Verify the location is serialized as a string in GNU format
        assert!(json.contains("\"Location\":\"test.py:10.5-10.79\""));
    }

    #[test]
    fn test_operation_source_extracted_serialization() {
        let metadata = SdkMethodCallMetadata::new(
            "dynamodb.get_item(\n        TableName='my-table',\n        Key={'id': {'S': '123'}}\n    )".to_string(),
            Location::new(PathBuf::from("iam-policy-autopilot-cli/tests/resources/test_example.py"), (19, 5), (22, 5)),
        )
        .with_receiver("dynamodb".to_string());

        let source = OperationSource::Extracted(metadata);
        let json = serde_json::to_string(&source).unwrap();

        // Verify the custom serialization format
        assert!(json.contains("\"Expr\":\"dynamodb.get_item(\\n        TableName='my-table',\\n        Key={'id': {'S': '123'}}\\n    )\""));
        assert!(json.contains(
            "\"Location\":\"iam-policy-autopilot-cli/tests/resources/test_example.py:19.5-22.5\""
        ));
        // Should not contain nested "Source" key
        assert!(!json.contains("\"Source\""));
    }

    #[test]
    fn test_operation_source_provided_serialization() {
        let source = OperationSource::Provided;
        let json = serde_json::to_string(&source).unwrap();

        // Verify the custom serialization format
        assert_eq!(json, "\"Provided\"");
    }

    #[test]
    fn test_operation_source_fas_serialization() {
        use crate::enrichment::operation_fas_map::FasContext;

        let fas_context = vec![FasContext::new(
            "kms:ViaService".to_string(),
            vec!["ssm.${region}.amazonaws.com".to_string()],
        )];
        let source = OperationSource::Fas(fas_context);
        let json = serde_json::to_string(&source).unwrap();

        // Verify the custom serialization format - should be just "FAS", not nested
        assert_eq!(json, "\"FAS\"");
    }

    #[tokio::test]
    async fn test_operation_methods() {
        let service_cfg = load_service_configuration().unwrap();
        let (_, service_reference_loader) =
            setup_mock_server_with_loader_without_operation_to_action_mapping().await;

        {
            let call = SdkMethodCall {
                name: "decrypt".to_string(),
                possible_services: vec!["kms".to_string()],
                metadata: None,
            };
            let op = Operation::from_call(
                &call,
                "kms",
                &service_cfg,
                SdkType::Boto3,
                &service_reference_loader,
            )
            .await
            .unwrap();
            assert_eq!(op.service_operation_name(), "kms:Decrypt");
            assert_eq!(op.context(), &[]);
        }

        {
            let expr = "kms.decrypt(...)".to_string();
            let metadata = SdkMethodCallMetadata::new(
                expr.clone(),
                Location::new(PathBuf::new(), (1, 1), (1, expr.len() + 1)),
            )
            .with_receiver("kms".to_string());
            let call = SdkMethodCall {
                name: "decrypt".to_string(),
                possible_services: vec!["kms".to_string()],
                metadata: Some(metadata),
            };
            let op = Operation::from_call(
                &call,
                "kms",
                &service_cfg,
                SdkType::Boto3,
                &service_reference_loader,
            )
            .await
            .unwrap();
            assert_eq!(op.service_operation_name(), "kms:Decrypt");
            assert_eq!(op.context(), &[]);
        }

        {
            let context = vec![FasContext::new(
                "kms:ViaService".to_string(),
                vec!["ssm.${region}.amazonaws.com".to_string()],
            )];
            let fas_operation =
                FasOperation::new("Decrypt".to_string(), "kms".to_string(), context.clone());
            let op = Operation::from(fas_operation);
            assert_eq!(op.service_operation_name(), "kms:Decrypt");
            assert_eq!(op.context(), &context);
        }
    }
}
