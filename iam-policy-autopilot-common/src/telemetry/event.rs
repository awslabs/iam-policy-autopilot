//! Telemetry event data model.
//!
//! Defines the JSON payload structure sent to the telemetry Lambda endpoint.
//! The payload matches the schema validated by the backend:
//!
//! ```json
//! {
//!   "command": "generate-policies",
//!   "version": "0.1.4",
//!   "installation_id": "550e8400-e29b-41d4-a716-446655440000",
//!   "params": { "language": "python", "pretty": true },
//!   "result": { "success": true, "num_policies_generated": 2, "services_used": ["s3", "dynamodb"] }
//! }
//! ```

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// Represents a single telemetry event emitted per CLI command or MCP tool invocation.
///
/// The event is serialized as a JSON payload and sent via POST to the telemetry endpoint.
/// It captures which command was run, which parameters were used, the tool version,
/// a persistent session ID, and (after execution) the result outcome.
#[derive(Debug, Clone, Serialize)]
pub struct TelemetryEvent {
    /// The command or tool name (e.g., "generate-policies", "mcp-tool-generate-policies")
    pub command: String,
    /// The tool version (from `CARGO_PKG_VERSION`)
    pub version: String,
    /// A persistent session UUID for counting unique installations
    pub installation_id: String,
    /// Recorded parameters with their telemetry-safe values
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, Value>>,
    /// Result data populated after command execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<HashMap<String, Value>>,
}

impl TelemetryEvent {
    /// Create a new telemetry event for a given command.
    ///
    /// The version and installation_id are automatically populated.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            installation_id: super::config::installation_id(),
            params: None,
            result: None,
        }
    }

    // --- Parameter recording methods (builder pattern) ---

    /// Record a boolean parameter (e.g., whether a flag was set).
    #[must_use]
    pub fn with_bool(mut self, name: impl Into<String>, value: bool) -> Self {
        self.params
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::Bool(value));
        self
    }

    /// Record a string parameter (e.g., language name, transport type).
    #[must_use]
    pub fn with_str(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::String(value.into()));
        self
    }

    /// Record presence generically. Called by `#[derive(TelemetryEvent)]` for `#[telemetry(presence)]` fields.
    ///
    /// Handles `Vec<T>` → `!is_empty()`, `Option<T>` → `is_some()`, and other types → `true`.
    #[must_use]
    pub fn with_telemetry_presence(self, name: &str, value: &impl TelemetryFieldPresence) -> Self {
        value.record_presence(self, name)
    }

    /// Record a numeric parameter (e.g., count of items in a Vec).
    #[must_use]
    pub fn with_number(mut self, name: impl Into<String>, value: usize) -> Self {
        self.params
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::Number(serde_json::Number::from(value)));
        self
    }

    /// Record a list parameter as a JSON array of strings.
    /// Only records the values themselves (e.g., service names), never user content.
    #[must_use]
    pub fn with_list(mut self, name: impl Into<String>, values: &[String]) -> Self {
        let json_values: Vec<Value> = values.iter().map(|v| Value::String(v.clone())).collect();
        self.params
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::Array(json_values));
        self
    }

    // --- Result recording methods (builder pattern) ---

    /// Set whether the command succeeded (builder pattern).
    #[must_use]
    pub fn with_result_success(mut self, success: bool) -> Self {
        self.set_result_success(success);
        self
    }

    /// Set the number of policies generated (builder pattern).
    #[must_use]
    pub fn with_result_num_policies(mut self, count: usize) -> Self {
        self.set_result_num_policies(count);
        self
    }

    /// Record a string result (builder pattern).
    #[must_use]
    pub fn with_result_str(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.set_result_str(name, value);
        self
    }

    /// Record a list result (builder pattern).
    #[must_use]
    pub fn with_result_list(mut self, name: impl Into<String>, values: &[String]) -> Self {
        self.set_result_list(name, values);
        self
    }

    // --- In-place mutation methods ---

    /// Set whether the command succeeded (in-place mutation).
    pub fn set_result_success(&mut self, success: bool) {
        self.result
            .get_or_insert_with(HashMap::new)
            .insert("success".to_string(), Value::Bool(success));
    }

    /// Set the number of policies generated (in-place mutation).
    pub fn set_result_num_policies(&mut self, count: usize) {
        self.result.get_or_insert_with(HashMap::new).insert(
            "num_policies_generated".to_string(),
            Value::Number(serde_json::Number::from(count)),
        );
    }

    /// Set a string parameter (in-place mutation).
    pub fn set_str(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.params
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::String(value.into()));
    }

    /// Set a string result (in-place mutation).
    pub fn set_result_str(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.result
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::String(value.into()));
    }

    /// Set a numeric result (in-place mutation).
    pub fn set_result_number(&mut self, name: impl Into<String>, value: usize) {
        self.result
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::Number(serde_json::Number::from(value)));
    }

    /// Set a list result as a JSON array of strings (in-place mutation).
    pub fn set_result_list(&mut self, name: impl Into<String>, values: &[String]) {
        let json_values: Vec<Value> = values.iter().map(|v| Value::String(v.clone())).collect();
        self.result
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), Value::Array(json_values));
    }

    /// Merge fields from a [`TelemetrySpanSnapshot`] into the result section.
    ///
    /// String fields become JSON strings; numeric fields become JSON numbers;
    /// set fields become JSON arrays of strings.
    /// Existing result fields (e.g., `success`) are preserved.
    pub fn merge_result_span(&mut self, span: &super::span::TelemetrySpanSnapshot) {
        let result = self.result.get_or_insert_with(HashMap::new);
        for (k, v) in &span.strings {
            result.insert(k.clone(), Value::String(v.clone()));
        }
        for (k, v) in &span.numbers {
            result.insert(k.clone(), Value::Number(serde_json::Number::from(*v)));
        }
        for (k, values) in &span.sets {
            let json_values: Vec<Value> = values.iter().map(|v| Value::String(v.clone())).collect();
            result.insert(k.clone(), Value::Array(json_values));
        }
    }

    /// Serialize this event to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Trait for types that can record their presence as a telemetry parameter.
/// Used by `#[telemetry(presence)]` fields in `#[derive(TelemetryEvent)]`.
pub trait TelemetryFieldPresence {
    /// Record whether this field is "present" (non-empty, non-None).
    fn record_presence(&self, event: TelemetryEvent, name: &str) -> TelemetryEvent;
}

impl<T> TelemetryFieldPresence for Vec<T> {
    fn record_presence(&self, event: TelemetryEvent, name: &str) -> TelemetryEvent {
        event.with_bool(name, !self.is_empty())
    }
}

impl<T> TelemetryFieldPresence for Option<T> {
    fn record_presence(&self, event: TelemetryEvent, name: &str) -> TelemetryEvent {
        event.with_bool(name, self.is_some())
    }
}

impl TelemetryFieldPresence for bool {
    fn record_presence(&self, event: TelemetryEvent, name: &str) -> TelemetryEvent {
        event.with_bool(name, *self)
    }
}

impl TelemetryFieldPresence for String {
    fn record_presence(&self, event: TelemetryEvent, name: &str) -> TelemetryEvent {
        event.with_bool(name, !self.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn test_event_new_defaults() {
        let event = TelemetryEvent::new("test-command");
        assert_eq!(event.command, "test-command");
        assert!(!event.version.is_empty());
        assert!(!event.installation_id.is_empty());
        assert!(event.params.is_none());
        assert!(event.result.is_none());
    }

    // =========================================================================
    // Parameter recording — parameterized
    // =========================================================================

    #[rstest]
    #[case::bool_true("pretty", true)]
    #[case::bool_false("debug", false)]
    fn test_with_bool(#[case] key: &str, #[case] value: bool) {
        let event = TelemetryEvent::new("cmd").with_bool(key, value);
        assert_eq!(event.params.unwrap()[key], serde_json::json!(value));
    }

    #[rstest]
    #[case::language("language", "python")]
    #[case::format("format", "json")]
    fn test_with_str(#[case] key: &str, #[case] value: &str) {
        let event = TelemetryEvent::new("cmd").with_str(key, value);
        assert_eq!(event.params.unwrap()[key], serde_json::json!(value));
    }

    #[rstest]
    #[case::non_empty(vec!["s3".into(), "ec2".into()], serde_json::json!(["s3", "ec2"]))]
    #[case::empty(vec![], serde_json::json!([]))]
    fn test_with_list(#[case] values: Vec<String>, #[case] expected: serde_json::Value) {
        let event = TelemetryEvent::new("cmd").with_list("hints", &values);
        assert_eq!(event.params.unwrap()["hints"], expected);
    }

    // =========================================================================
    // Result recording — parameterized (builder + mutation)
    // =========================================================================

    #[rstest]
    #[case::success_true(true)]
    #[case::success_false(false)]
    fn test_result_success(#[case] success: bool) {
        let event = TelemetryEvent::new("cmd").with_result_success(success);
        assert_eq!(event.result.unwrap()["success"], serde_json::json!(success));
    }

    #[rstest]
    #[case::zero_builder(0, false)]
    #[case::three_builder(3, false)]
    #[case::five_mutation(5, true)]
    fn test_result_num_policies(#[case] count: usize, #[case] use_mutation: bool) {
        let event = if use_mutation {
            let mut e = TelemetryEvent::new("cmd");
            e.set_result_num_policies(count);
            e
        } else {
            TelemetryEvent::new("cmd").with_result_num_policies(count)
        };
        assert_eq!(
            event.result.unwrap()["num_policies_generated"],
            serde_json::json!(count)
        );
    }

    // =========================================================================
    // Chaining + JSON serialization
    // =========================================================================

    #[test]
    fn test_chaining_and_json_roundtrip() {
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("pretty", true)
            .with_str("language", "python")
            .with_result_success(true)
            .with_result_num_policies(2);

        // Builder chaining produces correct counts
        assert_eq!(event.params.as_ref().unwrap().len(), 2);
        assert_eq!(event.result.as_ref().unwrap().len(), 2);

        // JSON roundtrip — correct values and no unexpected keys
        let parsed: serde_json::Value = serde_json::from_str(&event.to_json().unwrap()).unwrap();

        assert_eq!(parsed["command"], "generate-policies");
        assert!(parsed["installation_id"].is_string());
        assert_eq!(parsed["params"]["pretty"], true);
        assert_eq!(parsed["params"]["language"], "python");
        assert_eq!(parsed["result"]["success"], true);
        assert_eq!(parsed["result"]["num_policies_generated"], 2);

        let keys: std::collections::HashSet<_> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in &keys {
            assert!(
                ["command", "version", "installation_id", "params", "result"].contains(key),
                "Unexpected key: {key}"
            );
        }
    }
}
