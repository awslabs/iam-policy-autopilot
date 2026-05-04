//! Data types and structures used across the runner library.

use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Language configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LangConfig {
    pub script_file: &'static str,
    /// Build the command argv given the script directory.
    pub run_cmd: fn(&Path) -> Vec<String>,
}

#[must_use]
pub fn language_configs() -> HashMap<&'static str, LangConfig> {
    let mut m = HashMap::new();

    m.insert(
        "python",
        LangConfig {
            script_file: "script.py",
            run_cmd: |dir| {
                vec![
                    dir.join(".venv/bin/python3").to_string_lossy().into(),
                    dir.join("script.py").to_string_lossy().into(),
                ]
            },
        },
    );
    m.insert(
        "go",
        LangConfig {
            script_file: "script.go",
            run_cmd: |dir| {
                vec![
                    "go".into(),
                    "run".into(),
                    dir.join("script.go").to_string_lossy().into(),
                ]
            },
        },
    );
    m.insert(
        "java",
        LangConfig {
            script_file: "Script.java",
            run_cmd: |dir| {
                vec![
                    "mvn".into(),
                    "compile".into(),
                    "exec:java".into(),
                    "-f".into(),
                    dir.join("pom.xml").to_string_lossy().into(),
                    "-Dexec.mainClass=Script".into(),
                ]
            },
        },
    );
    m.insert(
        "typescript",
        LangConfig {
            script_file: "script.ts",
            run_cmd: |dir| {
                vec![
                    "npx".into(),
                    "ts-node".into(),
                    dir.join("script.ts").to_string_lossy().into(),
                ]
            },
        },
    );

    m
}

// ---------------------------------------------------------------------------
// Data types for JSON output
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RoleInfo {
    pub role_name: String,
    pub role_arn: String,
    pub policy_names: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExecutionLog {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub sdk_calls: Option<Value>,
    pub sdk_analysis: Option<Value>,
    pub timestamp: String,
}

/// SDK operation counts extracted from the autopilot analysis.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SdkStats {
    pub total_operations: usize,
    pub single_service_operations: usize,
    pub multiple_service_operations: usize,
    pub total_additional_services: usize,
}

#[derive(Debug, Serialize)]
pub struct LangSummary {
    pub language: String,
    pub script_path: String,
    pub success: bool,
    pub failure_reason: Option<String>,
    pub stages: HashMap<String, bool>,
    pub sdk_stats: Option<SdkStats>,
    pub start_time: String,
    pub end_time: Option<String>,
}

/// Typed result for a CDK deploy or destroy step, serialised into `RunReport`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CdkStepResult {
    #[serde(default)]
    pub skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

impl CdkStepResult {
    #[must_use]
    pub fn skipped() -> Self {
        Self {
            skipped: true,
            success: None,
        }
    }
    #[must_use]
    pub fn done(ok: bool) -> Self {
        Self {
            skipped: false,
            success: Some(ok),
        }
    }
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.success.unwrap_or(false)
    }
    #[must_use]
    pub fn is_skipped(&self) -> bool {
        self.skipped
    }
}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub run_name: String,
    pub timestamp: String,
    pub region: String,
    pub languages: Vec<String>,
    pub cdk_deploy: CdkStepResult,
    pub language_results: HashMap<String, LangSummary>,
    pub cdk_destroy: CdkStepResult,
    pub overall_success: bool,
    pub start_time: String,
    pub end_time: Option<String>,
}

// ---------------------------------------------------------------------------
// iam-policy-autopilot output types
// ---------------------------------------------------------------------------

/// One entry in the `extract-sdk-calls` JSON array.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AutopilotSdkCall {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "PossibleServices", default)]
    pub possible_services: Vec<String>,
}

/// Top-level object returned by `generate-policies`.
#[derive(Debug, Deserialize)]
pub(crate) struct AutopilotPoliciesOutput {
    #[serde(rename = "Policies")]
    pub policies: Vec<AutopilotPolicyItem>,
}

/// One element of the `Policies` array.
#[derive(Debug, Deserialize)]
pub(crate) struct AutopilotPolicyItem {
    #[serde(rename = "Policy")]
    pub policy: Value,
}

// ---------------------------------------------------------------------------
// Script execution result
// ---------------------------------------------------------------------------

pub(crate) struct ExecResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::Path;

    // ── CdkStepResult ────────────────────────────────────────────────────────

    #[rstest]
    #[case::skipped(CdkStepResult::skipped(), true, false, None)]
    #[case::done_success(CdkStepResult::done(true), false, true, Some(true))]
    #[case::done_failure(CdkStepResult::done(false), false, false, Some(false))]
    fn cdk_step_result_accessors(
        #[case] result: CdkStepResult,
        #[case] expect_skipped: bool,
        #[case] expect_ok: bool,
        #[case] expect_success: Option<bool>,
    ) {
        assert_eq!(result.is_skipped(), expect_skipped);
        assert_eq!(result.is_ok(), expect_ok);
        assert_eq!(result.success, expect_success);
    }

    #[test]
    fn cdk_step_result_serialization_skipped_omits_success() {
        let json = serde_json::to_value(CdkStepResult::skipped()).unwrap();
        assert_eq!(json["skipped"], true);
        assert!(json.get("success").is_none());
    }

    #[test]
    fn cdk_step_result_serialization_done_includes_success() {
        let json = serde_json::to_value(CdkStepResult::done(true)).unwrap();
        assert_eq!(json["skipped"], false);
        assert_eq!(json["success"], true);
    }

    #[test]
    fn cdk_step_result_deserialization_defaults_skipped_to_false() {
        let r: CdkStepResult = serde_json::from_str(r#"{"success": true}"#).unwrap();
        assert!(!r.is_skipped());
        assert!(r.is_ok());
    }

    // ── language_configs ─────────────────────────────────────────────────────

    #[test]
    fn language_configs_contains_all_expected_languages() {
        let configs = language_configs();
        let expected = ["python", "go", "java", "typescript"];
        assert_eq!(configs.len(), expected.len());
        for lang in expected {
            assert!(configs.contains_key(lang), "missing language: {lang}");
        }
    }

    #[rstest]
    #[case::python("python", "script.py")]
    #[case::go("go", "script.go")]
    #[case::java("java", "Script.java")]
    #[case::typescript("typescript", "script.ts")]
    fn language_config_script_file(#[case] lang: &str, #[case] expected_file: &str) {
        let configs = language_configs();
        assert_eq!(configs[lang].script_file, expected_file);
    }

    #[rstest]
    #[case::python("python", "/tmp/p/python", &["/tmp/p/python/.venv/bin/python3", "/tmp/p/python/script.py"])]
    #[case::go("go", "/tmp/p/go", &["go", "run", "/tmp/p/go/script.go"])]
    #[case::java("java", "/tmp/p/java", &["mvn", "compile", "exec:java", "-f", "/tmp/p/java/pom.xml", "-Dexec.mainClass=Script"])]
    #[case::typescript("typescript", "/tmp/p/ts", &["npx", "ts-node", "/tmp/p/ts/script.ts"])]
    fn language_config_run_cmd(
        #[case] lang: &str,
        #[case] dir: &str,
        #[case] expected_argv: &[&str],
    ) {
        let configs = language_configs();
        let cmd = (configs[lang].run_cmd)(Path::new(dir));
        assert_eq!(
            cmd,
            expected_argv
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
    }

    // ── AutopilotSdkCall deserialization ─────────────────────────────────────

    #[rstest]
    #[case::with_services(
        r#"{"Name": "PutObject", "PossibleServices": ["s3", "s3-object-lambda"]}"#,
        "PutObject",
        vec!["s3".to_string(), "s3-object-lambda".to_string()]
    )]
    #[case::no_services(
        r#"{"Name": "GetObject"}"#,
        "GetObject",
        vec![]
    )]
    fn autopilot_sdk_call_deserialize(
        #[case] json: &str,
        #[case] expected_name: &str,
        #[case] expected_services: Vec<String>,
    ) {
        let call: AutopilotSdkCall = serde_json::from_str(json).unwrap();
        assert_eq!(call.name, expected_name);
        assert_eq!(call.possible_services, expected_services);
    }

    // ── AutopilotPoliciesOutput deserialization ──────────────────────────────

    #[test]
    fn autopilot_policies_output_deserialize() {
        let json = r#"{
            "Policies": [
                {"Policy": {"Version": "2012-10-17", "Statement": []}},
                {"Policy": {"Version": "2012-10-17", "Statement": []}}
            ]
        }"#;
        let output: AutopilotPoliciesOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.policies.len(), 2);
    }
}
