//! Telemetry event data model.
//!
//! Defines the structure for telemetry events, parameters, and their encoding
//! into HTTP headers for transmission to the service reference endpoint.

use std::fmt;

/// Represents a single telemetry event emitted per CLI command or MCP tool invocation.
///
/// Each event captures which command was run, which parameters were used,
/// and the tool version. Events are encoded as HTTP headers for transmission.
#[derive(Debug, Clone)]
pub struct TelemetryEvent {
    /// The command or tool name (e.g., "generate-policies", "mcp:generate_application_policies")
    pub command: String,
    /// List of recorded parameters with their telemetry-safe values
    pub params: Vec<TelemetryParam>,
    /// The tool version (from `CARGO_PKG_VERSION`)
    pub version: String,
}

impl TelemetryEvent {
    /// Create a new telemetry event for a given command.
    ///
    /// The version is automatically populated from the crate version.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            params: Vec::new(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Add a parameter to this telemetry event.
    #[must_use]
    pub fn with_param(mut self, name: impl Into<String>, value: TelemetryValue) -> Self {
        self.params.push(TelemetryParam {
            name: name.into(),
            value,
        });
        self
    }

    /// Record a boolean parameter (e.g., whether a flag was set).
    #[must_use]
    pub fn with_bool(self, name: impl Into<String>, value: bool) -> Self {
        self.with_param(name, TelemetryValue::Bool(value))
    }

    /// Record a string parameter (e.g., language name, transport type).
    #[must_use]
    pub fn with_str(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.with_param(name, TelemetryValue::Str(value.into()))
    }

    /// Record an optional string parameter. If `None`, records `"none"`.
    #[must_use]
    pub fn with_optional_str(self, name: impl Into<String>, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.with_str(name, v),
            None => self.with_str(name, "none"),
        }
    }

    /// Record a presence-only parameter (whether it was provided, not the value).
    #[must_use]
    pub fn with_presence(self, name: impl Into<String>, is_present: bool) -> Self {
        self.with_bool(name, is_present)
    }

    /// Record a list parameter as a comma-separated string of values.
    /// Only records the values themselves (e.g., service names), never user content.
    #[must_use]
    pub fn with_list(self, name: impl Into<String>, values: &[String]) -> Self {
        if values.is_empty() {
            self.with_str(name, "none")
        } else {
            self.with_str(name, values.join(","))
        }
    }

    /// Encode this event as a list of HTTP header key-value pairs.
    ///
    /// Header format:
    /// - `X-Ipa-Command` → command name
    /// - `X-Ipa-Version` → tool version
    /// - `X-Ipa-P-{name}` → parameter value
    #[must_use]
    pub fn to_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::with_capacity(self.params.len() + 2);
        headers.push(("X-Ipa-Command".to_string(), self.command.clone()));
        headers.push(("X-Ipa-Version".to_string(), self.version.clone()));

        for param in &self.params {
            let header_name = format!("X-Ipa-P-{}", param.name);
            headers.push((header_name, param.value.to_string()));
        }

        headers
    }
}

/// A single telemetry parameter: a name and its telemetry-safe value.
#[derive(Debug, Clone)]
pub struct TelemetryParam {
    /// Parameter name (e.g., "region", "pretty", "language")
    pub name: String,
    /// Telemetry-safe value
    pub value: TelemetryValue,
}

/// Telemetry-safe parameter value.
///
/// We only record:
/// - Boolean presence (whether a parameter was provided)
/// - Enum/fixed values (language names, transport types, service names)
/// - Never user-supplied content (paths, ARNs, account IDs, policy content)
#[derive(Debug, Clone)]
pub enum TelemetryValue {
    /// Boolean value (parameter presence or flag value)
    Bool(bool),
    /// String value (enum/fixed values like language names, service names)
    Str(String),
}

impl fmt::Display for TelemetryValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Str(s) => write!(f, "{s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_new_has_version() {
        let event = TelemetryEvent::new("test-command");
        assert_eq!(event.command, "test-command");
        assert!(!event.version.is_empty());
        assert!(event.params.is_empty());
    }

    #[test]
    fn test_event_with_bool() {
        let event = TelemetryEvent::new("cmd").with_bool("pretty", true);
        assert_eq!(event.params.len(), 1);
        assert_eq!(event.params[0].name, "pretty");
        assert_eq!(event.params[0].value.to_string(), "true");
    }

    #[test]
    fn test_event_with_str() {
        let event = TelemetryEvent::new("cmd").with_str("language", "python");
        assert_eq!(event.params.len(), 1);
        assert_eq!(event.params[0].name, "language");
        assert_eq!(event.params[0].value.to_string(), "python");
    }

    #[test]
    fn test_event_with_optional_str_some() {
        let event = TelemetryEvent::new("cmd").with_optional_str("language", Some("go"));
        assert_eq!(event.params[0].value.to_string(), "go");
    }

    #[test]
    fn test_event_with_optional_str_none() {
        let event = TelemetryEvent::new("cmd").with_optional_str("language", None);
        assert_eq!(event.params[0].value.to_string(), "none");
    }

    #[test]
    fn test_event_with_presence() {
        let event = TelemetryEvent::new("cmd")
            .with_presence("region", true)
            .with_presence("account", false);
        assert_eq!(event.params[0].value.to_string(), "true");
        assert_eq!(event.params[1].value.to_string(), "false");
    }

    #[test]
    fn test_event_with_list() {
        let services = vec!["s3".to_string(), "ec2".to_string()];
        let event = TelemetryEvent::new("cmd").with_list("service_hints", &services);
        assert_eq!(event.params[0].value.to_string(), "s3,ec2");
    }

    #[test]
    fn test_event_with_list_empty() {
        let event = TelemetryEvent::new("cmd").with_list("service_hints", &[]);
        assert_eq!(event.params[0].value.to_string(), "none");
    }

    #[test]
    fn test_event_chaining() {
        let event = TelemetryEvent::new("generate-policies")
            .with_presence("source_files", true)
            .with_presence("region", false)
            .with_bool("pretty", true)
            .with_str("language", "python");

        assert_eq!(event.params.len(), 4);
        assert_eq!(event.command, "generate-policies");
    }

    #[test]
    fn test_to_headers() {
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("pretty", true)
            .with_str("language", "python");

        let headers = event.to_headers();

        // Should have command + version + 2 params = 4 headers
        assert_eq!(headers.len(), 4);

        assert_eq!(headers[0].0, "X-Ipa-Command");
        assert_eq!(headers[0].1, "generate-policies");

        assert_eq!(headers[1].0, "X-Ipa-Version");
        assert!(!headers[1].1.is_empty());

        assert_eq!(headers[2].0, "X-Ipa-P-pretty");
        assert_eq!(headers[2].1, "true");

        assert_eq!(headers[3].0, "X-Ipa-P-language");
        assert_eq!(headers[3].1, "python");
    }

    #[test]
    fn test_to_headers_empty_params() {
        let event = TelemetryEvent::new("cmd");
        let headers = event.to_headers();

        // Should have only command + version = 2 headers
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].0, "X-Ipa-Command");
        assert_eq!(headers[1].0, "X-Ipa-Version");
    }

    #[test]
    fn test_telemetry_value_display() {
        assert_eq!(TelemetryValue::Bool(true).to_string(), "true");
        assert_eq!(TelemetryValue::Bool(false).to_string(), "false");
        assert_eq!(TelemetryValue::Str("python".to_string()).to_string(), "python");
    }
}
