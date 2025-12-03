//! AWS SDK Waiter model parser
//!
//! This module parses `waiters-2.json` files from botocore to provide
//! mappings from waiter names to their underlying SDK operations.
//! Waiters are available across all AWS SDKs (Python boto3, JavaScript/TypeScript, Go, etc.)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
