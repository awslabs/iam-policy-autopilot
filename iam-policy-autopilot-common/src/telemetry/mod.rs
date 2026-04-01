//! Telemetry module for anonymous usage metrics collection.
//!
//! This module provides fire-and-forget anonymous telemetry for IAM Policy Autopilot.
//!
//! **Precedence (highest to lowest):**
//! 1. `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true` environment variable → disabled
//! 2. Persistent config file choice (`~/.iam-policy-autopilot/config.json`)
//! 3. Default: telemetry ON, notice shown
//!
//! The notice is shown when the user has not yet made an explicit choice
//! (neither via env var nor via `iam-policy-autopilot telemetry --enable/--disable`).
//!
//! Telemetry never collects PII, file paths, policy content, AWS account IDs, or credentials.
//! It collects only anonymous parameter-usage data (boolean presence, enum values, service names)
//! and post-execution result data (success/failure, number of policies, services used).
//!
//! A persistent UUID (installation_id) is stored in `~/.iam-policy-autopilot/config.json`
//! and included in every event to allow counting unique installations without identifying users.

mod client;
mod config;
mod event;
pub mod span;

pub use client::TelemetryClient;
pub use config::{
    get_telemetry_choice, installation_id, set_telemetry_choice, TelemetryChoice, TelemetryConfig,
};
pub use event::TelemetryEvent;

// Re-export the derive macro so users can write:
//   use iam_policy_autopilot_common::telemetry::TelemetryEvent;  (the struct)
//   use iam_policy_autopilot_common::telemetry::TelemetryEventDerive;  (the macro)
pub use iam_policy_autopilot_proc_macros::TelemetryEvent as TelemetryEventDerive;

/// Trait for types that can be converted to a [`TelemetryEvent`].
///
/// This is automatically implemented by `#[derive(TelemetryEvent)]` from
/// `iam-policy-autopilot-proc-macros`.
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

/// Environment variable name that disables telemetry when set to `"true"`.
///
/// Any other value or unset → fall through to config file.
pub const TELEMETRY_ENV_VAR: &str = "DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY";

/// Represents the resolved telemetry state based on environment variable and config file.
///
/// **Precedence (highest to lowest):**
/// 1. Environment variable `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true` → disabled
/// 2. Config file `telemetryChoice` (`enabled`/`disabled`/`notSet`)
/// 3. Default: telemetry ON, notice shown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryState {
    /// Telemetry is explicitly enabled (via config file).
    /// No notice is shown.
    Enabled,
    /// Telemetry is explicitly disabled (via env var or config file).
    /// No telemetry emitted, no notice shown.
    Disabled,
    /// No explicit choice has been made. Telemetry is ON by default, and a notice should be shown.
    DefaultOn,
}

/// Reads the `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY` environment variable and the persistent
/// config file, returning the corresponding [`TelemetryState`].
///
/// **Precedence:**
/// 1. Env var `"true"` → `Disabled`
/// 2. Any other env var value or unset → fall through to config file
/// 3. Config file `telemetryChoice: "disabled"` → `Disabled`
/// 4. Config file `telemetryChoice: "enabled"` → `Enabled`
/// 5. Config file `telemetryChoice: "notSet"` or missing → `DefaultOn`
#[must_use]
pub fn telemetry_state() -> TelemetryState {
    // 1. Environment variable: only "true" disables; anything else is ignored
    if std::env::var(TELEMETRY_ENV_VAR).ok().as_deref() == Some("true") {
        return TelemetryState::Disabled;
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
                  or set DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true

        Details:  https://github.com/awslabs/iam-policy-autopilot/blob/main/TELEMETRY.md";

/// Returns the telemetry notice if the user has not made an explicit choice.
///
/// The notice is shown when:
/// - The env var is unset or not `"true"`, AND
/// - The config file `telemetryChoice` is `notSet`
///
/// Returns `None` if the user has explicitly disabled via env var or made a config file choice.
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
/// and the persistent installation ID. Used by the CLI `telemetry --status` command.
#[must_use]
pub fn telemetry_status_string() -> String {
    let state = telemetry_state();
    let config_choice = get_telemetry_choice();
    let env_var = std::env::var(TELEMETRY_ENV_VAR).ok();
    let installation_id = installation_id();

    format!(
        "Telemetry status:\n\
         \x20 Effective state:  {state:?}\n\
         \x20 Config file:      {config_choice}\n\
         \x20 Env var ({env_name}): {env_value}\n\
         \x20 Installation ID:  {installation_id}",
        env_name = TELEMETRY_ENV_VAR,
        env_value = env_var.as_deref().unwrap_or("<not set>"),
    )
}

/// Emit a telemetry event in the background (fire-and-forget).
///
/// Spawns a tokio task that sends the telemetry event. The task runs
/// independently — callers don't need to await the result.
/// Any failures are silently ignored.
pub fn spawn_telemetry(event: TelemetryEvent) {
    drop(tokio::spawn(async move {
        TelemetryClient::global().emit(&event).await;
    }));
}

/// Finalize and emit a telemetry event, then await completion with a timeout.
///
/// Sets the `success` result on the event, spawns the telemetry send, and awaits
/// it with [`TELEMETRY_TIMEOUT`]. If `event` is `None`, this is a no-op.
/// This is the recommended single-call API for CLI command telemetry emission.
pub async fn finalize_and_emit(event: Option<TelemetryEvent>, success: bool) {
    if let Some(event) = event {
        let event = event.with_result_success(success);
        let handle = tokio::spawn(async move {
            TelemetryClient::global().emit(&event).await;
        });
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
    use rstest::rstest;
    use serial_test::serial;

    /// Run a closure with a specific env var value, restoring afterwards.
    fn with_env_var<F: FnOnce()>(value: &str, f: F) {
        let original = std::env::var(TELEMETRY_ENV_VAR).ok();
        std::env::set_var(TELEMETRY_ENV_VAR, value);
        f();
        match original {
            Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
            None => std::env::remove_var(TELEMETRY_ENV_VAR),
        }
    }

    /// Run a closure with the env var removed, restoring afterwards.
    fn without_env_var<F: FnOnce()>(f: F) {
        let original = std::env::var(TELEMETRY_ENV_VAR).ok();
        std::env::remove_var(TELEMETRY_ENV_VAR);
        f();
        match original {
            Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
            None => std::env::remove_var(TELEMETRY_ENV_VAR),
        }
    }

    // =========================================================================
    // Env-var–driven behavior — "true" disables, anything else falls through
    // to config file.
    // =========================================================================

    #[test]
    #[serial]
    fn test_telemetry_env_true_disables() {
        with_env_var("true", || {
            assert_eq!(telemetry_state(), TelemetryState::Disabled);
            assert!(!is_telemetry_enabled());
            // Disabled = no notice
            assert!(telemetry_notice().is_none());
        });
    }

    /// Non-"true" env var values (e.g. "false", "0", "1") are ignored and
    /// fall through to the config file. With config at NotSet → DefaultOn.
    #[rstest]
    #[case::false_str("false")]
    #[case::zero("0")]
    #[case::one("1")]
    #[case::random("foobar")]
    #[test]
    #[serial]
    fn test_telemetry_env_non_true_falls_through(#[case] env_val: &str) {
        with_env_var(env_val, || {
            // With default NotSet config, non-"true" env values fall through to DefaultOn
            let state = telemetry_state();
            assert_ne!(
                state,
                TelemetryState::Disabled,
                "env var '{env_val}' should NOT disable telemetry"
            );
        });
    }

    // =========================================================================
    // Notice content — parameterized
    // =========================================================================

    #[rstest]
    #[case("IAM Policy Autopilot")]
    #[case("telemetry --disable")]
    #[case("DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true")]
    #[case("TELEMETRY.md")]
    fn test_notice_contains(#[case] needle: &str) {
        assert!(TELEMETRY_NOTICE.contains(needle), "missing: {needle}");
    }

    // =========================================================================
    // Installation ID + TelemetryChoice
    // =========================================================================

    #[test]
    fn test_installation_id_is_valid_persistent_uuid() {
        let id1 = installation_id();
        let id2 = installation_id();
        assert_eq!(id1.len(), 36);
        assert!(uuid::Uuid::parse_str(&id1).is_ok());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_telemetry_choice_defaults() {
        assert_eq!(TelemetryChoice::default(), TelemetryChoice::NotSet);
        assert_ne!(TelemetryChoice::Enabled, TelemetryChoice::Disabled);
    }

    // =========================================================================
    // Env var unset — falls through to config file (DefaultOn with NotSet config)
    // =========================================================================

    #[test]
    #[serial]
    fn test_telemetry_env_unset_falls_through_to_config() {
        without_env_var(|| {
            // With default NotSet config, should be DefaultOn
            let state = telemetry_state();
            // State depends on config file, but should not be Disabled
            // (unless config file says disabled)
            assert!(
                state == TelemetryState::DefaultOn || state == TelemetryState::Enabled,
                "With env unset and default config, state should be DefaultOn or Enabled, got {state:?}"
            );
        });
    }
}
