//! Telemetry module for anonymous usage metrics collection.
//!
//! This module provides fire-and-forget anonymous telemetry for IAM Policy Autopilot.
//!
//! **Precedence (highest to lowest):**
//! 1. `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable (`0` = off, `1` = on)
//! 2. Persistent config file choice (`~/.iam-policy-autopilot/telemetry.json`)
//! 3. Default: telemetry ON, notice shown
//!
//! The notice is shown when the user has not yet made an explicit choice
//! (neither via env var nor via `iam-policy-autopilot telemetry --enable/--disable`).
//!
//! Telemetry never collects PII, file paths, policy content, AWS account IDs, or credentials.
//! It collects only anonymous parameter-usage data (boolean presence, enum values, service names)
//! and post-execution result data (success/failure, number of policies, services used).
//!
//! A persistent UUID (anonymous_id) is stored in `~/.iam-policy-autopilot/telemetry.json`
//! and included in every event to allow counting unique installations without identifying users.

mod client;
mod config;
mod event;

pub use client::TelemetryClient;
pub use config::{
    anonymous_id, get_telemetry_choice, set_telemetry_choice,
    TelemetryChoice, TelemetryConfig,
};
pub use event::TelemetryEvent;

// Re-export the derive macro so users can write:
//   use iam_policy_autopilot_common::telemetry::TelemetryEvent;  (the struct)
//   use iam_policy_autopilot_common::telemetry::TelemetryEventDerive;  (the macro)
pub use iam_policy_autopilot_telemetry_derive::TelemetryEvent as TelemetryEventDerive;

/// Trait for types that can be converted to a [`TelemetryEvent`].
///
/// This is automatically implemented by `#[derive(TelemetryEvent)]` from
/// `iam-policy-autopilot-telemetry-derive`.
pub trait ToTelemetryEvent {
    /// Convert this value into an optional telemetry event.
    ///
    /// Returns `None` if telemetry should not be emitted for this value
    /// (e.g., `#[telemetry(skip)]` variants).
    fn to_telemetry_event(&self) -> Option<TelemetryEvent>;

    /// Return metadata about what telemetry fields are collected.
    ///
    /// Used to auto-generate the TELEMETRY.md disclosure table.
    fn telemetry_fields() -> Vec<TelemetryFieldInfo>;

    /// Whether the CLI telemetry notice should be suppressed for this variant.
    ///
    /// Returns `true` for variants annotated with `#[telemetry(skip)]` or
    /// `#[telemetry(skip_notice)]`. The CLI uses this to avoid printing the
    /// notice for commands that handle it differently (e.g., `mcp-server`
    /// sends it via MCP notifications) or where it's irrelevant (`telemetry`).
    fn should_skip_notice(&self) -> bool {
        false
    }
}

/// Metadata about a single telemetry field, used for auto-generating documentation.
#[derive(Debug, Clone)]
pub struct TelemetryFieldInfo {
    /// The command name (e.g., "generate-policies")
    pub command: String,
    /// The field/parameter name (e.g., "pretty", "source_files")
    pub field_name: String,
    /// How the field is collected (e.g., "value (bool)", "presence", "list", "skipped")
    pub collection_mode: String,
}

/// Environment variable name that controls telemetry opt-in/opt-out.
pub const TELEMETRY_ENV_VAR: &str = "IAM_POLICY_AUTOPILOT_TELEMETRY";

/// Represents the resolved telemetry state based on environment variable and config file.
///
/// **Precedence (highest to lowest):**
/// 1. Environment variable `IAM_POLICY_AUTOPILOT_TELEMETRY` (`0` = disabled, other = enabled)
/// 2. Config file `telemetryChoice` (`enabled`/`disabled`/`notSet`)
/// 3. Default: telemetry ON, notice shown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryState {
    /// Telemetry is explicitly enabled (via env var or config file).
    /// No notice is shown.
    Enabled,
    /// Telemetry is explicitly disabled (via env var or config file).
    /// No telemetry emitted, no notice shown.
    Disabled,
    /// No explicit choice has been made. Telemetry is ON by default, and a notice should be shown.
    DefaultOn,
}

/// Reads the `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable and the persistent
/// config file, returning the corresponding [`TelemetryState`].
///
/// **Precedence:**
/// 1. Env var `"0"` → `Disabled`
/// 2. Env var set to any other value → `Enabled`
/// 3. Config file `telemetryChoice: "disabled"` → `Disabled`
/// 4. Config file `telemetryChoice: "enabled"` → `Enabled`
/// 5. Config file `telemetryChoice: "notSet"` or missing → `DefaultOn`
#[must_use]
pub fn telemetry_state() -> TelemetryState {
    // 1. Environment variable takes highest precedence
    match std::env::var(TELEMETRY_ENV_VAR) {
        Ok(v) if v == "0" => return TelemetryState::Disabled,
        Ok(_) => return TelemetryState::Enabled,
        Err(_) => {} // Not set — fall through to config file
    }

    // 2. Check persistent config file
    match get_telemetry_choice() {
        TelemetryChoice::Enabled => TelemetryState::Enabled,
        TelemetryChoice::Disabled => TelemetryState::Disabled,
        TelemetryChoice::NotSet => TelemetryState::DefaultOn,
    }
}

/// Returns `true` if telemetry should be emitted (either `DefaultOn` or `Enabled`).
#[must_use]
pub fn is_telemetry_enabled() -> bool {
    telemetry_state() != TelemetryState::Disabled
}

/// The telemetry notice content, shared between CLI and MCP.
///
/// This is the single source of truth for the notice text. The CLI prints it
/// to stderr; the MCP server sends it via `notifications/message`.
pub const TELEMETRY_NOTICE: &str = "\
IAM Policy Autopilot will collect telemetry data on command usage starting at \
version 0.2.0 (unless opted out)

        Overview: We do not collect customer content and we anonymize the
                  telemetry we do collect. See the attached link for more
                  information on what data is collected, why, and how to
                  opt-out. Telemetry will NOT be collected for any version
                  prior to 0.2.0 - regardless of opt-in/out.

        Opt-out:  Run `iam-policy-autopilot telemetry --disable`
                  or set IAM_POLICY_AUTOPILOT_TELEMETRY=0

        Details:  https://github.com/awslabs/iam-policy-autopilot/blob/main/TELEMETRY.md";

/// Returns the telemetry notice if the user has not made an explicit choice.
///
/// The notice is shown when:
/// - The env var is unset, AND
/// - The config file `telemetryChoice` is `notSet`
///
/// Returns `None` if the user has explicitly set the env var or made a config file choice.
/// Used by both the CLI (printed to stderr) and the MCP server (sent via `notifications/message`).
#[must_use]
pub fn telemetry_notice() -> Option<&'static str> {
    if telemetry_state() == TelemetryState::DefaultOn {
        Some(TELEMETRY_NOTICE)
    } else {
        None
    }
}

/// Format the current telemetry status as a human-readable multi-line string.
///
/// Includes the effective state, config file preference, environment variable,
/// and the persistent anonymous ID. Used by the CLI `telemetry --status` command.
#[must_use]
pub fn telemetry_status_string() -> String {
    let state = telemetry_state();
    let config_choice = get_telemetry_choice();
    let env_var = std::env::var(TELEMETRY_ENV_VAR).ok();
    let anonymous_id = anonymous_id();

    format!(
        "Telemetry status:\n\
         \x20 Effective state:  {state:?}\n\
         \x20 Config file:      {config_choice}\n\
         \x20 Env var ({env_name}): {env_value}\n\
         \x20 Anonymous ID:     {anonymous_id}",
        env_name = TELEMETRY_ENV_VAR,
        env_value = env_var.as_deref().unwrap_or("<not set>"),
    )
}

/// Emit a telemetry event in the background.
///
/// Spawns a tokio task that sends the telemetry event and returns the
/// `JoinHandle`. Callers should await this with a timeout before exiting
/// to ensure the request completes. Any failures are silently ignored.
pub fn spawn_telemetry(event: TelemetryEvent) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        TelemetryClient::global().emit(&event).await;
    })
}

/// Finalize and emit a telemetry event, then await completion with a timeout.
///
/// Sets the `success` result on the event, spawns the telemetry send, and awaits
/// it with [`TELEMETRY_TIMEOUT`]. If `event` is `None`, this is a no-op.
/// This is the recommended single-call API for CLI command telemetry emission.
pub async fn finalize_and_emit(event: Option<TelemetryEvent>, success: bool) {
    if let Some(event) = event {
        let event = event.with_result_success(success);
        let handle = spawn_telemetry(event);
        await_telemetry(handle).await;
    }
}


/// Maximum time to wait for telemetry to complete before exiting.
/// This prevents the CLI from hanging if the network is slow.
pub const TELEMETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Await a telemetry task with a timeout.
///
/// Waits up to [`TELEMETRY_TIMEOUT`] for the task to complete. If it times out
/// or errors, the result is silently ignored — telemetry must never delay exit
/// beyond the timeout.
pub async fn await_telemetry(handle: tokio::task::JoinHandle<()>) {
    let _ = tokio::time::timeout(TELEMETRY_TIMEOUT, handle).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // Note: These tests modify environment variables, so they must run serially.
    // The `#[serial]` attribute ensures no parallel execution.

    fn with_env_var<F: FnOnce()>(value: Option<&str>, f: F) {
        // Save original value
        let original = std::env::var(TELEMETRY_ENV_VAR).ok();

        // Set or unset the env var
        match value {
            Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
            None => std::env::remove_var(TELEMETRY_ENV_VAR),
        }

        f();

        // Restore original value
        match original {
            Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
            None => std::env::remove_var(TELEMETRY_ENV_VAR),
        }
    }

    #[test]
    #[serial]
    fn test_telemetry_state_env_zero_returns_disabled() {
        with_env_var(Some("0"), || {
            assert_eq!(telemetry_state(), TelemetryState::Disabled);
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_state_env_one_returns_enabled() {
        with_env_var(Some("1"), || {
            assert_eq!(telemetry_state(), TelemetryState::Enabled);
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_state_env_other_value_returns_enabled() {
        with_env_var(Some("true"), || {
            assert_eq!(telemetry_state(), TelemetryState::Enabled);
        });
    }

    #[test]
    #[serial]
    fn test_is_telemetry_enabled_when_env_enabled() {
        with_env_var(Some("1"), || {
            assert!(is_telemetry_enabled());
        });
    }

    #[test]
    #[serial]
    fn test_is_telemetry_disabled_when_env_zero() {
        with_env_var(Some("0"), || {
            assert!(!is_telemetry_enabled());
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_notice_returns_none_when_env_explicitly_set() {
        with_env_var(Some("1"), || {
            assert!(telemetry_notice().is_none());
        });
        with_env_var(Some("0"), || {
            assert!(telemetry_notice().is_none());
        });
    }

    #[test]
    fn test_telemetry_notice_constant_contains_required_content() {
        assert!(TELEMETRY_NOTICE.contains("IAM Policy Autopilot"));
        assert!(TELEMETRY_NOTICE.contains("telemetry --disable"));
        assert!(TELEMETRY_NOTICE.contains("IAM_POLICY_AUTOPILOT_TELEMETRY=0"));
        assert!(TELEMETRY_NOTICE.contains("TELEMETRY.md"));
    }

    #[test]
    fn test_get_anonymous_id_is_valid_uuid() {
        let anonymous_id = anonymous_id();
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(anonymous_id.len(), 36);
        assert!(uuid::Uuid::parse_str(&anonymous_id).is_ok());
    }

    #[test]
    fn test_get_anonymous_id_is_persistent() {
        // Two calls should return the same ID (persisted to disk)
        let id1 = anonymous_id();
        let id2 = anonymous_id();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_telemetry_choice_enum() {
        assert_eq!(TelemetryChoice::default(), TelemetryChoice::NotSet);
        assert_ne!(TelemetryChoice::Enabled, TelemetryChoice::Disabled);
    }
}
