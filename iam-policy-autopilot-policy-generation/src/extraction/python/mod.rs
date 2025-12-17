//! SDK method extraction and disambiguation for Python
pub(crate) mod extractor;

pub(crate) mod boto3_resources_model;
pub(crate) mod common;
pub(crate) mod disambiguation;
pub(crate) mod node_kinds;
pub(crate) mod paginator_extractor;
pub(crate) mod resource_direct_calls_extractor;
pub(crate) mod waiters_extractor;

#[cfg(test)]
mod disambiguation_tests;
