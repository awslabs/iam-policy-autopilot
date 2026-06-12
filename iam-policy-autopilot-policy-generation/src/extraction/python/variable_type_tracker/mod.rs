//! Variable type tracking for boto3 clients and resources
//!
//! This module tracks boto3 client and resource assignments to improve
//! SDK method call extraction precision when variables are passed across
//! function boundaries.
//!
//! ## Not Yet Supported
//!
//! - **Function return values**: `def create_client(): return boto3.client('s3')`
//! - **Class attributes**: `self.client = boto3.client('s3')`

mod lookup;
mod tracking;
mod types;

pub(crate) use types::VariableTypeTracker;

#[cfg(test)]
mod tests;
