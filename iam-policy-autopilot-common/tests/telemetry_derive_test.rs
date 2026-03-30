//! Integration tests for `#[derive(TelemetryEvent)]` proc macro.
//!
//! These tests exercise the generated `ToTelemetryEvent` implementation on both
//! enums and structs, validating field recording, skip behavior, notice suppression,
//! and `telemetry_fields()` metadata.

use iam_policy_autopilot_common::telemetry::{
    TelemetryEventDerive, TelemetryFieldInfo, ToTelemetryEvent, TELEMETRY_ENV_VAR,
};
use serial_test::serial;

// =============================================================================
// Test helpers
// =============================================================================

/// Run a closure with telemetry enabled (env var = "1"), then restore original.
fn with_telemetry_enabled<F: FnOnce()>(f: F) {
    let original = std::env::var(TELEMETRY_ENV_VAR).ok();
    std::env::set_var(TELEMETRY_ENV_VAR, "1");
    f();
    match original {
        Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
        None => std::env::remove_var(TELEMETRY_ENV_VAR),
    }
}

/// Run a closure with telemetry disabled (env var = "0"), then restore original.
fn with_telemetry_disabled<F: FnOnce()>(f: F) {
    let original = std::env::var(TELEMETRY_ENV_VAR).ok();
    std::env::set_var(TELEMETRY_ENV_VAR, "0");
    f();
    match original {
        Some(v) => std::env::set_var(TELEMETRY_ENV_VAR, v),
        None => std::env::remove_var(TELEMETRY_ENV_VAR),
    }
}

// =============================================================================
// Enum test fixture
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
    Version {
        verbose: bool,
    },

    #[telemetry(skip_notice)]
    McpServer {
        #[telemetry(value)]
        transport: String,
    },

    // Bare variant with no telemetry attribute on fields — all fields should be skipped
    #[telemetry(command = "status")]
    Status {
        verbose: bool,
    },

    // Unit variant
    #[telemetry(command = "help")]
    Help,

    // Tuple variant (skip, to avoid field recording)
    #[telemetry(skip)]
    Internal(String),
}

// =============================================================================
// Struct test fixture
// =============================================================================

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

// =============================================================================
// Enum: to_telemetry_event() tests
// =============================================================================

#[test]
#[serial]
fn enum_generate_policies_records_all_field_modes() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::GeneratePolicies {
            source_files: vec!["file1.py".to_string(), "file2.py".to_string()],
            pretty: true,
            language: "python".to_string(),
            debug: true,
            region: Some("us-west-2".to_string()),
            service_hints: Some(vec!["s3".to_string(), "ec2".to_string()]),
            default_region: "us-east-1".to_string(),
        };

        let event = cmd.to_telemetry_event().expect("should produce event");
        assert_eq!(event.command, "generate-policies");

        let params = event.params.expect("should have params");

        // #[telemetry(presence)] on Vec — records !is_empty()
        assert_eq!(
            params.get("source_files"),
            Some(&serde_json::Value::Bool(true)),
            "presence on non-empty Vec should be true"
        );

        // #[telemetry(value)] on bool
        assert_eq!(
            params.get("pretty"),
            Some(&serde_json::Value::Bool(true)),
            "value on bool should record actual value"
        );

        // #[telemetry(value)] on String
        assert_eq!(
            params.get("language"),
            Some(&serde_json::Value::String("python".to_string())),
            "value on String should record to_string()"
        );

        // #[telemetry(skip)] — should not be present
        assert!(
            params.get("debug").is_none(),
            "skipped field should not appear in params"
        );

        // #[telemetry(value, if_present)] on Some
        assert_eq!(
            params.get("region"),
            Some(&serde_json::Value::String("us-west-2".to_string())),
            "value_if_present with Some should record the value"
        );

        // #[telemetry(list)] on non-empty vec
        let expected_list = serde_json::Value::Array(vec![
            serde_json::Value::String("s3".to_string()),
            serde_json::Value::String("ec2".to_string()),
        ]);
        assert_eq!(
            params.get("service_hints"),
            Some(&expected_list),
            "list should record as array"
        );

        // #[telemetry(presence, default = "us-east-1")] — value equals default
        assert_eq!(
            params.get("default_region"),
            Some(&serde_json::Value::Bool(false)),
            "presence with default should be false when value == default"
        );
    });
}

#[test]
#[serial]
fn enum_generate_policies_empty_and_none_values() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::GeneratePolicies {
            source_files: vec![],
            pretty: false,
            language: "go".to_string(),
            debug: false,
            region: None,
            service_hints: None,
            default_region: "eu-west-1".to_string(),
        };

        let event = cmd.to_telemetry_event().expect("should produce event");
        let params = event.params.expect("should have params");

        // Empty Vec → presence = false
        assert_eq!(
            params.get("source_files"),
            Some(&serde_json::Value::Bool(false)),
            "presence on empty Vec should be false"
        );

        // None → value_if_present omits the field entirely
        assert!(
            params.get("region").is_none(),
            "value_if_present with None should not appear in params"
        );

        // None → list omits the field entirely
        assert!(
            params.get("service_hints").is_none(),
            "list with None should not appear in params"
        );

        // Non-default value → presence with default = true
        assert_eq!(
            params.get("default_region"),
            Some(&serde_json::Value::Bool(true)),
            "presence with default should be true when value != default"
        );
    });
}

#[test]
#[serial]
fn enum_list_with_empty_vec_omits_field() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::GeneratePolicies {
            source_files: vec![],
            pretty: false,
            language: "go".to_string(),
            debug: false,
            region: None,
            service_hints: Some(vec![]), // Some but empty
            default_region: "us-east-1".to_string(),
        };

        let event = cmd.to_telemetry_event().expect("should produce event");
        let params = event.params.expect("should have params");

        assert!(
            params.get("service_hints").is_none(),
            "list with Some(empty vec) should not appear in params"
        );
    });
}

#[test]
#[serial]
fn enum_skip_variant_returns_none() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::Version { verbose: true };
        assert!(
            cmd.to_telemetry_event().is_none(),
            "skipped variant should return None"
        );
    });
}

#[test]
#[serial]
fn enum_skip_tuple_variant_returns_none() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::Internal("secret".to_string());
        assert!(
            cmd.to_telemetry_event().is_none(),
            "skipped tuple variant should return None"
        );
    });
}

#[test]
#[serial]
fn enum_unit_variant_produces_event_with_command_only() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::Help;
        let event = cmd.to_telemetry_event().expect("should produce event");
        assert_eq!(event.command, "help");
        assert!(
            event.params.is_none(),
            "unit variant should have no params"
        );
    });
}

#[test]
#[serial]
fn enum_variant_with_unannotated_fields_produces_event_no_params() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::Status { verbose: true };
        let event = cmd.to_telemetry_event().expect("should produce event");
        assert_eq!(event.command, "status");
        assert!(
            event.params.is_none(),
            "variant with all unannotated fields should have no params"
        );
    });
}

#[test]
#[serial]
fn enum_skip_notice_variant_records_event_but_suppresses_notice() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::McpServer {
            transport: "stdio".to_string(),
        };
        let event = cmd.to_telemetry_event().expect("skip_notice should still produce event");
        assert_eq!(event.command, "mcpserver");

        let params = event.params.expect("should have params");
        assert_eq!(
            params.get("transport"),
            Some(&serde_json::Value::String("stdio".to_string()))
        );
    });
}

#[test]
#[serial]
fn enum_returns_none_when_telemetry_disabled() {
    with_telemetry_disabled(|| {
        let cmd = TestCommands::GeneratePolicies {
            source_files: vec!["file.py".to_string()],
            pretty: true,
            language: "python".to_string(),
            debug: false,
            region: None,
            service_hints: None,
            default_region: "us-east-1".to_string(),
        };

        assert!(
            cmd.to_telemetry_event().is_none(),
            "should return None when telemetry is disabled"
        );
    });
}

// =============================================================================
// Enum: should_skip_notice() tests
// =============================================================================

#[test]
fn enum_should_skip_notice_for_skip_variant() {
    let cmd = TestCommands::Version { verbose: true };
    assert!(
        cmd.should_skip_notice(),
        "skip variant should skip notice"
    );
}

#[test]
fn enum_should_skip_notice_for_skip_notice_variant() {
    let cmd = TestCommands::McpServer {
        transport: "stdio".to_string(),
    };
    assert!(
        cmd.should_skip_notice(),
        "skip_notice variant should skip notice"
    );
}

#[test]
fn enum_should_not_skip_notice_for_normal_variant() {
    let cmd = TestCommands::GeneratePolicies {
        source_files: vec![],
        pretty: false,
        language: "python".to_string(),
        debug: false,
        region: None,
        service_hints: None,
        default_region: "us-east-1".to_string(),
    };
    assert!(
        !cmd.should_skip_notice(),
        "normal variant should not skip notice"
    );
}

#[test]
fn enum_should_skip_notice_for_skip_tuple_variant() {
    let cmd = TestCommands::Internal("data".to_string());
    assert!(
        cmd.should_skip_notice(),
        "skipped tuple variant should skip notice"
    );
}

#[test]
fn enum_should_not_skip_notice_for_unit_variant() {
    let cmd = TestCommands::Help;
    assert!(
        !cmd.should_skip_notice(),
        "unit variant without skip should not skip notice"
    );
}

// =============================================================================
// Enum: telemetry_fields() tests
// =============================================================================

#[test]
fn enum_telemetry_fields_includes_all_non_skip_variants() {
    let fields = TestCommands::telemetry_fields();

    // GeneratePolicies has 7 fields, McpServer has 1, Status has 1 (unannotated but still reported)
    // Version is skipped at container level, Internal is skipped at container level
    let generate_fields: Vec<&TelemetryFieldInfo> = fields
        .iter()
        .filter(|f| f.command == "generate-policies")
        .collect();

    assert_eq!(
        generate_fields.len(),
        7,
        "generate-policies should report all 7 fields in metadata"
    );

    // Check that skip fields are still reported (with "not collected" mode)
    let debug_field = generate_fields
        .iter()
        .find(|f| f.field_name == "debug")
        .expect("debug field should be in metadata");
    assert_eq!(debug_field.collection_mode, "not collected");

    // Check presence field
    let source_field = generate_fields
        .iter()
        .find(|f| f.field_name == "source_files")
        .expect("source_files should be in metadata");
    assert_eq!(source_field.collection_mode, "presence (boolean)");

    // Check value field
    let pretty_field = generate_fields
        .iter()
        .find(|f| f.field_name == "pretty")
        .expect("pretty should be in metadata");
    assert_eq!(pretty_field.collection_mode, "actual value");

    // Check value_if_present
    let region_field = generate_fields
        .iter()
        .find(|f| f.field_name == "region")
        .expect("region should be in metadata");
    assert_eq!(region_field.collection_mode, "value if provided, omitted otherwise");

    // Check list
    let hints_field = generate_fields
        .iter()
        .find(|f| f.field_name == "service_hints")
        .expect("service_hints should be in metadata");
    assert_eq!(hints_field.collection_mode, "list of values if non-empty, omitted otherwise");

    // Check presence with default
    let default_field = generate_fields
        .iter()
        .find(|f| f.field_name == "default_region")
        .expect("default_region should be in metadata");
    assert_eq!(default_field.collection_mode, "whether non-default (boolean)");
}

#[test]
fn enum_telemetry_fields_omits_container_skipped_variants() {
    let fields = TestCommands::telemetry_fields();

    // Version variant is skipped — its fields should NOT appear in metadata
    let version_fields: Vec<&TelemetryFieldInfo> = fields
        .iter()
        .filter(|f| f.command == "version")
        .collect();
    assert!(
        version_fields.is_empty(),
        "skipped variant fields should not appear in telemetry_fields()"
    );
}

// =============================================================================
// Struct: to_telemetry_event() tests
// =============================================================================

#[test]
#[serial]
fn struct_records_all_field_modes() {
    with_telemetry_enabled(|| {
        let input = TestMcpInput {
            source_files: vec!["app.py".to_string()],
            pretty: true,
            region: Some("us-west-2".to_string()),
            service_hints: Some(vec!["s3".to_string()]),
            account: Some("123456789".to_string()),
            internal_id: "secret-id".to_string(),
        };

        let event = input.to_telemetry_event().expect("should produce event");
        assert_eq!(event.command, "mcp-tool-generate");

        let params = event.params.expect("should have params");

        assert_eq!(
            params.get("source_files"),
            Some(&serde_json::Value::Bool(true)),
            "presence on non-empty Vec"
        );
        assert_eq!(
            params.get("pretty"),
            Some(&serde_json::Value::Bool(true)),
            "value on bool"
        );
        assert_eq!(
            params.get("region"),
            Some(&serde_json::Value::String("us-west-2".to_string())),
            "value_if_present with Some"
        );
        assert_eq!(
            params.get("account"),
            Some(&serde_json::Value::Bool(true)),
            "presence on Some"
        );

        let expected_hints = serde_json::Value::Array(vec![serde_json::Value::String(
            "s3".to_string(),
        )]);
        assert_eq!(params.get("service_hints"), Some(&expected_hints), "list");

        assert!(
            params.get("internal_id").is_none(),
            "unannotated field should not appear in params"
        );
    });
}

#[test]
#[serial]
fn struct_none_and_empty_values() {
    with_telemetry_enabled(|| {
        let input = TestMcpInput {
            source_files: vec![],
            pretty: false,
            region: None,
            service_hints: None,
            account: None,
            internal_id: "id".to_string(),
        };

        let event = input.to_telemetry_event().expect("should produce event");
        let params = event.params.expect("should have params");

        assert_eq!(
            params.get("source_files"),
            Some(&serde_json::Value::Bool(false)),
            "presence on empty Vec"
        );
        assert!(params.get("region").is_none(), "value_if_present with None");
        assert!(params.get("service_hints").is_none(), "list with None");
        assert_eq!(
            params.get("account"),
            Some(&serde_json::Value::Bool(false)),
            "presence on None"
        );
    });
}

#[test]
#[serial]
fn struct_returns_none_when_telemetry_disabled() {
    with_telemetry_disabled(|| {
        let input = TestMcpInput {
            source_files: vec!["file.py".to_string()],
            pretty: true,
            region: None,
            service_hints: None,
            account: None,
            internal_id: "id".to_string(),
        };

        assert!(
            input.to_telemetry_event().is_none(),
            "should return None when telemetry is disabled"
        );
    });
}

// =============================================================================
// Struct: should_skip_notice() tests
// =============================================================================

#[test]
fn struct_should_not_skip_notice_by_default() {
    let input = TestMcpInput {
        source_files: vec![],
        pretty: false,
        region: None,
        service_hints: None,
        account: None,
        internal_id: "id".to_string(),
    };
    assert!(
        !input.should_skip_notice(),
        "struct without skip_notice should not skip"
    );
}

#[test]
fn struct_should_skip_notice_when_annotated() {
    let input = TestSkipNoticeStruct {
        name: "test".to_string(),
    };
    assert!(
        input.should_skip_notice(),
        "struct with skip_notice should skip"
    );
}

// =============================================================================
// Struct: telemetry_fields() tests
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
// Edge case: default command name (lowercased)
// =============================================================================

#[allow(dead_code)]
#[derive(TelemetryEventDerive)]
struct AutoNamedStruct {
    #[telemetry(value)]
    flag: bool,
}

#[test]
#[serial]
fn struct_default_command_name_is_lowercased() {
    with_telemetry_enabled(|| {
        let input = AutoNamedStruct { flag: true };
        let event = input.to_telemetry_event().expect("should produce event");
        assert_eq!(
            event.command, "autonamedstruct",
            "default command name should be the lowercased struct name"
        );
    });
}

#[test]
#[serial]
fn enum_default_command_name_is_lowercased_variant() {
    with_telemetry_enabled(|| {
        let cmd = TestCommands::McpServer {
            transport: "stdio".to_string(),
        };
        let event = cmd.to_telemetry_event().expect("should produce event");
        assert_eq!(
            event.command, "mcpserver",
            "default command name should be the lowercased variant name"
        );
    });
}
