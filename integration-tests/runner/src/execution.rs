//! Script execution — assume role, run language scripts, and the per-language pipeline.

use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;
use chrono::Utc;
use serde_json::Value;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::autopilot::{analyze_sdk_calls, extract_sdk_calls, generate_policies};
use crate::aws::{stderr_indicates_transient_credential_error, verify_credentials};
use crate::helpers::{
    build_safe_env, go_mod_tidy_if_needed, npm_install_if_needed, pip_venv_if_needed, redact_arn,
    redact_json_account_ids, save_execution_log,
};
use crate::iam::{cleanup_execution_role, create_execution_role};
use crate::types::{ExecResult, ExecutionLog, LangConfig, LangSummary, RoleInfo, SdkStats};

// ---------------------------------------------------------------------------
// Script execution
// ---------------------------------------------------------------------------

/// Assume *role_info.role_arn*, inject credentials, and run the language script.
///
/// The role assumption is retried with exponential back-off (up to 6 attempts,
/// ~63 s total wait) to handle transient STS errors and IAM propagation delays
/// for newly created roles.
async fn execute_script(
    language: &str,
    script_dir: &Path,
    role_info: &RoleInfo,
    region: &str,
    sts: &StsClient,
    lang_cfg: &LangConfig,
) -> ExecResult {
    // Assume execution role with retry + exponential back-off.
    // Newly created IAM roles may not be immediately assumable (IAM eventual
    // consistency), and transient STS service errors can occur under load.
    const MAX_ASSUME_ATTEMPTS: u32 = 6;
    let session_name = format!("runner-{language}-session");

    debug!(
        "[execute_script] Assuming role {} for language {} (up to {} attempts) ...",
        redact_arn(&role_info.role_arn),
        language,
        MAX_ASSUME_ATTEMPTS
    );

    let mut last_err_msg = String::new();
    let mut creds_opt = None;

    for attempt in 0..MAX_ASSUME_ATTEMPTS {
        match sts
            .assume_role()
            .role_arn(&role_info.role_arn)
            .role_session_name(&session_name)
            .send()
            .await
        {
            Ok(resp) => {
                if let Some(c) = resp.credentials {
                    if attempt > 0 {
                        debug!(
                            "[execute_script] sts:AssumeRole succeeded on attempt {}/{} for {} (language {})",
                            attempt + 1, MAX_ASSUME_ATTEMPTS, redact_arn(&role_info.role_arn), language
                        );
                    }
                    creds_opt = Some(c);
                } else {
                    last_err_msg =
                        format!(
                        "AssumeRole returned no credentials for role (language {}, attempt {}/{})",
                        language, attempt + 1, MAX_ASSUME_ATTEMPTS
                    );
                    warn!("[execute_script] {}", last_err_msg);
                    // No point retrying if the response itself has no credentials.
                }
                break;
            }
            Err(e) => {
                let wait = 2u64.pow(attempt); // 1, 2, 4, 8, 16, 32 s
                let detail = format!("{e:#}");
                last_err_msg = format!(
                    "sts:AssumeRole attempt {}/{} failed for role (language {}): {}",
                    attempt + 1,
                    MAX_ASSUME_ATTEMPTS,
                    language,
                    detail
                );
                if attempt + 1 < MAX_ASSUME_ATTEMPTS {
                    warn!(
                        "[execute_script] {} — retrying in {}s ...",
                        last_err_msg, wait
                    );
                    sleep(Duration::from_secs(wait)).await;
                } else {
                    error!("[execute_script] {} — no retries remaining", last_err_msg);
                }
            }
        }
    }

    let creds = if let Some(c) = creds_opt {
        c
    } else {
        error!(
            "[execute_script] Failed to assume execution role after {} attempts: {}",
            MAX_ASSUME_ATTEMPTS, last_err_msg
        );
        return ExecResult {
            returncode: -1,
            stdout: String::new(),
            stderr: format!(
                "Failed to assume execution role after {MAX_ASSUME_ATTEMPTS} attempts: {last_err_msg}"
            ),
            success: false,
        };
    };

    // ── Credential warm-up ──────────────────────────────────────────────────
    // The AssumeRole call can succeed but the returned credentials may not
    // yet be usable across all STS/service endpoints (IAM eventual
    // consistency).  We verify the credentials with a lightweight
    // GetCallerIdentity call before handing them to the child process.
    {
        const WARMUP_ATTEMPTS: u32 = 10;
        const WARMUP_INTERVAL_SECS: u64 = 2;
        let mut warmup_ok = false;
        for attempt in 0..WARMUP_ATTEMPTS {
            if verify_credentials(
                &creds.access_key_id,
                &creds.secret_access_key,
                &creds.session_token,
                region,
            )
            .await
            {
                if attempt > 0 {
                    debug!(
                        "[execute_script] Credential warm-up succeeded on attempt {}/{} for {} (language {})",
                        attempt + 1, WARMUP_ATTEMPTS, redact_arn(&role_info.role_arn), language
                    );
                }
                warmup_ok = true;
                break;
            } else if attempt + 1 < WARMUP_ATTEMPTS {
                warn!(
                    "[execute_script] Credential warm-up attempt {}/{} failed for language {} — retrying in {}s ...",
                    attempt + 1, WARMUP_ATTEMPTS, language, WARMUP_INTERVAL_SECS
                );
                sleep(Duration::from_secs(WARMUP_INTERVAL_SECS)).await;
            } else {
                error!(
                    "[execute_script] Credential warm-up failed after {} attempts for language {}",
                    WARMUP_ATTEMPTS, language
                );
            }
        }
        if !warmup_ok {
            return ExecResult {
                returncode: -1,
                stdout: String::new(),
                stderr: format!(
                    "Credential warm-up failed after {WARMUP_ATTEMPTS} attempts — assumed-role credentials are not yet usable"
                ),
                success: false,
            };
        }
    }

    // Build a minimal environment for the child process.
    // Only pass through safe, non-sensitive variables plus the assumed-role credentials.
    let mut aws_env: HashMap<String, String> = HashMap::new();
    aws_env.insert("AWS_ACCESS_KEY_ID".into(), creds.access_key_id.clone());
    aws_env.insert(
        "AWS_SECRET_ACCESS_KEY".into(),
        creds.secret_access_key.clone(),
    );
    aws_env.insert("AWS_SESSION_TOKEN".into(), creds.session_token.clone());
    aws_env.insert("AWS_DEFAULT_REGION".into(), region.into());
    aws_env.insert("AWS_REGION".into(), region.into());
    let child_env = build_safe_env(&aws_env);

    let argv = (lang_cfg.run_cmd)(script_dir);
    info!("[RUN] {}: {}", language, argv.join(" "));

    let output = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(script_dir)
        .env_clear()
        .envs(child_env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Err(e) => ExecResult {
            returncode: -1,
            stdout: String::new(),
            stderr: format!("Failed to spawn process: {e}"),
            success: false,
        },
        Ok(out) => {
            let rc = out.status.code().unwrap_or(-1);
            ExecResult {
                returncode: rc,
                stdout: String::from_utf8_lossy(&out.stdout).into(),
                stderr: String::from_utf8_lossy(&out.stderr).into(),
                success: rc == 0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-language pipeline
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn run_language(
    language: &str,
    run_dir: &Path,
    results_lang_dir: &Path,
    region: &str,
    account: &str,
    cleanup_roles: bool,
    verbose_logs: bool,
    iam: &IamClient,
    sts: &StsClient,
    lang_cfg: &LangConfig,
    caller_arn: &str,
) -> LangSummary {
    fs::create_dir_all(results_lang_dir).ok();

    let script_dir = run_dir.join(language);
    let script_file = script_dir.join(lang_cfg.script_file);
    let start_time = Utc::now().to_rfc3339();

    let mut summary = LangSummary {
        language: language.to_string(),
        script_path: script_file.to_string_lossy().into(),
        success: false,
        failure_reason: None,
        stages: HashMap::new(),
        sdk_stats: None,
        start_time: start_time.clone(),
        end_time: None,
    };

    println!("\n{}", "=".repeat(60));
    println!("  Language: {}", language.to_uppercase());
    println!("  Script:   {script_file:?}");
    println!("{}", "=".repeat(60));

    if !script_file.exists() {
        let msg = format!("Script file not found: {script_file:?}");
        error!("{}", msg);
        summary.failure_reason = Some(msg.clone());
        summary.stages.insert("script_found".into(), false);
        save_execution_log(results_lang_dir, -1, "", &msg, false, None, None);
        return summary;
    }
    summary.stages.insert("script_found".into(), true);

    // ── TypeScript: ensure local node_modules are present before npx ts-node ─
    if language == "typescript" {
        let empty_env: HashMap<String, String> = HashMap::new();
        if !npm_install_if_needed(&script_dir, &empty_env) {
            let msg = "npm install failed in typescript dir".to_string();
            error!("{}", msg);
            summary.failure_reason = Some(msg.clone());
            summary.stages.insert("npm_install".into(), false);
            save_execution_log(results_lang_dir, -1, "", &msg, false, None, None);
            return summary;
        }
        summary.stages.insert("npm_install".into(), true);
    }

    // ── Go: ensure go.sum is populated before go run ──────────────────────────
    if language == "go" {
        if !go_mod_tidy_if_needed(&script_dir) {
            let msg = "go mod download failed in go dir".to_string();
            error!("{}", msg);
            summary.failure_reason = Some(msg.clone());
            summary.stages.insert("go_mod_download".into(), false);
            save_execution_log(results_lang_dir, -1, "", &msg, false, None, None);
            return summary;
        }
        summary.stages.insert("go_mod_download".into(), true);
    }

    // ── Python: create .venv and pip-install requirements.txt if needed ───────
    if language == "python" {
        if !pip_venv_if_needed(&script_dir) {
            let msg = "pip venv setup failed in python dir".to_string();
            error!("{}", msg);
            summary.failure_reason = Some(msg.clone());
            summary.stages.insert("pip_venv".into(), false);
            save_execution_log(results_lang_dir, -1, "", &msg, false, None, None);
            return summary;
        }
        summary.stages.insert("pip_venv".into(), true);
    }

    // ── Stage 1: Extract SDK calls ──────────────────────────────────────────
    info!("Stage 1: Extracting SDK calls ...");
    let sdk_calls_typed = extract_sdk_calls(&script_file);
    let sdk_calls_val: Option<Value> = sdk_calls_typed
        .as_deref()
        .map(|calls| serde_json::to_value(calls).unwrap_or(Value::Null));
    let sdk_analysis_val: Option<Value>;

    if let Some(ref calls) = sdk_calls_typed {
        let analysis = analyze_sdk_calls(calls);
        info!(
            "[OK] Extracted {} operations ({} single-service, {} multi-service)",
            analysis["total_operations"],
            analysis["single_service_operations"],
            analysis["multiple_service_operations"],
        );
        let sdk_path = results_lang_dir.join("sdk_calls.json");
        let _ = fs::write(
            &sdk_path,
            serde_json::to_string_pretty(calls).unwrap_or_default(),
        );
        info!("[saved] sdk_calls.json");
        #[allow(clippy::cast_possible_truncation)]
        {
            summary.sdk_stats = Some(SdkStats {
                total_operations: analysis["total_operations"].as_u64().unwrap_or(0) as usize,
                single_service_operations: analysis["single_service_operations"]
                    .as_u64()
                    .unwrap_or(0) as usize,
                multiple_service_operations: analysis["multiple_service_operations"]
                    .as_u64()
                    .unwrap_or(0) as usize,
                total_additional_services: analysis["total_additional_services"]
                    .as_u64()
                    .unwrap_or(0) as usize,
            });
        }
        sdk_analysis_val = Some(analysis);
        summary.stages.insert("sdk_extraction".into(), true);
    } else {
        warn!("SDK extraction failed -- continuing with policy generation");
        summary.stages.insert("sdk_extraction".into(), false);
        sdk_analysis_val = None;
    }

    // ── Stage 2: Generate IAM policies ─────────────────────────────────────
    info!("Stage 2: Generating IAM policies ...");
    let policies = generate_policies(&script_file, region, account);

    let policies = if let Some(p) = policies {
        p
    } else {
        let msg = "iam-policy-autopilot policy generation failed".to_string();
        error!("{}", msg);
        summary.failure_reason = Some(msg.clone());
        summary.stages.insert("policy_generation".into(), false);
        save_execution_log(
            results_lang_dir,
            -1,
            "",
            &msg,
            false,
            sdk_calls_val.as_ref(),
            sdk_analysis_val.as_ref(),
        );
        return summary;
    };

    summary.stages.insert("policy_generation".into(), true);
    let total_stmts: usize = policies
        .iter()
        .map(|p| {
            p.get("Statement")
                .and_then(|s| s.as_array())
                .map(std::vec::Vec::len)
                .unwrap_or(0)
        })
        .sum();
    info!(
        "[OK] Generated {} policies with {} statements",
        policies.len(),
        total_stmts
    );
    let policy_path = results_lang_dir.join("policy.json");
    let redacted_policies: Vec<Value> = policies
        .iter()
        .map(|p| redact_json_account_ids(p, account))
        .collect();
    let _ = fs::write(
        &policy_path,
        serde_json::to_string_pretty(&redacted_policies).unwrap_or_default(),
    );
    info!("[saved] policy.json");

    // ── Stage 3: Create execution role ──────────────────────────────────────
    info!("Stage 3: Creating execution role ...");
    let role_suffix = format!(
        "{}-{}",
        run_dir.file_name().unwrap_or_default().to_string_lossy(),
        language
    );
    let role_info =
        match create_execution_role(iam, policies, &role_suffix, account, caller_arn).await {
            Ok(r) => r,
            Err(_e) => {
                let msg = "IAM execution role creation failed.".to_string();
                error!("{}", msg);
                summary.failure_reason = Some(msg.clone());
                summary.stages.insert("role_creation".into(), false);
                save_execution_log(
                    results_lang_dir,
                    -1,
                    "",
                    &msg,
                    false,
                    sdk_calls_val.as_ref(),
                    sdk_analysis_val.as_ref(),
                );
                return summary;
            }
        };

    summary.stages.insert("role_creation".into(), true);
    info!("[OK] Created role: {}", role_info.role_name);

    // ── Stage 4: Execute script ─────────────────────────────────────────────
    info!("Stage 4: Executing {} script ...", language);
    let exec = execute_script(language, &script_dir, &role_info, region, sts, lang_cfg).await;

    summary
        .stages
        .insert("script_execution".into(), exec.success);
    if exec.success {
        info!("[OK] Script succeeded");
        summary.success = true;
    } else {
        let msg = format!("Script execution failed (exit code {})", exec.returncode);
        error!("{}", msg);
        summary.failure_reason = Some(msg);
    }

    // Build sdk_analysis without operations_breakdown for the execution log.
    let sdk_analysis_compact = sdk_analysis_val.as_ref().map(|a| {
        let mut m = a.as_object().cloned().unwrap_or_default();
        m.remove("operations_breakdown");
        Value::Object(m)
    });

    let exec_log = ExecutionLog {
        returncode: exec.returncode,
        stdout: if verbose_logs {
            exec.stdout.clone()
        } else {
            String::new()
        },
        stderr: if verbose_logs {
            exec.stderr.clone()
        } else {
            String::new()
        },
        success: exec.success,
        sdk_calls: sdk_calls_val.clone(),
        sdk_analysis: sdk_analysis_compact,
        timestamp: Utc::now().to_rfc3339(),
    };
    let log_path = results_lang_dir.join("execution_log.json");
    let _ = fs::write(
        &log_path,
        serde_json::to_string_pretty(&exec_log).unwrap_or_default(),
    );
    info!("[saved] execution_log.json");

    // ── Stage 5: Cleanup execution role ─────────────────────────────────────
    if cleanup_roles {
        cleanup_execution_role(iam, &role_info).await;
    }

    summary.end_time = Some(Utc::now().to_rfc3339());
    summary
}

// ---------------------------------------------------------------------------
// run_language_with_policies
// ---------------------------------------------------------------------------

/// Run a single language script with an already-known set of policies.
#[allow(clippy::too_many_arguments)]
pub async fn run_language_with_policies(
    language: &str,
    run_dir: &Path,
    results_dir: &Path,
    policies: Vec<Value>,
    region: &str,
    account: &str,
    keep_role: bool,
    verbose_logs: bool,
    iam: &IamClient,
    sts: &StsClient,
    lang_cfg: &LangConfig,
    caller_arn: &str,
) -> LangSummary {
    const MAX_SCRIPT_ATTEMPTS: u32 = 3;

    fs::create_dir_all(results_dir).ok();

    let script_dir = run_dir.join(language);
    let start_time = Utc::now().to_rfc3339();

    let mut summary = LangSummary {
        language: language.to_string(),
        script_path: script_dir
            .join(lang_cfg.script_file)
            .to_string_lossy()
            .into(),
        success: false,
        failure_reason: None,
        stages: HashMap::new(),
        sdk_stats: None,
        start_time,
        end_time: None,
    };

    // Step 1: Create execution role.
    let role_suffix = format!(
        "{}-{}",
        run_dir.file_name().unwrap_or_default().to_string_lossy(),
        language
    );
    let role_info =
        match create_execution_role(iam, policies, &role_suffix, account, caller_arn).await {
            Ok(r) => r,
            Err(_e) => {
                let msg = "IAM execution role creation failed.".to_string();
                error!("{}", msg);
                summary.failure_reason = Some(msg.clone());
                summary.stages.insert("role_creation".into(), false);
                save_execution_log(results_dir, -1, "", &msg, false, None, None);
                return summary;
            }
        };
    summary.stages.insert("role_creation".into(), true);

    // Step 2: Execute script (with retry for transient credential errors).
    //
    // Even with the credential warm-up in execute_script(), the child
    // process may occasionally hit transient IAM propagation errors.
    // We use a two-pronged detection strategy:
    //
    //   (a) **Credential probe:** re-assume the role and call
    //       sts:GetCallerIdentity.  If the probe fails, the credentials
    //       are still propagating → retry.
    //
    //   (b) **Stderr pattern matching:** even when the probe succeeds
    //       (because sts:GetCallerIdentity doesn't require IAM auth and
    //       always works against STS), the script's stderr may contain
    //       transient credential errors like `InvalidAccessKeyId` or
    //       `InvalidSecurityToken` from other services (S3, Glue, etc.)
    //       that haven't received the credential propagation yet → retry.
    //
    // Only if *both* the probe succeeds AND stderr does NOT contain
    // transient credential patterns do we conclude the failure is genuine.
    let mut exec = execute_script(language, &script_dir, &role_info, region, sts, lang_cfg).await;

    if !exec.success {
        let session_name = format!("runner-{language}-probe");
        for retry in 1..MAX_SCRIPT_ATTEMPTS {
            // Check stderr for transient credential error patterns first.
            // This catches cases where sts:GetCallerIdentity succeeds but
            // other services (S3, Glue, DynamoDB) reject the credentials
            // because they haven't propagated yet.
            let stderr_transient = stderr_indicates_transient_credential_error(&exec.stderr);

            // Re-assume the role and verify the credentials from the runner.
            let creds_ok = match sts
                .assume_role()
                .role_arn(&role_info.role_arn)
                .role_session_name(&session_name)
                .send()
                .await
            {
                Ok(resp) => match resp.credentials {
                    Some(c) => {
                        verify_credentials(
                            &c.access_key_id,
                            &c.secret_access_key,
                            &c.session_token,
                            region,
                        )
                        .await
                    }
                    None => false,
                },
                Err(_) => false,
            };

            if creds_ok && !stderr_transient {
                // Credentials are valid AND stderr doesn't contain transient
                // credential errors — the failure was a genuine application
                // or permissions error, not a propagation issue.
                info!(
                    "[run_language_with_policies] Credential probe succeeded for {} — \
                     script failure is not a transient credential issue, skipping retry",
                    language
                );
                break;
            }

            // Either the probe failed OR stderr contains transient credential
            // errors — wait and retry with fresh credentials.
            if stderr_transient {
                warn!(
                    "[run_language_with_policies] Transient credential error detected in stderr \
                     for {} (attempt {}/{}): stderr contains InvalidAccessKeyId/InvalidSecurityToken \
                     — waiting 5s before retry ...",
                    language, retry + 1, MAX_SCRIPT_ATTEMPTS
                );
            } else {
                warn!(
                    "[run_language_with_policies] Credential probe failed for {} (attempt {}/{}), \
                     waiting 5s before retry ...",
                    language,
                    retry + 1,
                    MAX_SCRIPT_ATTEMPTS
                );
            }
            sleep(Duration::from_secs(5)).await;
            exec = execute_script(language, &script_dir, &role_info, region, sts, lang_cfg).await;
            if exec.success {
                info!(
                    "[run_language_with_policies] Retry {}/{} succeeded for {}",
                    retry + 1,
                    MAX_SCRIPT_ATTEMPTS,
                    language
                );
                break;
            }
        }
    }

    summary
        .stages
        .insert("script_execution".into(), exec.success);

    if exec.success {
        summary.success = true;
    } else {
        summary.failure_reason = Some(format!(
            "Script execution failed (exit code {})",
            exec.returncode
        ));
    }

    // Step 3: Save execution log.
    // stdout/stderr are only included when --verbose-logs is passed to avoid
    // leaking sensitive information (account IDs, resource names, etc.).
    let exec_log = ExecutionLog {
        returncode: exec.returncode,
        stdout: if verbose_logs {
            exec.stdout.clone()
        } else {
            String::new()
        },
        stderr: if verbose_logs {
            exec.stderr.clone()
        } else {
            String::new()
        },
        success: exec.success,
        sdk_calls: None,
        sdk_analysis: None,
        timestamp: Utc::now().to_rfc3339(),
    };
    let log_path = results_dir.join("execution_log.json");
    let _ = fs::write(
        &log_path,
        serde_json::to_string_pretty(&exec_log).unwrap_or_default(),
    );

    // Step 4: Cleanup role unless keep_role is set.
    if !keep_role {
        cleanup_execution_role(iam, &role_info).await;
    }

    summary.end_time = Some(Utc::now().to_rfc3339());
    summary
}
