//! AWS SDK Waiter model parser
//!
//! This module parses `waiters-2.json` files from botocore to provide
//! mappings from waiter names to their underlying SDK operations.
//! Waiters are available across all AWS SDKs (Python boto3, JavaScript/TypeScript, Go, etc.)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::embedded_data::EmbeddedServiceData;
use crate::providers::JsonProvider;

/// Minimal waiter entry that only captures the operation field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WaiterEntry {
    /// AWS operation that this waiter polls (PascalCase, e.g., "DescribeInstances")
    pub(crate) operation: String,
    // Note: delay and maxAttempts fields are ignored during deserialization
}

/// Complete waiters description for a service
///
/// Describes all waiters available for an AWS service, including their
/// underlying operations and metadata parsed from waiters-2.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WaitersDescription {
    pub(crate) version: u32,
    pub(crate) waiters: HashMap<String, WaiterEntry>,
}

/// Registry of all waiters across all AWS services
#[derive(Debug, Clone)]
pub struct WaitersRegistry;

impl WaitersRegistry {
    /// Load waiters from embedded data for a specific service
    ///
    /// # Arguments
    /// * `service_name` - Service name (e.g., "ec2", "s3")
    /// * `api_version` - API version (e.g., "2016-11-15", "2006-03-01")
    ///
    /// # Returns
    /// HashMap of waiter names to waiter entries, or None if no waiters found
    pub async fn load_waiters_from_embedded(service_name: &str, api_version: &str) -> Option<HashMap<String, WaiterEntry>> {
        let waiters_data = EmbeddedServiceData::get_waiters_raw(service_name, api_version)?;
        
        let waiters_str = std::str::from_utf8(&waiters_data).ok()?;
        
        match JsonProvider::parse::<WaitersDescription>(waiters_str).await {
            Ok(waiters_desc) => Some(waiters_desc.waiters),
            Err(_) => None, // Silently skip on parse error
        }
    }
}
