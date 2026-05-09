//! Service Definition Files (SDF) loader with caching capabilities.
//!
//! This module provides functionality to load AWS service definition files
//! from the filesystem with exact service name matching and caching for
//! performance optimization.

use crate::enrichment::Context;
use crate::errors::ExtractorError;
use crate::providers::JsonProvider;
use reqwest::{Client, Url};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime},
};
use tokio::fs;
use tokio::sync::RwLock;

type OperationName = String;
const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";

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

/// Represents a single entry in the service reference index
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceReferenceEntry {
    /// URL to fetch the full service reference
    pub(crate) url: Url,
    /// Epoch timestamp indicating when this service was last modified
    pub(crate) modified: u64,
}

/// represents the top level mapping returned by service reference
/// to resolve the url for target service
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceReferenceMapping {
    // represents the top level service reference mapping
    pub(crate) service_reference_mapping: HashMap<String, ServiceReferenceEntry>,
}

fn deserialize_service_reference_mapping(
    value: Value,
) -> crate::errors::Result<HashMap<String, ServiceReferenceEntry>> {
    #[derive(Deserialize)]
    struct ServiceEntry {
        service: String,
        url: String,
        /// Defaults to 0 for backward compatibility with index entries that
        /// predate the `modified` field.
        #[serde(default)]
        modified: u64,
    }

    let entries: Vec<ServiceEntry> = serde_json::from_value(value)?;
    let mut map = HashMap::new();
    for entry in entries {
        let url = Url::parse(&entry.url).map_err(|e| {
            ExtractorError::service_reference_parse_error_with_source(
                "RemoteServiceReferenceLoaderMappingInitialization",
                "Failed to parse service reference mapping",
                e,
            )
        })?;
        map.insert(
            entry.service,
            ServiceReferenceEntry {
                url,
                modified: entry.modified,
            },
        );
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
    client: Client,
    /// The index mapping; refreshed by `refresh_index()` before each enrichment run.
    service_reference_mapping: RwLock<Option<ServiceReferenceMapping>>,
    /// In-memory cache: service name → (parsed data, `modified` timestamp from the index).
    /// Entries are evicted by `refresh_index()` when the remote `modified` increases.
    service_cache: RwLock<HashMap<String, (ServiceReference, u64)>>,
    /// When the index was last successfully fetched. Used together with
    /// `refresh_interval` to skip redundant network calls.
    last_refreshed: RwLock<Option<SystemTime>>,
    /// Optional minimum interval between index refreshes. When `Some`, `refresh_index()`
    /// is a no-op if the index was already fetched within this window. `None` (the default)
    /// means refresh every invocation.
    refresh_interval: Option<Duration>,
    mapping_url: String,
    disable_file_system_cache: bool,
}

impl RemoteServiceReferenceLoader {
    pub(crate) fn new(disable_file_system_cache: bool) -> crate::errors::Result<Self> {
        Ok(Self {
            client: Self::create_client()?,
            service_reference_mapping: RwLock::new(None),
            service_cache: RwLock::new(HashMap::new()),
            last_refreshed: RwLock::new(None),
            refresh_interval: None,
            mapping_url: "https://servicereference.us-east-1.amazonaws.com".to_string(),
            disable_file_system_cache,
        })
    }

    /// Creates a loader that always returns `None` for any service.
    /// Useful in tests that don't need real SDF data.
    #[cfg(test)]
    pub(crate) fn empty_loader_for_tests() -> crate::errors::Result<Self> {
        let loader = Self {
            client: Self::create_client()?,
            service_reference_mapping: RwLock::new(Some(ServiceReferenceMapping {
                service_reference_mapping: HashMap::new(),
            })),
            service_cache: RwLock::new(HashMap::new()),
            last_refreshed: RwLock::new(None),
            refresh_interval: None,
            mapping_url: String::new(),
            disable_file_system_cache: true,
        };
        Ok(loader)
    }

    /// Sets a custom mapping URL (e.g., a mock server) and resets the cached mapping
    /// so the next call fetches from the new URL.
    #[cfg(test)]
    pub(crate) fn with_mapping_url(mut self, url: String) -> Self {
        self.mapping_url = url;
        self.service_reference_mapping = RwLock::new(None);
        self.last_refreshed = RwLock::new(None);
        self
    }

    /// Sets the minimum interval between index refreshes. When set,
    /// `refresh_index()` is a no-op if the index was already fetched
    /// within this window.
    #[cfg(test)]
    pub(crate) fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = Some(interval);
        self
    }

    /// Fetches the index from the remote endpoint and returns a `ServiceReferenceMapping`.
    async fn fetch_mapping(&self) -> crate::errors::Result<ServiceReferenceMapping> {
        let json_text = self
            .client
            .get(&self.mapping_url)
            .send()
            .await
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    "RemoteServiceReferenceLoaderMappingInitialization",
                    "Failed to send request".to_string(),
                    e,
                )
            })?
            .error_for_status()
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    "RemoteServiceReferenceLoaderMappingInitialization",
                    "Failed to fetch mapping".to_string(),
                    e,
                )
            })?
            .text()
            .await
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    "RemoteServiceReferenceLoaderMappingInitialization",
                    "Failed to read response".to_string(),
                    e,
                )
            })?;

        let json_value: serde_json::Value = serde_json::from_str(&json_text).map_err(|e| {
            ExtractorError::service_reference_parse_error_with_source(
                "RemoteServiceReferenceLoaderMappingInitialization",
                "Failed to parse JSON".to_string(),
                e,
            )
        })?;

        let mapping = deserialize_service_reference_mapping(json_value).map_err(|e| {
            ExtractorError::service_reference_parse_error_with_source(
                "RemoteServiceReferenceLoaderMappingInitialization",
                "Failed to deserialize mapping".to_string(),
                e,
            )
        })?;

        Ok(ServiceReferenceMapping {
            service_reference_mapping: mapping,
        })
    }

    /// Refresh the service index and invalidate any cached entries whose
    /// `modified` timestamp has increased. Call once per invocation, before
    /// the enrichment loop.
    ///
    /// When `refresh_interval` is set, the refresh is skipped if the index
    /// was already fetched within that window.
    pub(crate) async fn refresh_index(&self) -> crate::errors::Result<()> {
        // Skip if the index was recently refreshed and an interval is configured
        if let Some(interval) = self.refresh_interval {
            let last = *self.last_refreshed.read().await;
            if let Some(last) = last {
                if let Ok(elapsed) = SystemTime::now().duration_since(last) {
                    if elapsed < interval {
                        return Ok(());
                    }
                }
            }
        }

        let new_mapping = self.fetch_mapping().await?;

        // Evict entries whose `modified` timestamp in the new index is greater
        // than the `cached_modified` we stored when the entry was last fetched.
        // After eviction, the next `load()` call will re-fetch from remote.
        // (`last_refreshed` tracks when *this index fetch* happened, for
        // refresh-interval gating; `cached_modified` tracks the per-service
        // version from the index, for staleness detection.)
        let mut cache = self.service_cache.write().await;
        let stale_services: Vec<String> = cache
            .iter()
            .filter(|(name, (_, cached_modified))| {
                new_mapping
                    .service_reference_mapping
                    .get(name.as_str())
                    .is_some_and(|entry| entry.modified > *cached_modified)
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in &stale_services {
            cache.remove(name);
            if !self.disable_file_system_cache {
                let _ = fs::remove_file(Self::get_cache_path(name)).await;
                let _ = fs::remove_file(Self::get_cache_modified_path(name)).await;
            }
        }
        drop(cache);

        *self.service_reference_mapping.write().await = Some(new_mapping);
        *self.last_refreshed.write().await = Some(SystemTime::now());
        Ok(())
    }

    fn create_client() -> crate::errors::Result<Client> {
        let user_agent_suffix = if cfg!(feature = "integ-test") {
            "-integration-test"
        } else {
            ""
        };

        let user_agent = format!(
            "{}{}/{}",
            IAM_POLICY_AUTOPILOT,
            user_agent_suffix,
            env!("CARGO_PKG_VERSION")
        );
        Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    "RemoteServiceReferenceLoaderClientInitialization",
                    "Failed to create service reference client".to_string(),
                    e,
                )
            })
    }

    fn get_cache_dir() -> PathBuf {
        // not using tempfile crate
        // instead, using the std to resolve temp dir and then manage the file itself
        // file deletion is delegated to the OS.
        let cache_dir = std::env::temp_dir().join(IAM_POLICY_AUTOPILOT);
        let _ = std::fs::create_dir_all(&cache_dir);
        cache_dir
    }

    fn get_cache_path(service_name: &str) -> PathBuf {
        Self::get_cache_dir().join(format!("{service_name}.json"))
    }

    /// Path to the `.modified` sidecar for a cached service. Each service gets
    /// its own sidecar rather than a single index file so that individual
    /// entries can be written/evicted independently without locking a shared file.
    fn get_cache_modified_path(service_name: &str) -> PathBuf {
        Self::get_cache_dir().join(format!("{service_name}.modified"))
    }

    async fn read_cached_modified(service_name: &str) -> Option<u64> {
        let modified_path = Self::get_cache_modified_path(service_name);
        let content = fs::read_to_string(&modified_path).await.ok()?;
        content.trim().parse().ok()
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

    /// Load a service reference by name.
    ///
    /// Tries three sources in order:
    /// 1. In-memory cache (already validated by `refresh_index`)
    /// 2. Filesystem cache (validated against the index `modified` timestamp)
    /// 3. Remote fetch (updates both caches)
    ///
    /// If `refresh_index()` has not been called yet (e.g. callers outside the
    /// enrichment path like Terraform resource resolution), the index is lazily
    /// fetched here so the caller doesn't need to remember to call it.
    pub(crate) async fn load(
        &self,
        service_name: &str,
    ) -> crate::errors::Result<Option<ServiceReference>> {
        if let Some(hit) = self.load_from_memory_cache(service_name).await {
            return Ok(Some(hit));
        }

        self.ensure_mapping_initialized().await?;

        let entry = match self.resolve_index_entry(service_name).await {
            Some(e) => e,
            None => return Ok(None),
        };

        if let Some(hit) = self.load_from_filesystem_cache(service_name, &entry).await {
            return Ok(Some(hit));
        }

        self.fetch_and_cache_service(service_name, &entry).await
    }

    /// Returns the cached `ServiceReference` if present in the in-memory cache.
    async fn load_from_memory_cache(&self, service_name: &str) -> Option<ServiceReference> {
        self.service_cache
            .read()
            .await
            .get(service_name)
            .map(|(cached, _)| cached.clone())
    }

    /// Lazily initializes the index mapping if `refresh_index()` hasn't been
    /// called yet. This ensures callers outside the enrichment path (e.g.
    /// Terraform resource resolution) don't need to explicitly call
    /// `refresh_index()` first.
    async fn ensure_mapping_initialized(&self) -> crate::errors::Result<()> {
        if self.service_reference_mapping.read().await.is_none() {
            let fresh = self.fetch_mapping().await?;
            *self.service_reference_mapping.write().await = Some(fresh);
        }
        Ok(())
    }

    /// Looks up the service in the index mapping. Returns `None` if the
    /// mapping is empty (should not happen after `ensure_mapping_initialized`)
    /// or the service is not listed in the index.
    async fn resolve_index_entry(&self, service_name: &str) -> Option<ServiceReferenceEntry> {
        let guard = self.service_reference_mapping.read().await;
        guard
            .as_ref()
            .and_then(|m| m.service_reference_mapping.get(service_name).cloned())
    }

    /// Attempts to load from the filesystem cache. Returns `Some` only when
    /// the cached `modified` timestamp is still current.
    async fn load_from_filesystem_cache(
        &self,
        service_name: &str,
        entry: &ServiceReferenceEntry,
    ) -> Option<ServiceReference> {
        if self.disable_file_system_cache {
            return None;
        }
        let cached_modified = Self::read_cached_modified(service_name).await?;
        if cached_modified < entry.modified {
            return None;
        }
        let cache_path = Self::get_cache_path(service_name);
        let content = fs::read_to_string(&cache_path).await.ok()?;
        let service_ref = JsonProvider::parse::<ServiceReference>(&content)
            .await
            .ok()?;
        self.service_cache.write().await.insert(
            service_name.to_string(),
            (service_ref.clone(), cached_modified),
        );
        Some(service_ref)
    }

    /// Fetches the service reference from the remote URL and updates both
    /// the in-memory and filesystem caches.
    async fn fetch_and_cache_service(
        &self,
        service_name: &str,
        entry: &ServiceReferenceEntry,
    ) -> crate::errors::Result<Option<ServiceReference>> {
        let service_reference_content = self
            .client
            .get(entry.url.as_ref())
            .send()
            .await
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    service_name,
                    "Failed to fetch service reference data".to_string(),
                    e,
                )
            })?
            .text()
            .await
            .map_err(|e| {
                ExtractorError::service_reference_parse_error_with_source(
                    service_name,
                    "Failed to read service reference response".to_string(),
                    e,
                )
            })?;

        let service_ref: ServiceReference = JsonProvider::parse(&service_reference_content)
            .await
            .map_err(|e| {
            ExtractorError::service_reference_parse_error_with_source(
                service_name,
                format!("Failed to parse service reference content. Detailed error: {e}"),
                e,
            )
        })?;

        if !self.disable_file_system_cache {
            let cache_path = Self::get_cache_path(service_name);
            let _ = fs::write(&cache_path, &service_reference_content).await;
            let _ = fs::write(
                &Self::get_cache_modified_path(service_name),
                entry.modified.to_string(),
            )
            .await;
        }

        self.service_cache.write().await.insert(
            service_name.to_string(),
            (service_ref.clone(), entry.modified),
        );
        Ok(Some(service_ref))
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
        assert!(loader.service_cache.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_create_client() {
        let client = RemoteServiceReferenceLoader::create_client();
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_cache_functionality() {
        let (_, loader) = mock_remote_service_reference::setup_mock_server_with_loader().await;

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
        let cached = loader.service_cache.read().await.get("s3").cloned();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().0.service_name, "s3");

        // Verify cache is unique
        assert_eq!(loader.service_cache.read().await.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_cache_invalidation_via_refresh() {
        let (_, loader) = mock_remote_service_reference::setup_mock_server_with_loader().await;

        // Load and cache s3
        let result = loader.load("s3").await;
        assert!(result.is_ok());

        // Verify cache is populated with modified=1000 (from mock)
        let cached = loader.service_cache.read().await.get("s3").cloned();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().1, 1000);

        // Manually set a stale modified timestamp so refresh_index will evict it.
        // The mock index returns modified=1000, so setting cached to 0 means
        // 1000 > 0 → stale → evicted.
        if let Some(entry) = loader.service_cache.write().await.get_mut("s3") {
            entry.1 = 0;
        }

        // refresh_index should evict the stale entry
        loader.refresh_index().await.unwrap();
        assert!(
            loader.service_cache.read().await.get("s3").is_none(),
            "stale entry should be evicted after refresh_index"
        );

        // load() should re-fetch and re-populate the cache
        let result = loader.load("s3").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap().service_name, "s3");
        assert!(loader.service_cache.read().await.get("s3").is_some());
    }

    // Integration test - requires network access
    #[tokio::test]
    #[ignore] // Use `cargo test -- --ignored` to run this test
    async fn test_load_from_service_reference_success() {
        let loader = RemoteServiceReferenceLoader::new(false).unwrap();
        loader.refresh_index().await.unwrap();
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
        loader.refresh_index().await.unwrap();
        let result = loader.load("nonexistent-service-xyz").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none())
    }

    #[tokio::test]
    async fn test_deserialize_service_reference_mapping() {
        let json = serde_json::json!([
            {"service": "s3", "url": "https://example.com/s3.json", "modified": 100},
            {"service": "ec2", "url": "https://example.com/ec2.json", "modified": 200}
        ]);

        let result = deserialize_service_reference_mapping(json);
        assert!(result.is_ok());

        let mapping = result.unwrap();
        assert_eq!(mapping.len(), 2);
        assert!(mapping.contains_key("s3"));
        assert!(mapping.contains_key("ec2"));
        assert_eq!(mapping["s3"].url.as_str(), "https://example.com/s3.json");
        assert_eq!(mapping["s3"].modified, 100);
        assert_eq!(mapping["ec2"].url.as_str(), "https://example.com/ec2.json");
        assert_eq!(mapping["ec2"].modified, 200);
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
    async fn test_get_cache_dir() {
        let cache_dir = RemoteServiceReferenceLoader::get_cache_dir();
        assert!(cache_dir.ends_with("IAMPolicyAutopilot"));
        assert!(cache_dir.exists());
    }

    #[tokio::test]
    async fn test_get_cache_path() {
        let cache_path = RemoteServiceReferenceLoader::get_cache_path("s3");
        assert!(cache_path.ends_with("IAMPolicyAutopilot/s3.json"));
    }

    #[tokio::test]
    async fn test_get_cache_modified_path() {
        let modified_path = RemoteServiceReferenceLoader::get_cache_modified_path("s3");
        assert!(modified_path.ends_with("IAMPolicyAutopilot/s3.modified"));
    }

    #[tokio::test]
    async fn test_filesystem_cache() {
        let (_, mut loader) = mock_remote_service_reference::setup_mock_server_with_loader().await;
        // setup_mock_server_with_loader disables file system cache by default
        loader.disable_file_system_cache = false;
        let cache_path = RemoteServiceReferenceLoader::get_cache_path("s3");
        let modified_path = RemoteServiceReferenceLoader::get_cache_modified_path("s3");
        let _ = fs::remove_file(&cache_path).await;
        let _ = fs::remove_file(&modified_path).await;

        let result = loader.load("s3").await;
        assert!(result.is_ok());
        assert!(cache_path.exists());
        assert!(modified_path.exists());

        let cached_content = fs::read_to_string(&cache_path).await;
        assert!(cached_content.is_ok());
        let modified_content = fs::read_to_string(&modified_path).await;
        assert!(modified_content.is_ok());
        // modified should be parseable as u64
        assert!(modified_content.unwrap().trim().parse::<u64>().is_ok());

        let _ = fs::remove_file(&cache_path).await;
        let _ = fs::remove_file(&modified_path).await;
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
}

#[cfg(test)]
mod integ_tests {
    use super::*;
    use std::time::{Duration, SystemTime};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn index_request_count(requests: &[wiremock::Request]) -> usize {
        requests.iter().filter(|r| r.url.path() == "/").count()
    }

    fn service_request_count(requests: &[wiremock::Request], svc: &str) -> usize {
        let p = format!("/{svc}.json");
        requests.iter().filter(|r| r.url.path() == p).count()
    }

    /// Tests the full modified-based cache lifecycle on a single mock server:
    /// 1. Load caches the service data
    /// 2. Refresh with same modified → cache stays, no re-fetch
    /// 3. Refresh with bumped modified → cache evicted, next load re-fetches
    #[tokio::test]
    async fn test_modified_based_cache_invalidation() {
        let server = MockServer::start().await;
        let url = server.uri();

        // Index returns modified=1000
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", url), "modified": 1000}
            ])))
            .up_to_n_times(2) // first refresh + second refresh (same modified)
            .with_priority(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/s3.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Name": "s3",
                "Actions": [{"Name": "GetObject", "Resources": [{"Name": "object"}]}],
                "Resources": [{"Name": "object", "ARNFormats": ["arn:aws:s3:::*/*"]}],
                "Operations": []
            })))
            .mount(&server)
            .await;

        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(url.clone());

        // --- Phase 1: initial load ---
        loader.refresh_index().await.unwrap();
        let result = loader.load("s3").await.unwrap().unwrap();
        assert_eq!(result.service_name, "s3");

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(service_request_count(&reqs, "s3"), 1);

        // --- Phase 2: refresh with same modified → no eviction ---
        loader.refresh_index().await.unwrap();
        let _ = loader.load("s3").await.unwrap().unwrap();

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            service_request_count(&reqs, "s3"),
            1,
            "same modified should not re-fetch s3.json"
        );

        // --- Phase 3: bump modified to 2000 → eviction + re-fetch ---
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", url), "modified": 2000}
            ])))
            .with_priority(2)
            .mount(&server)
            .await;

        loader.refresh_index().await.unwrap();
        assert!(
            loader.service_cache.read().await.get("s3").is_none(),
            "s3 should be evicted after modified increased"
        );

        let _ = loader.load("s3").await.unwrap().unwrap();

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            service_request_count(&reqs, "s3"),
            2,
            "bumped modified should trigger re-fetch of s3.json"
        );
    }

    /// Tests the refresh interval behavior on a single mock server:
    /// 1. No interval → always refreshes
    /// 2. Long interval → skips refresh
    /// 3. Expired interval → refreshes again
    /// 4. Interval of 0 → always refreshes
    #[tokio::test]
    async fn test_refresh_interval_behavior() {
        let server = MockServer::start().await;
        let url = server.uri();

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"service": "s3", "url": format!("{}/s3.json", url), "modified": 1000}
            ])))
            .mount(&server)
            .await;

        // --- No interval: every call fetches ---
        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(url.clone());

        loader.refresh_index().await.unwrap();
        loader.refresh_index().await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            index_request_count(&reqs),
            2,
            "no interval → should fetch every time"
        );

        // --- Long interval: skip ---
        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(url.clone())
            .with_refresh_interval(Duration::from_secs(3600));

        loader.refresh_index().await.unwrap();
        loader.refresh_index().await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            index_request_count(&reqs),
            3,
            "within 3600s window → second call should skip"
        );

        // --- Backdate last_refreshed so interval is expired ---
        *loader.last_refreshed.write().await = Some(SystemTime::now() - Duration::from_secs(7200));
        loader.refresh_index().await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            index_request_count(&reqs),
            4,
            "expired interval → should fetch"
        );

        // --- Interval of 0: always fetches ---
        let loader = RemoteServiceReferenceLoader::new(true)
            .unwrap()
            .with_mapping_url(url)
            .with_refresh_interval(Duration::from_secs(0));

        loader.refresh_index().await.unwrap();
        loader.refresh_index().await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(
            index_request_count(&reqs),
            6,
            "interval=0 → should always fetch"
        );
    }
}
