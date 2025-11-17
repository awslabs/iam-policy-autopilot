//! IAM Policy Autopilot Core API Interface

mod generate_policies;
mod extract_sdk_calls;
pub use extract_sdk_calls::extract_sdk_calls;
pub use generate_policies::generate_policies;
mod common;
pub mod model;