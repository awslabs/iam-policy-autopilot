//! Service Definition Files (SDF) loader with caching capabilities.
//!
//! This module provides functionality to load AWS service definition files
//! from the filesystem with exact service name matching and caching for
//! performance optimization.
//!
//! Caching is abstracted behind the [`ServiceCache`] trait, with platform-specific
//! implementations:
//! - **Native** (`NativeServiceCache`): In-memory TTL + optional filesystem persistence.
//! - **WASM** (`WasmServiceCache`): In-memory only, no expiry (session-scoped).

use crate::enrichment::Context;
use crate::errors::ExtractorError;
use crate::providers::JsonProvider;
use crate::enrichment::http_client::{self, HttpGet};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::HashMap;
#[cfg(not(feature = "wasm"))]
use std::path::PathBuf;
#[cfg(not(feature = "wasm"))]
use std::time::{Duration, SystemTime};
#[cfg(not(feature = "wasm"))]
use tokio::fs;
use tokio::sync::{OnceCell, RwLock};

type OperationName = String;
#[cfg(not(feature = "wasm"))]
const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";
#[cfg(not(feature = "wasm"))]
const DEFAULT_CACHE_DURATION_IN_SECONDS: u64 = 300;
/// Service Reference data structure
///
/// Represents the complete service reference loaded from service reference endpoint.
/// These files contain metadata about AWS services including actions,
/// resources, condition keys, and actions authorized by an operation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceReference {
    /// Action mapping to resources
    pub(crate) actions: HashMap<String, Action>,
    /// Service name
    pub(crate) service_name: String,
    /// Resource mapping to ARN patterns
    pub(crate) resources: HashMap<String, Vec<String>>,
    /// Operation to authorized action mapping
    /// Note: Only partial service and operations have this data
    pub(crate) operation_to_authorized_actions: Option<HashMap<OperationName, Operation>>,
    /// Map from boto method names (snake_case) to operation names
    pub(crate) boto3_method_to_operation: HashMap<String, String>,
}

impl<'de> Deserialize<'de> for ServiceReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TempServiceReference {
            #[serde(rename = "Actions")]
            #[serde(default)]
            #[serde(deserialize_with = "deserialize_actions_map")]
            actions: HashMap<String, Action>,
            #[serde(rename = "Name")]
            name: String,
            #[serde(rename = "Resources", deserialize_with = "deserialize_resources_map")]
            #[serde(default)]
            resources: HashMap<String, Vec<String>>,
            #[serde(rename = "Operations")]
            #[serde(default)]
            operations: Vec<Operation>,
        }

        let temp = TempServiceReference::deserialize(deserializer)?;
        let mut operations = temp.operations;

        if !operations.is_empty() {
            for operation in &mut operations {
                operation.name = format!("{}:{}", temp.name.to_lowercase(), operation.name);
                if operation.authorized_actions.is_empty() {
                    // Fallback which uses the operation as the action, when there are no AuthorizedActions
                    let authorized_action = AuthorizedAction {
                        name: operation.name.clone(),
                        service: temp.name.clone(),
                        context: None,
                    };
                    operation.authorized_actions.insert(0, authorized_action);
                } else {
                    for authorized_action in &mut operation.authorized_actions {
                        authorized_action.name = format!(
                            "{}:{}",
                            authorized_action.service.to_lowercase(),
                            authorized_action.name
                        );
                    }
                }
            }
        }

        let operation_to_authorized_actions: Option<HashMap<OperationName, Operation>> =
            if operations.is_empty() {
                None
            } else {
                Some(
                    operations
                        .into_iter()
                        .map(|operation| (operation.name.clone(), operation))
                        .collect(),
                )
            };

        // Build boto3_method_to_operation map
        let mut boto3_method_to_operation = HashMap::new();
        if let Some(ref op_map) = operation_to_authorized_actions {
            for (operation_name, operation) in op_map {
                for sdk_method in &operation.sdk {
                    // Only add entries for Boto3 package where service name matches
                    if sdk_method.package == "Boto3" && sdk_method.name == temp.name {
                        boto3_method_to_operation
                            .insert(sdk_method.method.clone(), operation_name.clone());
                    }
                }
            }
        }

        Ok(Self {
            actions: temp.actions,
            service_name: temp.name,
            resources: temp.resources,
            operation_to_authorized_actions,
            boto3_method_to_operation,
        })
    }
}

// Models an action in service reference
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Action {
    #[serde(rename = "Name")]
    pub(crate) name: String,
    #[serde(rename = "Resources")]
    #[serde(default)]
    pub(crate) resources: Vec<String>,
    #[serde(rename = "ActionConditionKeys")]
    pub(crate) condition_keys: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ServiceReferenceContext {
    pub(crate) key: String,
    pub(crate) values: Vec<String>,
}

impl Context for ServiceReferenceContext {
    fn key(&self) -> &str {
        &self.key
    }

    fn values(&self) -> &[String] {
        &self.values
    }
}

fn deserialize_context<'de, D>(deserializer: D) -> Result<Option<ServiceReferenceContext>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: HashMap<String, Vec<String>> = HashMap::deserialize(deserializer)?;

    // Take the first key-value pair from the map
    // Context should have exactly one key-value pair
    if let Some((key, values)) = map.into_iter().next() {
        Ok(Some(ServiceReferenceContext { key, values }))
    } else {
        // If empty map, return None
        Ok(None)
    }
}

// Tracks actions each operation may authorize
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AuthorizedAction {
    pub(crate) name: String,
    pub(crate) service: String,
    #[serde(default, deserialize_with = "deserialize_context")]
    pub(crate) context: Option<ServiceReferenceContext>,
}

// Part of construct in the operation to authorized action map
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SdkMethod {
    pub(crate) name: String,
    pub(crate) method: String,
    pub(crate) package: String,
}

// Used for deserializing operation to authorized action map
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Operation {
    #[serde(rename = "Name")]
    pub(crate) name: OperationName,
    #[serde(rename = "AuthorizedActions")]
    #[serde(default)]
    pub(crate) authorized_actions: Vec<AuthorizedAction>,
    #[serde(rename = "SDK")]
    #[serde(default)]
    pub(crate) sdk: Vec<SdkMethod>,
}

fn deserialize_actions_map<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, Action>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct TempResource {
        #[serde(rename = "Name")]
        name: String,
    }

    #[derive(Deserialize)]
    struct TempAction {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Resources")]
        #[serde(default)]
        resources: Vec<TempResource>,
        #[serde(rename = "ActionConditionKeys")]
        #[serde(default)]
        condition_keys: Vec<String>,
    }

    let actions: Vec<TempAction> = Vec::deserialize(deserializer)?;
    Ok(actions
        .into_iter()
        .map(|temp_action| {
            let action = Action {
                name: temp_action.name.clone(),
                resources: temp_action.resources.into_iter().map(|r| r.name).collect(),
                condition_keys: temp_action.condition_keys,
            };
            (temp_action.name, action)
        })
        .collect())
}

fn deserialize_resources_map<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    // Resource within a Service Reference
    #[derive(Deserialize)]
    struct ServiceResource {
        #[serde(rename = "Name")]
        // Resource name (e.g., "certificate", "bucket")
        pub(crate) name: String,
        #[serde(rename = "ARNFormats")]
        // ARN format patterns for this resource
        pub(crate) arn_formats: Vec<String>,
    }
    let resources: Vec<ServiceResource> = Vec::deserialize(deserializer)?;
    Ok(resources
        .into_iter()
        .map(|resource| (resource.name, resource.arn_formats))
        .collect())
}

/// represents the top level mapping returned by service reference
/// to resolve the url for target service
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceReferenceMapping {
    // represents the top level service reference mapping
    pub(crate) service_reference_mapping: HashMap<String, String>,
}

fn deserialize_service_reference_mapping(
    value: Value,
) -> crate::errors::Result<HashMap<String, String>> {
    #[derive(Deserialize)]
    struct ServiceEntry {
        service: String,
        url: String,
    }

    let entries: Vec<ServiceEntry> = serde_json::from_value(value)?;
    let mut map = HashMap::new();
    for entry in entries {
        if entry.url.is_empty() {
            return Err(ExtractorError::service_reference_parse_error_with_source(
                &entry.service,
                format!("Empty URL in service reference mapping for '{}'", entry.service),
                std::io::Error::new(std::io::ErrorKind::InvalidData, "empty URL"),
            ));
        }
        map.insert(entry.service, entry.url);
    }
    Ok(map)
}

/// Service Reference Loader
///
/// This loader provides functionality to load AWS service definition files
/// with exact service name matching and thread-safe caching. Service names
/// must match exactly between input and Service Reference Name (case-sensitive).
#[derive(Debug)]

pub(crate) struct RemoteServiceReferenceLoader {
    client: Box<dyn HttpGet>,
    service_reference_mapping: OnceCell<ServiceReferenceMapping>,
    cache: ServiceCache,
    mapping_url: String,
    #[cfg_attr(feature = "wasm", allow(dead_code))]
    disable_file_system_cache: bool,
}

// ---------------------------------------------------------------------------
// ServiceCache — platform-specific caching strategy
// ---------------------------------------------------------------------------

/// Abstraction over the in-memory service reference cache.
///
/// Native builds store entries with a timestamp for TTL expiry.
/// WASM builds store entries without expiry (session-scoped).
#[derive(Debug)]
struct ServiceCache {
    inner: RwLock<HashMap<String, CacheEntry>>,
}

#[cfg(not(feature = "wasm"))]
type CacheEntry = (ServiceReference, SystemTime);

#[cfg(feature = "wasm")]
type CacheEntry = ServiceReference;

impl ServiceCache {
    fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Returns a cached entry if it exists and has not expired.
    async fn get(&self, service_name: &str) -> Option<ServiceReference> {
        let guard = self.inner.read().await;
        let entry = guard.get(service_name)?;

        #[cfg(not(feature = "wasm"))]
        {
            let (cached, timestamp) = entry;
            if let Ok(elapsed) = SystemTime::now().duration_since(*timestamp) {
                if elapsed < Duration::from_secs(DEFAULT_CACHE_DURATION_IN_SECONDS) {
                    return Some(cached.clone());
                }
            }
            None
        }

        #[cfg(feature = "wasm")]
        {
            Some(entry.clone())
        }
    }

    /// Insert or update a cache entry.
    async fn insert(&self, service_name: String, service_ref: ServiceReference) {
        #[cfg(not(feature = "wasm"))]
        {
            self.inner
                .write()
                .await
                .insert(service_name, (service_ref, SystemTime::now()));
        }

        #[cfg(feature = "wasm")]
        {
            self.inner.write().await.insert(service_name, service_ref);
        }
    }
}

const DEFAULT_MAPPING_URL: &str = "https://servicereference.us-east-1.amazonaws.com";
const MAPPING_URL_ENV_VAR: &str = "IAM_POLICY_AUTOPILOT_SERVICE_REFERENCE_URL";

impl RemoteServiceReferenceLoader {
    pub(crate) fn new(disable_file_system_cache: bool) -> crate::errors::Result<Self> {
        let mapping_url =
            std::env::var(MAPPING_URL_ENV_VAR).unwrap_or_else(|_| DEFAULT_MAPPING_URL.to_string());
        Ok(Self {
            client: http_client::create_http_client()?,
            service_reference_mapping: OnceCell::new(),
            cache: ServiceCache::new(),
            mapping_url,
            disable_file_system_cache,
        })
    }

    /// Creates a loader that always returns `None` for any service.
    /// Useful in tests that don't need real SDF data.
    #[cfg(test)]

    pub(crate) fn empty_loader_for_tests() -> crate::errors::Result<Self> {
        let loader = Self {
            client: http_client::create_http_client()?,
            service_reference_mapping: OnceCell::new(),
            cache: ServiceCache::new(),
            mapping_url: String::new(),
            disable_file_system_cache: true,
        };
        // Pre-initialize with an empty mapping so no network call is ever made.
        let _ = loader
            .service_reference_mapping
            .set(ServiceReferenceMapping {
                service_reference_mapping: HashMap::new(),
            });
        Ok(loader)
    }

    /// Sets a custom mapping URL (e.g., a mock server) and resets the cached mapping
    /// so the next call fetches from the new URL.
    #[cfg(test)]

    pub(crate) fn with_mapping_url(mut self, url: String) -> Self {
        self.mapping_url = url;
        self.service_reference_mapping = OnceCell::new();
        self
    }

    async fn get_or_init_mapping(&self) -> crate::errors::Result<&ServiceReferenceMapping> {
        self.service_reference_mapping
            .get_or_try_init(|| async {
                let json_text = self.client.get_text(&self.mapping_url).await?;

                let json_value: serde_json::Value =
                    serde_json::from_str(&json_text).map_err(|e| {
                        ExtractorError::service_reference_parse_error_with_source(
                            &self.mapping_url,
                            "Failed to parse JSON from service reference endpoint".to_string(),
                            e,
                        )
                    })?;

                let mapping = deserialize_service_reference_mapping(json_value).map_err(|e| {
                    ExtractorError::service_reference_parse_error_with_source(
                        &self.mapping_url,
                        "Failed to deserialize service reference mapping".to_string(),
                        e,
                    )
                })?;

                Ok(ServiceReferenceMapping {
                    service_reference_mapping: mapping,
                })
            })
            .await
    }

    #[cfg(not(feature = "wasm"))]
    fn get_cache_dir() -> PathBuf {
        // not using tempfile crate
        // instead, using the std to resolve temp dir and then manage the file itself
        // file deletion is delegated to the OS.
        let cache_dir = std::env::temp_dir().join(IAM_POLICY_AUTOPILOT);
        let _ = std::fs::create_dir_all(&cache_dir);
        cache_dir
    }

    #[cfg(not(feature = "wasm"))]
    fn get_cache_path(service_name: &str) -> PathBuf {
        Self::get_cache_dir().join(format!("{service_name}.json"))
    }

    #[cfg(not(feature = "wasm"))]
    async fn is_cache_valid(path: &PathBuf) -> bool {
        if let Ok(metadata) = fs::metadata(path).await {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                    return elapsed < Duration::from_secs(DEFAULT_CACHE_DURATION_IN_SECONDS);
                }
            }
        }
        false
    }

    pub(crate) async fn get_resource_arns(
        &self,
        service_name: &str,
        resource_type: &str,
    ) -> Option<Vec<String>> {
        if let Ok(Some(service_ref)) = self.load(service_name).await {
            service_ref.resources.get(resource_type).cloned()
        } else {
            None
        }
    }

    pub(crate) async fn load(
        &self,
        service_name: &str,
    ) -> crate::errors::Result<Option<ServiceReference>> {
        // Check in-memory cache
        if let Some(cached) = self.cache.get(service_name).await {
            return Ok(Some(cached));
        }

        // Filesystem cache — not available on WASM
        #[cfg(not(feature = "wasm"))]
        {
            let cache_path = Self::get_cache_path(service_name);
            if !self.disable_file_system_cache && Self::is_cache_valid(&cache_path).await {
                if let Ok(content) = fs::read_to_string(&cache_path).await {
                    if let Ok(service_ref) =
                        JsonProvider::parse::<ServiceReference>(&content).await
                    {
                        self.cache
                            .insert(service_name.to_string(), service_ref.clone())
                            .await;
                        return Ok(Some(service_ref));
                    }
                }
            }
        }

        let mapping = self.get_or_init_mapping().await?;
        let service_url = mapping.service_reference_mapping.get(service_name);

        match service_url {
            Some(service_url) => {
                let service_reference_content = self
                    .client
                    .get_text(service_url)
                    .await
                    .map_err(|e| {
                        ExtractorError::service_reference_parse_error_with_source(
                            service_name,
                            "Failed to fetch service reference data".to_string(),
                            e,
                        )
                    })?;

                let service_ref: ServiceReference = JsonProvider::parse(&service_reference_content)
                    .await
                    .map_err(|e| {
                        ExtractorError::service_reference_parse_error_with_source(
                            service_name,
                            format!(
                                "Failed to parse service reference content. Detailed error: {e}"
                            ),
                            e,
                        )
                    })?;
                // Persist to filesystem cache (native only)
                #[cfg(not(feature = "wasm"))]
                if !self.disable_file_system_cache {
                    let cache_path = Self::get_cache_path(service_name);
                    let _ = fs::write(&cache_path, &service_reference_content).await;
                }
                self.cache
                    .insert(service_name.to_string(), service_ref.clone())
                    .await;
                Ok(Some(service_ref))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrichment::mock_remote_service_reference;

    #[tokio::test]
    async fn test_remote_loader_new() {
        let loader = RemoteServiceReferenceLoader::new(false);
        assert!(loader.is_ok());

        let loader = loader.unwrap();
        assert!(loader.cache.inner.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_create_client() {
        let client = RemoteServiceReferenceLoader::create_client();
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_cache_functionality() {
        let (_mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        let loader = std::sync::Arc::new(loader);
        let mut handles = vec![];

        // Spawn multiple concurrent tasks
        for i in 0..5 {
            let loader_clone = loader.clone();
            let handle = tokio::spawn(async move {
                let result = loader_clone.load("s3").await;
                assert!(result.is_ok());
                assert_eq!(result.unwrap().unwrap().service_name, "s3");
                i
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify cache is populated
        let cached = loader.cache.get("s3").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().service_name, "s3");

        // Verify cache is unique
        assert_eq!(loader.cache.inner.read().await.len(), 1);
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_memory_cache_expiry() {
        let (_mock_server, loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;

        // Load and cache s3
        let result = loader.load("s3").await;
        assert!(result.is_ok());

        // Manually expire the cache by setting old timestamp
        let expired_time =
            SystemTime::now() - Duration::from_secs(DEFAULT_CACHE_DURATION_IN_SECONDS + 1);
        if let Some(entry) = loader.cache.inner.write().await.get_mut("s3") {
            entry.1 = expired_time;
        }

        // Load again - should fetch fresh data, not use expired cache
        let result = loader.load("s3").await;
        assert!(result.is_ok());

        // Verify cache has fresh timestamp
        let guard = loader.cache.inner.read().await;
        let cached = guard.get("s3");
        assert!(cached.is_some());
        let (_, timestamp) = cached.unwrap();
        let elapsed = SystemTime::now().duration_since(*timestamp).unwrap();
        assert!(elapsed < Duration::from_secs(10));
    }

    // Integration test - requires network access
    #[tokio::test]
    #[ignore] // Use `cargo test -- --ignored` to run this test
    async fn test_load_from_service_reference_success() {
        let loader = RemoteServiceReferenceLoader::new(false).unwrap();
        let result = loader.load("s3").await;

        match result {
            Ok(service_ref) => {
                assert_eq!(service_ref.as_ref().unwrap().service_name, "s3");
                assert!(!service_ref.as_ref().unwrap().actions.is_empty());
                assert!(!service_ref.as_ref().unwrap().resources.is_empty());

                // Test caching - second call should use cache
                let cached_result = loader.load("s3").await;
                assert!(cached_result.is_ok());
                assert_eq!(cached_result.unwrap().unwrap().service_name, "s3");
            }
            Err(e) => {
                println!("Network test failed (expected in CI): {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Use `cargo test -- --ignored` to run this test
    async fn test_load_nonexistent_service() {
        let loader = RemoteServiceReferenceLoader::new(false).unwrap();
        let result = loader.load("nonexistent-service-xyz").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none())
    }

    #[tokio::test]
    async fn test_deserialize_service_reference_mapping() {
        let json = serde_json::json!([
            {"service": "s3", "url": "https://example.com/s3.json"},
            {"service": "ec2", "url": "https://example.com/ec2.json"}
        ]);

        let result = deserialize_service_reference_mapping(json);
        assert!(result.is_ok());

        let mapping = result.unwrap();
        assert_eq!(mapping.len(), 2);
        assert!(mapping.contains_key("s3"));
        assert!(mapping.contains_key("ec2"));
        assert_eq!(mapping["s3"].as_str(), "https://example.com/s3.json");
        assert_eq!(mapping["ec2"].as_str(), "https://example.com/ec2.json");
    }

    #[tokio::test]
    async fn test_deserialize_service_reference_mapping_invalid_url() {
        let json = serde_json::json!([
            {"service": "s3", "url": "invalid-url"}
        ]);

        let result = deserialize_service_reference_mapping(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deserialize_service_reference_mapping_empty() {
        let json = serde_json::json!([]);

        let result = deserialize_service_reference_mapping(json);
        assert!(result.is_ok());

        let mapping = result.unwrap();
        assert!(mapping.is_empty());
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_get_cache_dir() {
        let cache_dir = RemoteServiceReferenceLoader::get_cache_dir();
        assert!(cache_dir.ends_with("IAMPolicyAutopilot"));
        assert!(cache_dir.exists());
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_get_cache_path() {
        let cache_path = RemoteServiceReferenceLoader::get_cache_path("s3");
        assert!(
            cache_path.ends_with("IAMPolicyAutopilot/s3.json")
                || cache_path.ends_with("IAMAutoPilot\\s3.json")
        );
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_is_cache_valid_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/file.json");
        assert!(!RemoteServiceReferenceLoader::is_cache_valid(&path).await);
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_is_cache_valid_fresh() {
        let cache_path = RemoteServiceReferenceLoader::get_cache_path("test_fresh");
        let _ = fs::write(&cache_path, "test content").await;

        assert!(RemoteServiceReferenceLoader::is_cache_valid(&cache_path).await);
        let _ = fs::remove_file(&cache_path).await;
    }

    #[tokio::test]
    #[cfg(not(feature = "wasm"))]
    async fn test_filesystem_cache() {
        let (_mock_server, mut loader) =
            mock_remote_service_reference::setup_mock_server_with_loader().await;
        // setup_mock_server_with_loader disables file system cache by default
        loader.disable_file_system_cache = false;
        let cache_path = RemoteServiceReferenceLoader::get_cache_path("s3");
        let _ = fs::remove_file(&cache_path).await;

        let result = loader.load("s3").await;
        assert!(result.is_ok());
        assert!(cache_path.exists());

        let cached_content = fs::read_to_string(&cache_path).await;
        assert!(cached_content.is_ok());
        let _ = fs::remove_file(&cache_path).await;
    }

    #[tokio::test]
    async fn test_service_reference_deserialization() {
        let json = r#"{
            "Name": "s3",
            "Actions": [
                {
                    "Name": "GetObject",
                    "Resources": [{"Name": "object"}]
                }
            ],
            "Resources": [
                {
                    "Name": "bucket",
                    "ARNFormats": ["arn:aws:s3:::bucket-name"]
                }
            ],
            "Operations": [
                {
                    "Name": "GetObject",
                    "AuthorizedActions": [
                        {
                            "Name": "GetObject",
                            "Service": "s3"
                        }
                    ]
                }
            ]
        }"#;

        let service_ref: ServiceReference = serde_json::from_str(json).unwrap();
        assert_eq!(service_ref.service_name, "s3");
        assert_eq!(service_ref.actions.len(), 1);
        assert!(service_ref.actions.contains_key("GetObject"));
        assert_eq!(service_ref.resources.len(), 1);
        assert!(service_ref.resources.contains_key("bucket"));

        // Test operation name prefixing
        let operations = service_ref.operation_to_authorized_actions.unwrap();
        assert!(operations.contains_key("s3:GetObject"));
        let operation = &operations["s3:GetObject"];
        assert_eq!(operation.name, "s3:GetObject");
        assert_eq!(operation.authorized_actions[0].name, "s3:GetObject");
    }

    #[tokio::test]
    async fn test_service_reference_deserialization_empty_authorized_actions() {
        let json = r#"{
            "Name": "s3",
            "Actions": [
                {
                    "Name": "GetObject",
                    "Resources": [{"Name": "object"}]
                }
            ],
            "Resources": [
                {
                    "Name": "bucket",
                    "ARNFormats": ["arn:aws:s3:::bucket-name"]
                }
            ],
            "Operations": [
                {
                    "Name": "GetObject"
                }
            ]
        }"#;

        let service_ref: ServiceReference = serde_json::from_str(json).unwrap();
        assert_eq!(service_ref.service_name, "s3");
        assert_eq!(service_ref.actions.len(), 1);
        assert!(service_ref.actions.contains_key("GetObject"));
        assert_eq!(service_ref.resources.len(), 1);
        assert!(service_ref.resources.contains_key("bucket"));

        // Test operation name prefixing
        let operations = service_ref.operation_to_authorized_actions.unwrap();
        assert!(operations.contains_key("s3:GetObject"));
        let operation = &operations["s3:GetObject"];
        assert_eq!(operation.name, "s3:GetObject");
        // Ensure the default authorized action is populated
        assert_eq!(operation.authorized_actions[0].name, "s3:GetObject");
    }

    #[tokio::test]
    async fn test_context_deserialization() {
        let json = r#"{"Context": {
            "iam:PassedToService": ["access-analyzer.amazonaws.com"]
        }}"#;

        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_context")]
            #[serde(rename = "Context")]
            context: Option<ServiceReferenceContext>,
        }

        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert!(result.context.is_some());
        let context = result.context.unwrap();
        assert_eq!(context.key, "iam:PassedToService");
        assert_eq!(context.values, vec!["access-analyzer.amazonaws.com"]);
    }

    #[tokio::test]
    async fn test_context_deserialization_multiple_values() {
        let json = r#"{"Context": {
            "iam:PassedToService": ["service1.amazonaws.com", "service2.amazonaws.com"]
        }}"#;

        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_context")]
            #[serde(rename = "Context")]
            context: Option<ServiceReferenceContext>,
        }

        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert!(result.context.is_some());
        let context = result.context.unwrap();
        assert_eq!(context.key, "iam:PassedToService");
        assert_eq!(
            context.values,
            vec!["service1.amazonaws.com", "service2.amazonaws.com"]
        );
    }

    #[tokio::test]
    async fn test_context_deserialization_empty() {
        let json = r#"{"Context": {}}"#;

        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_context")]
            #[serde(rename = "Context")]
            context: Option<ServiceReferenceContext>,
        }

        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert!(result.context.is_none());
    }

    #[tokio::test]
    async fn test_authorized_action_with_context() {
        let json = r#"{
            "Name": "access-analyzer",
            "Actions": [],
            "Resources": [],
            "Operations": [
                {
                    "Name": "StartPolicyGeneration",
                    "AuthorizedActions": [
                        {
                            "Name": "PassRole",
                            "Service": "iam",
                            "Context": {
                                "iam:PassedToService": ["access-analyzer.amazonaws.com"]
                            }
                        }
                    ]
                }
            ]
        }"#;

        let service_ref: ServiceReference = serde_json::from_str(json).unwrap();
        let operations = service_ref.operation_to_authorized_actions.unwrap();
        let operation = &operations["access-analyzer:StartPolicyGeneration"];
        let authorized_action = &operation.authorized_actions[0];

        assert!(authorized_action.context.is_some());
        let context = authorized_action.context.as_ref().unwrap();
        assert_eq!(context.key, "iam:PassedToService");
        assert_eq!(context.values, vec!["access-analyzer.amazonaws.com"]);
    }

    #[tokio::test]
    async fn test_authorized_action_without_context() {
        let json = r#"{
            "Name": "s3",
            "Actions": [],
            "Resources": [],
            "Operations": [
                {
                    "Name": "GetObject",
                    "AuthorizedActions": [
                        {
                            "Name": "GetObject",
                            "Service": "s3"
                        }
                    ]
                }
            ]
        }"#;

        let service_ref: ServiceReference = serde_json::from_str(json).unwrap();
        let operations = service_ref.operation_to_authorized_actions.unwrap();
        let operation = &operations["s3:GetObject"];
        let authorized_action = &operation.authorized_actions[0];

        assert!(authorized_action.context.is_none());
    }

    #[tokio::test]
    async fn test_network_failure_error_message() {
        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url("http://127.0.0.1:1".to_string());

        let result = loader.load("s3").await;
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Failed to connect to the service reference endpoint"),
            "Error should contain connection failure guidance, got: {err}"
        );
        assert!(
            err.contains("http://127.0.0.1:1"),
            "Error should contain the URL that was attempted, got: {err}"
        );
        assert!(
            err.contains("HTTPS_PROXY"),
            "Error should mention HTTPS_PROXY env var, got: {err}"
        );
    }
}
