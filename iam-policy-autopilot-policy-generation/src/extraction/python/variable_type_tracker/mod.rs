//! Variable type tracking for boto3 clients and resources
//!
//! This module tracks boto3 client and resource assignments to improve
//! SDK method call extraction precision when variables are passed across
//! function boundaries.

mod lookup;
mod tracking;
mod types;

pub(crate) use types::VariableTypeTracker;

#[cfg(test)]
mod tests;
