//! Telemetry module for anonymous usage metrics collection.
//!
//! This module provides fire-and-forget anonymous telemetry for IAM Policy Autopilot.
//! Telemetry is controlled by the `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable:
//!
//! - Unset: telemetry ON (default), telemetry notice shown
//! - `0`: telemetry disabled, no notice
//! - `1` (or any other value): telemetry enabled, no notice
//!
//! Telemetry never collects PII, file paths, policy content, AWS account IDs, or credentials.
//! It collects only anonymous parameter-usage data (boolean presence, enum values, service names).

mod client;
mod event;

pub use client::TelemetryClient;
pub use event::{TelemetryEvent, TelemetryParam, TelemetryValue};

/// Environment variable name that controls telemetry opt-in/opt-out.
pub const TELEMETRY_ENV_VAR: &str = "IAM_POLICY_AUTOPILOT_TELEMETRY";

/// Represents the resolved telemetry state based on the environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryState {
    /// Telemetry is explicitly enabled (`IAM_POLICY_AUTOPILOT_TELEMETRY=1` or any non-"0" value).
    /// No notice is shown.
    Enabled,
    /// Telemetry is explicitly disabled (`IAM_POLICY_AUTOPILOT_TELEMETRY=0`).
    /// No telemetry emitted, no notice shown.
    Disabled,
    /// Environment variable is unset. Telemetry is ON by default, and a notice should be shown.
    DefaultOn,
}

/// Reads the `IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable and returns
/// the corresponding [`TelemetryState`].
///
/// - Unset → `DefaultOn` (telemetry ON, show notice)
/// - `"0"` → `Disabled` (no telemetry, no notice)
/// - Any other value → `Enabled` (telemetry ON, no notice)
#[must_use]
pub fn telemetry_state() -> TelemetryState {
    match std::env::var(TELEMETRY_ENV_VAR) {
        Err(_) => TelemetryState::DefaultOn,
        Ok(v) if v == "0" => TelemetryState::Disabled,
        Ok(_) => TelemetryState::Enabled,
    }
}

/// Returns `true` if telemetry should be emitted (either `DefaultOn` or `Enabled`).
#[must_use]
pub fn is_telemetry_enabled() -> bool {
    telemetry_state() != TelemetryState::Disabled
}

/// Returns the CLI telemetry notice message if the environment variable is unset.
///
/// When the env var is unset (`DefaultOn`), returns `Some` with a notice string
/// suitable for printing to stderr. Returns `None` if the env var is explicitly set
/// to any value (including "0" or "1").
#[must_use]
pub fn telemetry_notice_cli() -> Option<&'static str> {
    if telemetry_state() == TelemetryState::DefaultOn {
        Some("[telemetry] Anonymous usage metrics enabled. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 to disable. See TELEMETRY.md")
    } else {
        None
    }
}

/// Returns the MCP server telemetry notice message if the environment variable is unset.
///
/// When the env var is unset (`DefaultOn`), returns `Some` with a notice string
/// suitable for sending as an MCP `notifications/message`. Returns `None` otherwise.
#[must_use]
pub fn telemetry_notice_mcp() -> Option<&'static str> {
    if telemetry_state() == TelemetryState::DefaultOn {
        Some("Anonymous usage metrics are enabled. Set IAM_POLICY_AUTOPILOT_TELEMETRY=0 in your MCP server env config to disable. See https://github.com/awslabs/iam-policy-autopilot/blob/main/TELEMETRY.md")
    } else {
        None
    }
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
    fn test_telemetry_state_unset_returns_default_on() {
        with_env_var(None, || {
            assert_eq!(telemetry_state(), TelemetryState::DefaultOn);
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_state_zero_returns_disabled() {
        with_env_var(Some("0"), || {
            assert_eq!(telemetry_state(), TelemetryState::Disabled);
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_state_one_returns_enabled() {
        with_env_var(Some("1"), || {
            assert_eq!(telemetry_state(), TelemetryState::Enabled);
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_state_other_value_returns_enabled() {
        with_env_var(Some("true"), || {
            assert_eq!(telemetry_state(), TelemetryState::Enabled);
        });
    }

    #[test]
    #[serial]
    fn test_is_telemetry_enabled_when_unset() {
        with_env_var(None, || {
            assert!(is_telemetry_enabled());
        });
    }

    #[test]
    #[serial]
    fn test_is_telemetry_enabled_when_enabled() {
        with_env_var(Some("1"), || {
            assert!(is_telemetry_enabled());
        });
    }

    #[test]
    #[serial]
    fn test_is_telemetry_disabled_when_zero() {
        with_env_var(Some("0"), || {
            assert!(!is_telemetry_enabled());
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_notice_cli_when_unset() {
        with_env_var(None, || {
            let notice = telemetry_notice_cli();
            assert!(notice.is_some());
            let msg = notice.expect("notice should be Some");
            assert!(msg.contains("IAM_POLICY_AUTOPILOT_TELEMETRY=0"));
            assert!(msg.contains("TELEMETRY.md"));
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_notice_cli_when_explicitly_set() {
        with_env_var(Some("1"), || {
            assert!(telemetry_notice_cli().is_none());
        });
        with_env_var(Some("0"), || {
            assert!(telemetry_notice_cli().is_none());
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_notice_mcp_when_unset() {
        with_env_var(None, || {
            let notice = telemetry_notice_mcp();
            assert!(notice.is_some());
            let msg = notice.expect("notice should be Some");
            assert!(msg.contains("IAM_POLICY_AUTOPILOT_TELEMETRY=0"));
            assert!(msg.contains("TELEMETRY.md"));
        });
    }

    #[test]
    #[serial]
    fn test_telemetry_notice_mcp_when_explicitly_set() {
        with_env_var(Some("1"), || {
            assert!(telemetry_notice_mcp().is_none());
        });
        with_env_var(Some("0"), || {
            assert!(telemetry_notice_mcp().is_none());
        });
    }
}
