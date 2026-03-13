# Engineering Design: Telemetry Framework Implementation Plan

> Reference: [Quip Design Doc](https://quip-amazon.com/Wx29ACgZWf4S/Engineering-Design-Telemetry-Framework-for-IAM-Policy-Autopilot)

## Summary

This document describes the implementation plan for adding anonymous usage telemetry to IAM Policy Autopilot. The telemetry framework collects anonymous, non-PII usage metrics by sending custom HTTP headers on a lightweight GET request to the existing service reference endpoint (`https://servicereference.us-east-1.amazonaws.com`).

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  iam-policy-autopilot-policy-generation                         │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ src/telemetry/                                             │ │
│  │   mod.rs          — TelemetryConfig, is_enabled(), notice  │ │
│  │   event.rs        — TelemetryEvent, TelemetryParam         │ │
│  │   client.rs       — TelemetryClient (reqwest-based sender) │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
         ▲                                    ▲
         │ uses                               │ uses
┌────────┴──────────┐             ┌───────────┴──────────────┐
│ iam-policy-       │             │ iam-policy-autopilot-    │
│ autopilot-cli     │             │ mcp-server               │
│                   │             │                          │
│ main.rs: builds   │             │ mcp.rs: builds           │
│ TelemetryEvent    │             │ TelemetryEvent per tool  │
│ per command,      │             │ invocation, spawns       │
│ spawns emit()     │             │ emit()                   │
└───────────────────┘             └──────────────────────────┘
```

### Why in `iam-policy-autopilot-policy-generation`?

- Both `iam-policy-autopilot-cli` and `iam-policy-autopilot-mcp-server` already depend on this crate
- It already has the `reqwest` dependency used by `RemoteServiceReferenceLoader`
- It already communicates with `servicereference.us-east-1.amazonaws.com`
- No new cross-crate dependencies needed

## Module Design

### `telemetry/mod.rs` — Configuration & Control

```rust
/// Environment variable name for telemetry opt-in/opt-out
const TELEMETRY_ENV_VAR: &str = "IAM_POLICY_AUTOPILOT_TELEMETRY";

/// Check telemetry state: Enabled (default/explicit), Disabled, or Unset (default ON + show notice)
pub enum TelemetryState { Enabled, Disabled, DefaultOn }

pub fn telemetry_state() -> TelemetryState;
pub fn is_telemetry_enabled() -> bool;
pub fn telemetry_notice_cli() -> Option<&'static str>;
```

### `telemetry/event.rs` — Telemetry Data Model

```rust
/// Represents a single telemetry event emitted per CLI command or MCP tool invocation
pub struct TelemetryEvent {
    pub command: String,              // e.g. "generate-policies", "mcp:generate_application_policies"
    pub params: Vec<TelemetryParam>,  // list of recorded parameters
    pub version: String,              // crate version
}

pub struct TelemetryParam {
    pub name: String,
    pub value: TelemetryValue,
}

pub enum TelemetryValue {
    Bool(bool),                  // parameter presence (true/false)
    Str(String),                 // enum/fixed value (e.g. "python", "s3")
    Count(usize),                // count of items (e.g. service_hints count)
}
```

### `telemetry/client.rs` — HTTP Sender

```rust
/// Fire-and-forget telemetry sender
pub struct TelemetryClient { ... }

impl TelemetryClient {
    pub fn new() -> Self;
    /// Encode event as custom HTTP headers and send GET to service reference endpoint
    pub async fn emit(&self, event: &TelemetryEvent);
}
```

The `emit()` method:
1. Builds a `reqwest::Client` with the same User-Agent as `RemoteServiceReferenceLoader`
2. Encodes the event as custom `X-Ipa-*` headers
3. Sends a GET request to `https://servicereference.us-east-1.amazonaws.com`
4. Silently ignores any errors (fire-and-forget)

## Header Encoding Scheme

Each telemetry event is encoded into HTTP headers:

| Header | Value | Example |
|--------|-------|---------|
| `X-Ipa-Command` | command name | `generate-policies` |
| `X-Ipa-Version` | tool version | `0.1.4` |
| `X-Ipa-P-{name}` | parameter value | `X-Ipa-P-Region: true` (presence) |

## Integration Points

### CLI (`iam-policy-autopilot-cli/src/main.rs`)

In the `main()` function, after parsing CLI args but before executing:
1. Check `telemetry_state()`
2. If `DefaultOn`, print notice to stderr
3. If enabled, build `TelemetryEvent` from parsed args, spawn `emit()`

### MCP Server (`iam-policy-autopilot-mcp-server/src/mcp.rs`)

In each tool handler (e.g., `generate_application_policies`):
1. Check `telemetry_state()`
2. If `DefaultOn` on startup, send MCP `notifications/message`
3. If enabled, build `TelemetryEvent` from tool params, spawn `emit()`

## Parameter Recording Rules (from design doc)

| Parameter | What We Record |
|-----------|---------------|
| `source_files` | Whether provided (boolean), not the paths |
| `region` | Whether provided (boolean), not the value |
| `account` | Whether provided (boolean), not the value |
| `service_hints` | Values (e.g. "s3", "ec2") — these are AWS service names, not PII |
| `pretty` | Value (true/false) |
| `language` | Value if provided (e.g. "python", "go") |
| `full_output` | Value (true/false) |
| `individual_policies` | Value (true/false) |
| `upload_policies` | Whether provided (boolean), not the prefix value |
| `minimal_policy_size` | Value (true/false) |
| `disable_cache` | Value (true/false) |
| `explain` | Value (e.g. "s3:*") |
| `debug` | Not collected |
| `transport` | Value (stdio/http) |
| `port` | Whether non-default used (boolean) |
| `source` (fix-access-denied) | Whether provided via arg vs stdin |
| `yes` | Value (true/false) |

## File Changes Summary

### New Files
- `iam-policy-autopilot-policy-generation/src/telemetry/mod.rs`
- `iam-policy-autopilot-policy-generation/src/telemetry/event.rs`
- `iam-policy-autopilot-policy-generation/src/telemetry/client.rs`
- `TELEMETRY.md`

### Modified Files
- `iam-policy-autopilot-policy-generation/src/lib.rs` — add `pub mod telemetry;`
- `iam-policy-autopilot-cli/src/main.rs` — integrate telemetry emission
- `iam-policy-autopilot-mcp-server/src/mcp.rs` — integrate telemetry emission
- `iam-policy-autopilot-mcp-server/src/lib.rs` — startup notice for MCP

## Testing Strategy

1. **Unit tests** in `telemetry/mod.rs`:
   - `test_telemetry_state_*` — env var parsing
   - `test_is_telemetry_enabled_*` — enabled/disabled logic

2. **Unit tests** in `telemetry/event.rs`:
   - `test_event_header_encoding` — correct header generation
   - `test_param_recording` — correct values

3. **Unit tests** in `telemetry/client.rs`:
   - `test_emit_fire_and_forget` — errors don't propagate
   - Mock server tests to verify correct headers sent

4. **CLI integration tests** in `iam-policy-autopilot-cli/tests/`:
   - `test_telemetry_notice_shown` — notice appears when env unset
   - `test_telemetry_notice_hidden` — notice hidden when env set
   - `test_telemetry_disabled` — no request when disabled

## Safety Guarantees

- **Fire-and-forget**: All telemetry failures are silently caught; they never affect tool output
- **No PII**: Only boolean presence, enum values, and AWS service names are recorded
- **No filesystem side-effects**: No config files, no marker files
- **No new dependencies**: Reuses existing `reqwest` in the workspace
- **Thread-safe**: Uses `tokio::spawn` for async emission
