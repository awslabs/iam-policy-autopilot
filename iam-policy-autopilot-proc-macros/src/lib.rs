//! Proc-macro crate for IAM Policy Autopilot telemetry.
//!
//! This crate houses proc-macros related to telemetry. Each macro implementation
//! lives in its own module so new macros can be added without growing a single file.
//!
//! Currently provides:
//! - [`TelemetryEvent`] — derive macro for automatic telemetry event generation
//!   (see [`telemetry`] module documentation for full usage details).

mod telemetry;

use proc_macro::TokenStream;

/// Derive macro for generating `ToTelemetryEvent` implementations.
///
/// See the [`telemetry`] module for full attribute documentation, usage examples,
/// and supported field modes.
#[proc_macro_derive(TelemetryEvent, attributes(telemetry))]
pub fn derive_telemetry_event(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    telemetry::derive_telemetry_event_impl(input).into()
}
