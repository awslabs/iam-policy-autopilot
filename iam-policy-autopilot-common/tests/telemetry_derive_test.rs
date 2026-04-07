//! Integration tests for `#[derive(TelemetryEvent)]` proc macro.
//!
//! Uses `rstest` parameterized tests and an RAII [`EnvGuard`] to reduce
//! repetition while maintaining full coverage of field recording, skip behavior,
//! notice suppression, and `telemetry_fields()` metadata.

use iam_policy_autopilot_common::telemetry::{
    TelemetryEventDerive, TelemetryFieldInfo, ToTelemetryEvent, TELEMETRY_ENV_VAR,
};
use rstest::rstest;
use serial_test::serial;

// =============================================================================
// RAII env-var guard
// =============================================================================

/// RAII guard that restores the telemetry env var on drop.
struct EnvGuard {
    original: Option<String>,
}

impl EnvGuard {
    /// Enable telemetry by removing the env var (telemetry is ON by default when unset).
    fn enabled() -> Self {
        let original = std::env::var(TELEMETRY_ENV_VAR).ok();
        std::env::remove_var(TELEMETRY_ENV_VAR);
        Self { original }
    }

    /// Disable telemetry by setting `DISABLE_IAM_POLICY_AUTOPILOT_TELEMETRY=true`.
    fn disabled() -> Self {
        let original = std::env::var(TELEMETRY_ENV_VAR).ok();
        std::env::set_var(TELEMETRY_ENV_VAR, "true");
        Self { original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
            None => std::env::remove_var(TELEMETRY_ENV_VAR),
        }
    }
}

// =============================================================================
// Test fixtures (types)
// =============================================================================

#[allow(dead_code)]
#[derive(TelemetryEventDerive)]
enum TestCommands {
    #[telemetry(command = "generate-policies")]
    GeneratePolicies {
        #[telemetry(presence)]
        source_files: Vec<String>,
        #[telemetry(value)]
        pretty: bool,
        #[telemetry(value)]
        language: String,
        #[telemetry(skip)]
        debug: bool,
        #[telemetry(value, if_present)]
        region: Option<String>,
        #[telemetry(list)]
        service_hints: Option<Vec<String>>,
        #[telemetry(presence, default = "us-east-1")]
        default_region: String,
    },

    #[telemetry(skip)]
    Version { verbose: bool },

    #[telemetry(skip_notice)]
    McpServer {
        #[telemetry(value)]
        transport: String,
    },

    // Bare variant — all fields unannotated → all skipped
    #[telemetry(command = "status")]
    Status { verbose: bool },

    // Unit variant
    #[telemetry(command = "help")]
    Help,

    // Tuple variant (skip)
    #[telemetry(skip)]
    Internal(String),
}

#[allow(dead_code)]
#[derive(TelemetryEventDerive)]
#[telemetry(command = "mcp-tool-generate")]
struct TestMcpInput {
    #[telemetry(presence)]
    source_files: Vec<String>,
    #[telemetry(value)]
    pretty: bool,
    #[telemetry(value, if_present)]
    region: Option<String>,
    #[telemetry(list)]
    service_hints: Option<Vec<String>>,
    #[telemetry(presence)]
    account: Option<String>,
    // No telemetry attribute — should be skipped
    internal_id: String,
}

#[allow(dead_code)]
#[derive(TelemetryEventDerive)]
#[telemetry(command = "mcp-tool-simple", skip_notice)]
struct TestSkipNoticeStruct {
    #[telemetry(value)]
    name: String,
}

#[allow(dead_code)]
#[derive(TelemetryEventDerive)]
struct AutoNamedStruct {
    #[telemetry(value)]
    flag: bool,
}

// =============================================================================
// Enum: to_telemetry_event() — populated fields
// =============================================================================

#[test]
#[serial]
fn enum_generate_policies_records_all_field_modes() {
    let _guard = EnvGuard::enabled();

    let cmd = TestCommands::GeneratePolicies {
        source_files: vec!["file1.py".into(), "file2.py".into()],
        pretty: true,
        language: "python".into(),
        debug: true,
        region: Some("us-west-2".into()),
        service_hints: Some(vec!["s3".into(), "ec2".into()]),
        default_region: "us-east-1".into(),
    };

    let event = cmd.to_telemetry_event().expect("should produce event");
    assert_eq!(event.command, "generate-policies");
    let params = event.params.expect("should have params");

    // #[telemetry(presence)] on non-empty Vec
    assert_eq!(params.get("source_files"), Some(&serde_json::json!(true)));
    // #[telemetry(value)] on bool
    assert_eq!(params.get("pretty"), Some(&serde_json::json!(true)));
    // #[telemetry(value)] on String
    assert_eq!(params.get("language"), Some(&serde_json::json!("python")));
    // #[telemetry(skip)] — absent
    assert!(
        params.get("debug").is_none(),
        "skipped field should not appear"
    );
    // #[telemetry(value, if_present)] on Some
    assert_eq!(params.get("region"), Some(&serde_json::json!("us-west-2")));
    // #[telemetry(list)] on non-empty vec
    assert_eq!(
        params.get("service_hints"),
        Some(&serde_json::json!(["s3", "ec2"]))
    );
    // #[telemetry(presence, default = "us-east-1")] — value == default → false
    assert_eq!(
        params.get("default_region"),
        Some(&serde_json::json!(false))
    );
}

#[test]
#[serial]
fn enum_generate_policies_empty_and_none_values() {
    let _guard = EnvGuard::enabled();

    let cmd = TestCommands::GeneratePolicies {
        source_files: vec![],
        pretty: false,
        language: "go".into(),
        debug: false,
        region: None,
        service_hints: None,
        default_region: "eu-west-1".into(),
    };

    let event = cmd.to_telemetry_event().expect("should produce event");
    let params = event.params.expect("should have params");

    assert_eq!(
        params.get("source_files"),
        Some(&serde_json::json!(false)),
        "empty Vec → presence false"
    );
    assert!(
        params.get("region").is_none(),
        "None → omitted by value_if_present"
    );
    assert!(
        params.get("service_hints").is_none(),
        "None → omitted by list"
    );
    assert_eq!(
        params.get("default_region"),
        Some(&serde_json::json!(true)),
        "non-default → true"
    );
}

#[test]
#[serial]
fn enum_list_with_empty_vec_omits_field() {
    let _guard = EnvGuard::enabled();

    let cmd = TestCommands::GeneratePolicies {
        source_files: vec![],
        pretty: false,
        language: "go".into(),
        debug: false,
        region: None,
        service_hints: Some(vec![]), // Some but empty
        default_region: "us-east-1".into(),
    };

    let event = cmd.to_telemetry_event().expect("should produce event");
    let params = event.params.expect("should have params");
    assert!(
        params.get("service_hints").is_none(),
        "Some(empty vec) → omitted"
    );
}

// =============================================================================
// Enum: skip variants return None (parameterized)
// =============================================================================

#[rstest]
#[case::skip_named(TestCommands::Version { verbose: true })]
#[case::skip_tuple(TestCommands::Internal("secret".into()))]
#[test]
#[serial]
fn enum_skip_variant_returns_none(#[case] cmd: TestCommands) {
    let _guard = EnvGuard::enabled();
    assert!(
        cmd.to_telemetry_event().is_none(),
        "skipped variant should return None"
    );
}

// =============================================================================
// Enum: minimal-event variants
// =============================================================================

#[test]
#[serial]
fn enum_unit_variant_produces_event_with_command_only() {
    let _guard = EnvGuard::enabled();
    let event = TestCommands::Help
        .to_telemetry_event()
        .expect("should produce event");
    assert_eq!(event.command, "help");
    assert!(event.params.is_none(), "unit variant should have no params");
}

#[test]
#[serial]
fn enum_variant_with_unannotated_fields_produces_event_no_params() {
    let _guard = EnvGuard::enabled();
    let event = TestCommands::Status { verbose: true }
        .to_telemetry_event()
        .expect("should produce event");
    assert_eq!(event.command, "status");
    assert!(
        event.params.is_none(),
        "variant with all unannotated fields should have no params"
    );
}

#[test]
#[serial]
fn enum_skip_notice_variant_records_event_but_suppresses_notice() {
    let _guard = EnvGuard::enabled();
    let cmd = TestCommands::McpServer {
        transport: "stdio".into(),
    };
    let event = cmd
        .to_telemetry_event()
        .expect("skip_notice should still produce event");
    assert_eq!(event.command, "mcpserver");
    let params = event.params.expect("should have params");
    assert_eq!(params.get("transport"), Some(&serde_json::json!("stdio")));
}

// =============================================================================
// Telemetry disabled → None (enum + struct)
// =============================================================================

#[test]
#[serial]
fn enum_returns_none_when_telemetry_disabled() {
    let _guard = EnvGuard::disabled();
    let cmd = TestCommands::GeneratePolicies {
        source_files: vec!["file.py".into()],
        pretty: true,
        language: "python".into(),
        debug: false,
        region: None,
        service_hints: None,
        default_region: "us-east-1".into(),
    };
    assert!(
        cmd.to_telemetry_event().is_none(),
        "should return None when telemetry is disabled"
    );
}

#[test]
#[serial]
fn struct_returns_none_when_telemetry_disabled() {
    let _guard = EnvGuard::disabled();
    let input = TestMcpInput {
        source_files: vec!["file.py".into()],
        pretty: true,
        region: None,
        service_hints: None,
        account: None,
        internal_id: "id".into(),
    };
    assert!(
        input.to_telemetry_event().is_none(),
        "should return None when telemetry is disabled"
    );
}

// =============================================================================
// should_skip_notice() — parameterized over all enum variants
// =============================================================================

#[rstest]
#[case::skip_variant(TestCommands::Version { verbose: true }, true)]
#[case::skip_notice_variant(TestCommands::McpServer { transport: "stdio".into() }, true)]
#[case::skip_tuple_variant(TestCommands::Internal("data".into()), true)]
#[case::normal_variant(
    TestCommands::GeneratePolicies {
        source_files: vec![], pretty: false, language: "python".into(),
        debug: false, region: None, service_hints: None,
        default_region: "us-east-1".into(),
    },
    false
)]
#[case::unit_variant(TestCommands::Help, false)]
fn enum_should_skip_notice(#[case] cmd: TestCommands, #[case] expected: bool) {
    assert_eq!(
        cmd.should_skip_notice(),
        expected,
        "should_skip_notice mismatch"
    );
}

#[test]
fn struct_should_not_skip_notice_by_default() {
    let input = TestMcpInput {
        source_files: vec![],
        pretty: false,
        region: None,
        service_hints: None,
        account: None,
        internal_id: "id".into(),
    };
    assert!(!input.should_skip_notice());
}

#[test]
fn struct_should_skip_notice_when_annotated() {
    let input = TestSkipNoticeStruct {
        name: "test".into(),
    };
    assert!(input.should_skip_notice());
}

// =============================================================================
// telemetry_fields() — enum
// =============================================================================

#[test]
fn enum_telemetry_fields_includes_all_non_skip_variants() {
    let fields = TestCommands::telemetry_fields();

    let generate_fields: Vec<&TelemetryFieldInfo> = fields
        .iter()
        .filter(|f| f.command == "generate-policies")
        .collect();

    assert_eq!(
        generate_fields.len(),
        7,
        "generate-policies should report all 7 fields in metadata"
    );

    // Spot-check every collection mode via table-driven assertions
    let expected_modes: &[(&str, &str)] = &[
        ("debug", "not collected"),
        ("source_files", "presence (boolean)"),
        ("pretty", "actual value (boolean)"),
        ("region", "value if provided, omitted otherwise"),
        (
            "service_hints",
            "list of values if non-empty, omitted otherwise",
        ),
        ("default_region", "whether non-default (boolean)"),
    ];

    for (field_name, expected_mode) in expected_modes {
        let field = generate_fields
            .iter()
            .find(|f| f.field_name == *field_name)
            .unwrap_or_else(|| panic!("{field_name} should be in metadata"));
        assert_eq!(
            field.collection_mode, *expected_mode,
            "wrong mode for {field_name}"
        );
    }
}

#[test]
fn enum_telemetry_fields_omits_container_skipped_variants() {
    let fields = TestCommands::telemetry_fields();
    let version_fields: Vec<&TelemetryFieldInfo> =
        fields.iter().filter(|f| f.command == "version").collect();
    assert!(
        version_fields.is_empty(),
        "skipped variant fields should not appear in telemetry_fields()"
    );
}

// =============================================================================
// Struct: to_telemetry_event()
// =============================================================================

#[test]
#[serial]
fn struct_records_all_field_modes() {
    let _guard = EnvGuard::enabled();

    let input = TestMcpInput {
        source_files: vec!["app.py".into()],
        pretty: true,
        region: Some("us-west-2".into()),
        service_hints: Some(vec!["s3".into()]),
        account: Some("123456789".into()),
        internal_id: "secret-id".into(),
    };

    let event = input.to_telemetry_event().expect("should produce event");
    assert_eq!(event.command, "mcp-tool-generate");
    let params = event.params.expect("should have params");

    assert_eq!(params.get("source_files"), Some(&serde_json::json!(true)));
    assert_eq!(params.get("pretty"), Some(&serde_json::json!(true)));
    assert_eq!(params.get("region"), Some(&serde_json::json!("us-west-2")));
    assert_eq!(params.get("account"), Some(&serde_json::json!(true)));
    assert_eq!(
        params.get("service_hints"),
        Some(&serde_json::json!(["s3"]))
    );
    assert!(
        params.get("internal_id").is_none(),
        "unannotated field should not appear in params"
    );
}

#[test]
#[serial]
fn struct_none_and_empty_values() {
    let _guard = EnvGuard::enabled();

    let input = TestMcpInput {
        source_files: vec![],
        pretty: false,
        region: None,
        service_hints: None,
        account: None,
        internal_id: "id".into(),
    };

    let event = input.to_telemetry_event().expect("should produce event");
    let params = event.params.expect("should have params");

    assert_eq!(
        params.get("source_files"),
        Some(&serde_json::json!(false)),
        "presence on empty Vec"
    );
    assert!(params.get("region").is_none(), "value_if_present with None");
    assert!(params.get("service_hints").is_none(), "list with None");
    assert_eq!(
        params.get("account"),
        Some(&serde_json::json!(false)),
        "presence on None"
    );
}

// =============================================================================
// Struct: should_skip_notice() + telemetry_fields()
// =============================================================================

#[test]
fn struct_telemetry_fields_reports_all_fields() {
    let fields = TestMcpInput::telemetry_fields();

    assert_eq!(
        fields.len(),
        6,
        "should report all 6 fields including unannotated ones"
    );

    for field in &fields {
        assert_eq!(field.command, "mcp-tool-generate");
    }

    let internal = fields
        .iter()
        .find(|f| f.field_name == "internal_id")
        .expect("internal_id should be in metadata");
    assert_eq!(
        internal.collection_mode, "not collected",
        "unannotated field should show 'not collected'"
    );
}

// =============================================================================
// Default command name (lowercased struct/variant name)
// =============================================================================

#[test]
#[serial]
fn struct_default_command_name_is_lowercased() {
    let _guard = EnvGuard::enabled();
    let input = AutoNamedStruct { flag: true };
    let event = input.to_telemetry_event().expect("should produce event");
    assert_eq!(
        event.command, "autonamedstruct",
        "default command name should be the lowercased struct name"
    );
}

#[test]
#[serial]
fn enum_default_command_name_is_lowercased_variant() {
    let _guard = EnvGuard::enabled();
    let cmd = TestCommands::McpServer {
        transport: "stdio".into(),
    };
    let event = cmd.to_telemetry_event().expect("should produce event");
    assert_eq!(
        event.command, "mcpserver",
        "default command name should be the lowercased variant name"
    );
}
