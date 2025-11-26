//! Policy synthesis (deterministic JSON generation)

pub mod policy_builder;

pub use policy_builder::{build_inline_allow, build_single_statement};
