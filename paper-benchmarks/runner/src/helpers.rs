//! Subprocess and build helpers — run commands, npm install, go mod tidy, pip venv.

use std::{collections::HashMap, env, fs, path::Path, process::Command};

use chrono::Utc;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Child process environment builder (shared by run_command and execute_script)
// ---------------------------------------------------------------------------

/// Allowlist of environment variables safe to pass to child processes.
///
/// Instead of inheriting the entire parent environment (which may contain
/// sensitive variables like `GITHUB_TOKEN`, the parent's own AWS credentials,
/// database passwords, etc.), we only pass through a curated allowlist of
/// variables needed for the language runtimes and build tools to function.
const PASSTHROUGH_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    // Go
    "GOPATH",
    "GOROOT",
    "GOPROXY",
    "GONOSUMCHECK",
    "GONOSUMDB",
    "GOPRIVATE",
    "GOFLAGS",
    // Java / Maven
    "JAVA_HOME",
    "M2_HOME",
    "MAVEN_HOME",
    "MAVEN_OPTS",
    // Node.js
    "NODE_PATH",
    "NPM_CONFIG_PREFIX",
    "NVM_DIR",
    // Python
    "PYTHONPATH",
    "VIRTUAL_ENV",
    // Misc toolchain
    "CARGO",
    "RUSTUP_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

/// Build a minimal, safe environment for child processes.
///
/// Passes through only the [`PASSTHROUGH_VARS`] allowlist from the parent
/// environment, then layers `extra_env` on top (which may include AWS
/// credentials for assumed roles, region overrides, etc.).
///
/// This is used by both [`run_command`] (for build/CDK subprocesses) and
/// [`crate::execution::execute_script`] (for language script execution).
#[must_use]
#[allow(clippy::implicit_hasher)]
pub(crate) fn build_safe_env(extra_env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env_map: HashMap<String, String> = HashMap::new();

    // Pass through allowlisted variables from the parent environment.
    for var in PASSTHROUGH_VARS {
        if let Ok(val) = env::var(var) {
            env_map.insert((*var).to_string(), val);
        }
    }

    // Layer caller-supplied overrides on top (AWS credentials, region, etc.).
    for (k, v) in extra_env {
        env_map.insert(k.clone(), v.clone());
    }

    env_map
}

// ---------------------------------------------------------------------------
// Sensitive data redaction
// ---------------------------------------------------------------------------

/// Redact an AWS account ID for safe display in logs/stdout.
///
/// Replaces all but the last 4 digits with asterisks, e.g. `123456789012` → `********9012`.
/// Returns the input unchanged if it doesn't look like a 12-digit account ID.
#[must_use]
fn redact_account_id(account: &str) -> String {
    if account.len() == 12 && account.chars().all(|c| c.is_ascii_digit()) {
        format!("********{}", &account[8..])
    } else {
        account.to_string()
    }
}

/// Redact an ARN for safe display in logs/stdout.
///
/// Masks the account-ID portion of the ARN (positions after the 4th colon).
/// Example: `arn:aws:iam::123456789012:role/Foo` → `arn:aws:iam::********9012:role/Foo`
#[must_use]
pub fn redact_arn(arn: &str) -> String {
    // ARN format: arn:partition:service:region:account:resource
    // We want to mask the account portion (5th field, 0-indexed = 4).
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    if parts.len() == 6 {
        let redacted_account = redact_account_id(parts[4]);
        format!(
            "{}:{}:{}:{}:{}:{}",
            parts[0], parts[1], parts[2], parts[3], redacted_account, parts[5]
        )
    } else {
        arn.to_string()
    }
}

/// Recursively redact AWS account IDs in a JSON value.
///
/// Walks the entire JSON tree and replaces any 12-digit numeric sequence that
/// appears in a string value in a position consistent with an AWS account ID
/// (inside ARNs or as a standalone 12-digit number) with `"XXXXXXXXXXXX"`.
#[must_use]
pub(crate) fn redact_json_account_ids(value: &Value, account: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(account, "XXXXXXXXXXXX")),
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| redact_json_account_ids(v, account))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_json_account_ids(v, account)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Subprocess helper (synchronous — captures output to avoid information leakage)
// ---------------------------------------------------------------------------

/// Run *argv* in *cwd* with a **sanitized** environment.
///
/// Only the [`PASSTHROUGH_VARS`] allowlist is inherited from the parent
/// process, with *extra_env* layered on top.  This prevents accidental
/// leakage of sensitive variables (e.g. `GITHUB_TOKEN`, `NPM_TOKEN`,
/// database passwords) to build tools, package managers, and CDK.
///
/// Subprocess stdout/stderr is **captured** and logged at `debug!` level
/// only.  This prevents CI logs from leaking AWS account IDs, stack ARNs,
/// and resource names that tools like CDK print by default.  Set
/// `RUST_LOG=debug` to see the full subprocess output during local
/// debugging.
///
/// Returns `true` if the process exits with code 0.
#[allow(clippy::implicit_hasher)]
pub(crate) fn run_command(argv: &[&str], cwd: &Path, extra_env: &HashMap<String, String>) -> bool {
    use std::process::Stdio;

    let safe_env = build_safe_env(extra_env);

    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(safe_env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stdout.is_empty() {
                for line in stdout.lines() {
                    debug!("[stdout] {}", line);
                }
            }
            if !stderr.is_empty() {
                for line in stderr.lines() {
                    debug!("[stderr] {}", line);
                }
            }
            if !output.status.success() {
                // Log last few lines of stderr at warn level to aid debugging
                // without exposing full output.  Limit to generic failure info.
                let exit_code = output.status.code().unwrap_or(-1);
                warn!(
                    "Command {:?} exited with code {} (set RUST_LOG=debug for full output)",
                    argv, exit_code
                );
            }
            output.status.success()
        }
        Err(e) => {
            error!("Failed to run {:?}: {}", argv, e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// npm install helper
// ---------------------------------------------------------------------------

/// Run `npm install` in *dir* if `node_modules/` is absent.
#[allow(clippy::implicit_hasher)]
pub(crate) fn npm_install_if_needed(dir: &Path, extra_env: &HashMap<String, String>) -> bool {
    if dir.join("node_modules").exists() {
        info!(
            "[npm] node_modules already present in {:?} -- skipping install",
            dir
        );
        return true;
    }
    info!(
        "[npm] node_modules not found in {:?} -- running npm install ...",
        dir
    );
    let ok = run_command(&["npm", "install"], dir, extra_env);
    if ok {
        info!("[npm] npm install complete in {:?}", dir);
    } else {
        error!("[npm] npm install failed in {:?}", dir);
    }
    ok
}

// ---------------------------------------------------------------------------
// go mod tidy helper
// ---------------------------------------------------------------------------

/// Run `go mod tidy` in *dir* if `go.sum` is absent.
///
/// `go mod tidy` (unlike `go mod download`) reliably writes `go.sum` with all
/// required checksums.  After the command succeeds we verify that `go.sum`
/// actually exists — if it doesn't, we report failure so the caller can abort
/// early with a clear error instead of a cryptic "missing go.sum entry" from
/// `go run`.
pub(crate) fn go_mod_tidy_if_needed(dir: &Path) -> bool {
    let go_sum = dir.join("go.sum");
    if go_sum.exists() {
        info!(
            "[go] go.sum already present in {:?} -- skipping go mod tidy",
            dir
        );
        return true;
    }
    info!(
        "[go] go.sum not found in {:?} -- running go mod tidy ...",
        dir
    );
    let empty_env: HashMap<String, String> = HashMap::new();
    let ok = run_command(&["go", "mod", "tidy"], dir, &empty_env);
    if ok && go_sum.exists() {
        info!("[go] go mod tidy complete in {:?}", dir);
        true
    } else if ok {
        error!(
            "[go] go mod tidy succeeded but go.sum still missing in {:?}",
            dir
        );
        false
    } else {
        error!("[go] go mod tidy failed in {:?}", dir);
        false
    }
}

// ---------------------------------------------------------------------------
// pip venv helper
// ---------------------------------------------------------------------------

/// Create a `.venv` in *dir* and `pip install -r requirements.txt` if the
/// venv does not yet exist.
pub(crate) fn pip_venv_if_needed(dir: &Path) -> bool {
    let venv_dir = dir.join(".venv");
    let empty_env: HashMap<String, String> = HashMap::new();

    if venv_dir.exists() {
        info!(
            "[python] .venv already present in {:?} -- skipping venv setup",
            dir
        );
        return true;
    }

    info!(
        "[python] .venv not found in {:?} -- creating virtual environment ...",
        dir
    );

    let ok = run_command(&["python3", "-m", "venv", ".venv"], dir, &empty_env);
    if !ok {
        error!("[python] python3 -m venv failed in {:?}", dir);
        return false;
    }

    let req_txt = dir.join("requirements.txt");
    if req_txt.exists() {
        info!("[python] installing requirements.txt in {:?} ...", dir);
        let pip = venv_dir.join("bin/pip");
        let pip_str = pip.to_string_lossy().into_owned();
        let req_str = req_txt.to_string_lossy().into_owned();
        let ok = run_command(&[&pip_str, "install", "-r", &req_str], dir, &empty_env);
        if ok {
            info!("[python] pip install complete in {:?}", dir);
        } else {
            error!("[python] pip install failed in {:?}", dir);
            return false;
        }
    } else {
        info!(
            "[python] no requirements.txt found in {:?} -- venv created without packages",
            dir
        );
    }

    true
}

// ---------------------------------------------------------------------------
// Minimal execution_log for early-failure paths
// ---------------------------------------------------------------------------

pub(crate) fn save_execution_log(
    results_dir: &Path,
    returncode: i32,
    stdout: &str,
    stderr: &str,
    success: bool,
    sdk_calls: Option<&Value>,
    sdk_analysis: Option<&Value>,
) {
    let log = json!({
        "returncode": returncode,
        "stdout": stdout,
        "stderr": stderr,
        "success": success,
        "sdk_calls": sdk_calls,
        "sdk_analysis": sdk_analysis,
        "timestamp": Utc::now().to_rfc3339(),
    });
    let path = results_dir.join("execution_log.json");
    let _ = fs::write(path, serde_json::to_string_pretty(&log).unwrap_or_default());
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── redact_account_id ────────────────────────────────────────────────────

    #[test]
    fn redact_account_id_masks_12_digit_id() {
        assert_eq!(redact_account_id("123456789012"), "********9012");
    }

    #[test]
    fn redact_account_id_leaves_non_12_digit_unchanged() {
        assert_eq!(redact_account_id("short"), "short");
        assert_eq!(redact_account_id("1234567890123"), "1234567890123");
    }

    #[test]
    fn redact_account_id_leaves_non_numeric_unchanged() {
        assert_eq!(redact_account_id("12345678901a"), "12345678901a");
    }

    // ── redact_arn ───────────────────────────────────────────────────────────

    #[test]
    fn redact_arn_masks_account_in_iam_arn() {
        assert_eq!(
            redact_arn("arn:aws:iam::123456789012:role/MyRole"),
            "arn:aws:iam::********9012:role/MyRole"
        );
    }

    #[test]
    fn redact_arn_masks_account_in_sts_arn() {
        assert_eq!(
            redact_arn("arn:aws:sts::123456789012:assumed-role/MyRole/session"),
            "arn:aws:sts::********9012:assumed-role/MyRole/session"
        );
    }

    #[test]
    fn redact_arn_leaves_non_arn_unchanged() {
        assert_eq!(redact_arn("not-an-arn"), "not-an-arn");
    }

    // ── redact_json_account_ids ──────────────────────────────────────────────

    #[test]
    fn redact_json_account_ids_replaces_in_resource_arn() {
        let policy = serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "arn:aws:s3:::my-bucket-123456789012-us-east-1/*"
            }]
        });
        let redacted = redact_json_account_ids(&policy, "123456789012");
        assert_eq!(
            redacted["Statement"][0]["Resource"],
            "arn:aws:s3:::my-bucket-XXXXXXXXXXXX-us-east-1/*"
        );
    }

    #[test]
    fn redact_json_account_ids_leaves_non_matching_unchanged() {
        let policy = serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*"
            }]
        });
        let redacted = redact_json_account_ids(&policy, "123456789012");
        assert_eq!(redacted, policy);
    }

    #[test]
    fn redact_json_account_ids_handles_nested_arrays() {
        let policy = serde_json::json!({
            "Statement": [{
                "Action": ["s3:GetObject"],
                "Resource": [
                    "arn:aws:s3:::bucket-123456789012/*",
                    "arn:aws:s3:::bucket-123456789012"
                ]
            }]
        });
        let redacted = redact_json_account_ids(&policy, "123456789012");
        let resources = redacted["Statement"][0]["Resource"].as_array().unwrap();
        assert_eq!(resources[0], "arn:aws:s3:::bucket-XXXXXXXXXXXX/*");
        assert_eq!(resources[1], "arn:aws:s3:::bucket-XXXXXXXXXXXX");
    }
}
