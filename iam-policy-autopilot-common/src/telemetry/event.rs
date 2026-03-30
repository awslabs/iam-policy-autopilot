//! Telemetry event data model.
//!
//! Defines the JSON payload structure sent to the telemetry Lambda endpoint.
//! The payload matches the schema validated by the backend:
//!
//! ```json
//! {
//!   "command": "generate-policies",
//!   "version": "0.1.4",
//!   "anonymous_id": "550e8400-e29b-41d4-a716-446655440000",
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
    pub anonymous_id: String,
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
    /// The version and anonymous_id are automatically populated.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            anonymous_id: super::config::anonymous_id(),
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

    // --- In-place mutation methods ---

    /// Set whether the command succeeded (in-place mutation).
    pub fn set_result_success(&mut self, success: bool) {
        self.result
            .get_or_insert_with(HashMap::new)
            .insert("success".to_string(), Value::Bool(success));
    }

    /// Set the number of policies generated (in-place mutation).
    pub fn set_result_num_policies(&mut self, count: usize) {
        self.result
            .get_or_insert_with(HashMap::new)
            .insert(
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

    #[test]
    fn test_event_new_has_version_and_anonymous_id() {
        let event = TelemetryEvent::new("test-command");
        assert_eq!(event.command, "test-command");
        assert!(!event.version.is_empty());
        assert!(event.params.is_none());
        assert!(event.result.is_none());
        assert!(!event.anonymous_id.is_empty());
    }

    #[test]
    fn test_event_with_bool() {
        let event = TelemetryEvent::new("cmd").with_bool("pretty", true);
        let params = event.params.expect("params should be Some");
        assert_eq!(params.get("pretty"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_event_with_str() {
        let event = TelemetryEvent::new("cmd").with_str("language", "python");
        let params = event.params.expect("params should be Some");
        assert_eq!(
            params.get("language"),
            Some(&Value::String("python".to_string()))
        );
    }

    #[test]
    fn test_event_with_list() {
        let services = vec!["s3".to_string(), "ec2".to_string()];
        let event = TelemetryEvent::new("cmd").with_list("service_hints", &services);
        let params = event.params.expect("params should be Some");
        let expected = Value::Array(vec![
            Value::String("s3".to_string()),
            Value::String("ec2".to_string()),
        ]);
        assert_eq!(params.get("service_hints"), Some(&expected));
    }

    #[test]
    fn test_event_with_list_empty() {
        let event = TelemetryEvent::new("cmd").with_list("service_hints", &[]);
        let params = event.params.expect("params should be Some");
        assert_eq!(
            params.get("service_hints"),
            Some(&Value::Array(Vec::new()))
        );
    }

    #[test]
    fn test_event_with_result_success() {
        let event = TelemetryEvent::new("cmd").with_result_success(true);
        let result = event.result.expect("result should be Some");
        assert_eq!(result.get("success"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_event_with_result_num_policies() {
        let event = TelemetryEvent::new("cmd").with_result_num_policies(3);
        let result = event.result.expect("result should be Some");
        assert_eq!(
            result.get("num_policies_generated"),
            Some(&Value::Number(serde_json::Number::from(3)))
        );
    }

    #[test]
    fn test_event_chaining_with_result() {
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("source_files", true)
            .with_str("language", "python")
            .with_result_success(true)
            .with_result_num_policies(2);

        assert_eq!(event.command, "generate-policies");
        assert!(!event.anonymous_id.is_empty());

        let params = event.params.expect("params should be Some");
        assert_eq!(params.len(), 2);

        let result = event.result.expect("result should be Some");
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("success"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_set_result_num_policies() {
        let mut event = TelemetryEvent::new("cmd");
        event.set_result_num_policies(5);
        let result = event.result.expect("result should be Some");
        assert_eq!(
            result.get("num_policies_generated"),
            Some(&Value::Number(serde_json::Number::from(5)))
        );
    }

    #[test]
    fn test_to_json_full() {
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("pretty", true)
            .with_str("language", "python")
            .with_result_success(true)
            .with_result_num_policies(2);

        let json = event.to_json().expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");

        assert_eq!(parsed["command"], "generate-policies");
        assert!(parsed["anonymous_id"].is_string());
        assert_eq!(parsed["params"]["pretty"], true);
        assert_eq!(parsed["params"]["language"], "python");
        assert_eq!(parsed["result"]["success"], true);
        assert_eq!(parsed["result"]["num_policies_generated"], 2);
    }

    #[test]
    fn test_json_only_contains_allowed_keys() {
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("pretty", true)
            .with_result_success(true);

        let json = event.to_json().expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");
        let obj = parsed.as_object().expect("should be object");

        let allowed_keys: std::collections::HashSet<&str> =
            ["command", "version", "anonymous_id", "params", "result"]
                .iter()
                .copied()
                .collect();

        for key in obj.keys() {
            assert!(
                allowed_keys.contains(key.as_str()),
                "Unexpected key in telemetry payload: {key}"
            );
        }
    }
}
