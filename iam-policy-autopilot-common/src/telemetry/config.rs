//! Persistent telemetry configuration.
//!
//! Manages the `~/.iam-policy-autopilot/telemetry.json` file which stores:
//! - `anonymousId`: A persistent UUID v4 that identifies this installation
//! - `telemetryChoice`: The user's telemetry preference (`notSet`, `enabled`, `disabled`)
//!
//! The file is created on first use with a freshly generated UUID.
//! If the file cannot be read or written, a new ephemeral UUID is generated
//! and telemetry continues without persistence (fire-and-forget principle).

use log::debug;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Name of the config directory under the user's home directory.
const CONFIG_DIR_NAME: &str = ".iam-policy-autopilot";

/// Name of the telemetry config file.
const CONFIG_FILE_NAME: &str = "telemetry.json";

/// Represents the user's persistent telemetry preference.
///
/// - `NotSet`: The user has not made a choice yet. The notice should be shown.
/// - `Enabled`: The user explicitly opted in.
/// - `Disabled`: The user explicitly opted out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelemetryChoice {
    /// User has not made a choice. Notice should be shown.
    NotSet,
    /// User explicitly opted in.
    Enabled,
    /// User explicitly opted out.
    Disabled,
}

impl Default for TelemetryChoice {
    fn default() -> Self {
        Self::NotSet
    }
}

impl std::fmt::Display for TelemetryChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSet => write!(f, "notSet"),
            Self::Enabled => write!(f, "enabled"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// Persistent telemetry configuration stored in `~/.iam-policy-autopilot/telemetry.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryConfig {
    /// A persistent UUID v4 identifying this installation.
    /// Used as `anonymous_id` in telemetry events to count unique installations
    /// without identifying individual users.
    pub anonymous_id: String,

    /// The user's telemetry preference.
    ///
    /// - `NotSet`: user hasn't made a choice; notice should be shown.
    /// - `Enabled`: user explicitly opted in.
    /// - `Disabled`: user explicitly opted out.
    ///
    /// This can be overridden by the `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable.
    #[serde(default)]
    pub telemetry_choice: TelemetryChoice,
}

impl TelemetryConfig {
    /// Create a new config with a fresh UUID and telemetry choice not set.
    fn new() -> Self {
        Self {
            anonymous_id: uuid::Uuid::new_v4().to_string(),
            telemetry_choice: TelemetryChoice::NotSet,
        }
    }
}

/// Get the path to the telemetry config file.
///
/// Returns `~/.iam-policy-autopilot/telemetry.json` on all platforms.
fn config_file_path() -> Option<PathBuf> {
    dirs_path().map(|dir| dir.join(CONFIG_FILE_NAME))
}

/// Get the path to the `.iam-policy-autopilot` directory.
fn dirs_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(CONFIG_DIR_NAME))
}

/// Get the user's home directory.
///
/// Uses `$HOME` on Unix/macOS and `%USERPROFILE%` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Process-level cache for the telemetry config.
///
/// Loaded once from disk on first access and reused for all subsequent reads.
/// Write operations (`set_telemetry_choice`) update both the cache and disk.
static CONFIG_CACHE: std::sync::OnceLock<TelemetryConfig> = std::sync::OnceLock::new();

/// Load the telemetry config from disk, or create and persist a new one.
///
/// This is called once per process (via `OnceLock`) and cached thereafter.
/// If the config file exists and is valid JSON, returns the parsed config.
/// If the file doesn't exist, creates a new config with a fresh UUID,
/// persists it (best-effort), and returns it.
fn load_or_create_config() -> TelemetryConfig {
    // Try to read existing config
    if let Some(path) = config_file_path() {
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<TelemetryConfig>(&content) {
                    Ok(config) => {
                        debug!("Loaded telemetry config from {}", path.display());
                        return config;
                    }
                    Err(e) => {
                        debug!("Failed to parse telemetry config (will recreate): {e}");
                    }
                },
                Err(e) => {
                    debug!("Failed to read telemetry config (will recreate): {e}");
                }
            }
        }
    }

    // Create new config and persist (best-effort)
    let config = TelemetryConfig::new();
    persist_config(&config);
    config
}

/// Persist the telemetry config to disk (best-effort, never fails the caller).
fn persist_config(config: &TelemetryConfig) {
    if let Some(dir) = dirs_path() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            debug!("Failed to create telemetry config directory: {e}");
            return;
        }

        if let Some(path) = config_file_path() {
            match serde_json::to_string_pretty(config) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&path, json) {
                        debug!("Failed to write telemetry config: {e}");
                    } else {
                        debug!("Wrote telemetry config to {}", path.display());
                    }
                }
                Err(e) => {
                    debug!("Failed to serialize telemetry config: {e}");
                }
            }
        }
    }
}

/// Get the cached telemetry config (loaded once from disk per process).
fn cached_config() -> &'static TelemetryConfig {
    CONFIG_CACHE.get_or_init(load_or_create_config)
}

/// Get the persistent anonymous ID for this installation.
///
/// Loaded from `~/.iam-policy-autopilot/telemetry.json` on first call,
/// cached for the lifetime of the process. If the config file doesn't exist,
/// a new UUID is generated and persisted automatically.
pub fn anonymous_id() -> String {
    cached_config().anonymous_id.clone()
}

/// Update the telemetry choice in the persistent config file.
///
/// Writes through to disk immediately. Note: the in-process cache retains
/// the original value (acceptable since the CLI exits shortly after this call).
pub fn set_telemetry_choice(choice: TelemetryChoice) {
    let mut config = cached_config().clone();
    config.telemetry_choice = choice;
    persist_config(&config);
}

/// Get the user's persistent telemetry choice from the config file.
///
/// Uses the process-level cache (loaded once from disk).
pub fn get_telemetry_choice() -> TelemetryChoice {
    cached_config().telemetry_choice
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_new_config_has_valid_uuid() {
        let config = TelemetryConfig::new();
        assert!(uuid::Uuid::parse_str(&config.anonymous_id).is_ok());
        assert_eq!(config.telemetry_choice, TelemetryChoice::NotSet);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = TelemetryConfig::new();
        let json = serde_json::to_string_pretty(&config).expect("should serialize");
        let parsed: TelemetryConfig =
            serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(config.anonymous_id, parsed.anonymous_id);
        assert_eq!(config.telemetry_choice, parsed.telemetry_choice);
    }

    #[test]
    fn test_config_json_format() {
        let config = TelemetryConfig {
            anonymous_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            telemetry_choice: TelemetryChoice::Enabled,
        };
        let json = serde_json::to_string_pretty(&config).expect("should serialize");
        assert!(json.contains("\"anonymousId\""));
        assert!(json.contains("\"telemetryChoice\""));
        assert!(json.contains("\"enabled\""));
        assert!(json.contains("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_config_deserialization_from_new_format() {
        let json = r#"{
            "anonymousId": "test-uuid-123",
            "telemetryChoice": "disabled"
        }"#;
        let config: TelemetryConfig =
            serde_json::from_str(json).expect("should deserialize");
        assert_eq!(config.anonymous_id, "test-uuid-123");
        assert_eq!(config.telemetry_choice, TelemetryChoice::Disabled);
    }

    #[test]
    fn test_config_deserialization_not_set() {
        let json = r#"{
            "anonymousId": "test-uuid-456",
            "telemetryChoice": "notSet"
        }"#;
        let config: TelemetryConfig =
            serde_json::from_str(json).expect("should deserialize");
        assert_eq!(config.anonymous_id, "test-uuid-456");
        assert_eq!(config.telemetry_choice, TelemetryChoice::NotSet);
    }

    #[test]
    fn test_telemetry_choice_default_is_not_set() {
        assert_eq!(TelemetryChoice::default(), TelemetryChoice::NotSet);
    }

    #[test]
    fn test_telemetry_choice_display() {
        assert_eq!(format!("{}", TelemetryChoice::NotSet), "notSet");
        assert_eq!(format!("{}", TelemetryChoice::Enabled), "enabled");
        assert_eq!(format!("{}", TelemetryChoice::Disabled), "disabled");
    }

    #[test]
    #[serial]
    fn test_load_or_create_config_returns_valid() {
        let id = anonymous_id();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }

    #[test]
    #[serial]
    fn test_anonymous_id_is_consistent() {
        // Two calls should return the same ID (cached in process)
        let id1 = anonymous_id();
        let id2 = anonymous_id();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_config_default_telemetry_choice_when_missing() {
        // When telemetryChoice is missing from JSON, should default to NotSet
        let json = r#"{ "anonymousId": "uuid-only" }"#;
        let config: TelemetryConfig =
            serde_json::from_str(json).expect("should deserialize");
        assert_eq!(config.telemetry_choice, TelemetryChoice::NotSet);
    }
}
