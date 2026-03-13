//! IAM Policy Autopilot Common Library
//!
//! Shared utilities for IAM Policy Autopilot workspace crates.
//! Currently provides the telemetry framework for anonymous usage metrics.

#![deny(missing_docs)]
#![deny(unsafe_code)]
#![warn(clippy::all)]
#![allow(clippy::module_name_repetitions)]

/// Telemetry module for anonymous usage metrics collection.
pub mod telemetry;
