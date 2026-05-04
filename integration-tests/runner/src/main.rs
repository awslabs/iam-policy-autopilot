//! Integration-test per-run orchestrator
//!
//! Workflow:
//!   1. CDK bootstrap + deploy  (`bash deploy.sh` inside <run_dir>/cdk/)
//!   2. For each language (python, go, java, typescript):
//!      a. extract-sdk-calls  via iam-policy-autopilot
//!      b. generate-policies  via iam-policy-autopilot
//!      c. Create temporary IAM execution role with those policies
//!      d. Assume the role and run the language script
//!      e. Write sdk_calls.json / policy.json / role_info.json / execution_log.json
//!   3. CDK destroy
//!   4. Write run_report.json + print final summary
//!
//! Usage:
//!   runner [options]
//!
//! Run `runner --help` for the full option list.

use runner::autopilot::generate_individual_policies;
use runner::aws::{get_aws_account_id, get_caller_arn};
use runner::cdk::{cdk_deploy, cdk_destroy};
use runner::execution::{run_language, run_language_with_policies};
use runner::helpers::redact_arn;
use runner::iam::{cleanup_execution_role, create_deploy_role};
use runner::types::{language_configs, CdkStepResult, RunReport};

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use aws_config::Region;
use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;
use chrono::Local;
use clap::Parser;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "runner",
    about = "Integration-test orchestrator: CDK deploy → iam-policy-autopilot → exec role → run → CDK destroy.\nBy default runs ALL run_*/ directories found in --projects-dir (or the current directory).",
    after_help = r#"Examples:
   # Run all discovered run_*/ directories under a specific path
   runner --projects-dir ./integration-tests/projects

   # Run only one specific example
   runner --projects-dir ./integration-tests/projects --only run_001

   # Only run Python and Go for a specific example, skip CDK destroy
   runner --projects-dir ./integration-tests/projects \
       --only run_001 \
       --languages python,go \
       --skip-destroy

   # Assume CDK is already deployed, just run scripts for one example
   runner --projects-dir ./integration-tests/projects \
       --only run_001 --skip-deploy --skip-destroy

   # Note: A deploy-role is created automatically per-run from
   # deploy_policy.json (if present in the run directory).
"#
)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Directory containing run_*/ subdirectories.
    /// Defaults to the current working directory if omitted.
    #[arg(long)]
    projects_dir: Option<PathBuf>,

    /// Run only this specific run directory (e.g. run_001-3478634b).
    /// If omitted, all run_*/ directories in --projects-dir are run.
    #[arg(long)]
    only: Option<String>,

    /// AWS region (default: AWS_DEFAULT_REGION or us-east-1)
    #[arg(long, env = "AWS_DEFAULT_REGION", default_value = "us-east-1")]
    region: String,

    /// AWS account ID (auto-detected if omitted)
    #[arg(long)]
    account: Option<String>,

    /// Comma-separated languages to run (default: python,go,java,typescript)
    #[arg(long, default_value = "python,go,java,typescript")]
    languages: String,

    /// Skip CDK deployment (assume stack is already deployed)
    #[arg(long)]
    skip_deploy: bool,

    /// Skip CDK destroy at the end
    #[arg(long)]
    skip_destroy: bool,

    /// Base directory for results (default: <projects_dir>run_results)
    #[arg(long)]
    results_dir: Option<PathBuf>,

    /// Do not delete temporary execution roles after each language run
    #[arg(long)]
    no_cleanup_roles: bool,

    /// Compute the minimal policy for Java after a successful run.
    /// Requires many validation runs (~50-100). Opt-in only.
    #[arg(long, default_value_t = false)]
    minimize_policy: bool,

    /// When --minimize-policy is set, skip runs that already have a
    /// minimal_policy.json instead of aborting with an error.
    #[arg(long, default_value_t = false)]
    skip_minimized: bool,

    /// Path to a candidate policy JSON file to use as the starting point
    /// for delta-debugging minimization (instead of the IAM Autopilot
    /// generated policy for Java).  Each Statement in the file becomes
    /// an individual policy document for the minimizer.
    ///
    /// Useful when autopilot cannot generate a policy or generates an
    /// incorrect one.  The file must be a valid IAM policy document with
    /// a top-level "Statement" array.
    #[arg(long)]
    candidate_policy: Option<PathBuf>,

    /// Include stdout/stderr in execution_log.json files.
    ///
    /// By default, execution logs only record the return code and success
    /// status to avoid leaking sensitive information (account IDs, resource
    /// names, etc.) that scripts may print.  Pass this flag during local
    /// debugging to capture full output.
    #[arg(long)]
    verbose_logs: bool,
}

// ---------------------------------------------------------------------------
// Main orchestration
// ---------------------------------------------------------------------------

async fn run_all(cli: &Cli) -> Result<()> {
    // ── Resolve the runs directory (--projects-dir or CWD) ───────────────────────
    let projects_dir = match &cli.projects_dir {
        Some(dir) => {
            let p = dir.canonicalize().with_context(|| {
                format!("--projects-dir {dir:?} does not exist or is not accessible")
            })?;
            if !p.is_dir() {
                bail!("--projects-dir {dir:?} is not a directory");
            }
            p
        }
        None => env::current_dir().context("Could not determine current directory")?,
    };

    // ── Resolve languages ───────────────────────────────────────────────────
    let all_configs = language_configs();
    let languages: Vec<&str> = cli
        .languages
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    for lang in &languages {
        if !all_configs.contains_key(lang) {
            bail!("Unknown language '{}'. Valid: {}", lang, {
                let mut keys: Vec<&str> = all_configs.keys().copied().collect();
                keys.sort_unstable();
                keys.join(", ")
            });
        }
    }

    // ── Results base directory ───────────────────────────────────────────────
    let results_base = cli
        .results_dir
        .clone()
        .unwrap_or_else(|| projects_dir.join("run_results"));
    let results_dir_name = results_base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // ── Discover run directories ─────────────────────────────────────────────
    let run_names: Vec<String> = if let Some(only) = &cli.only {
        vec![only.clone()]
    } else {
        let mut names: Vec<String> = fs::read_dir(&projects_dir)
            .context("Could not read iac-examples directory")?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() && name.starts_with("run_") && name != results_dir_name {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        if names.is_empty() {
            bail!("No run_*/ directories found in {projects_dir:?}");
        }
        names
    };

    // Validate all requested run directories exist before starting.
    for name in &run_names {
        let run_dir = projects_dir.join(name);
        if !run_dir.is_dir() {
            bail!("Run directory not found: {run_dir:?}");
        }
    }

    // ── AWS clients ─────────────────────────────────────────────────────────
    let sdk_config = aws_config::from_env()
        .region(Region::new(cli.region.clone()))
        .load()
        .await;
    let sts = StsClient::new(&sdk_config);
    let iam = IamClient::new(&sdk_config);

    // ── Resolve account ─────────────────────────────────────────────────────
    let account = if let Some(a) = &cli.account {
        a.clone()
    } else {
        info!("Auto-detecting AWS account ID ...");
        get_aws_account_id(&sts)
            .await
            .context("Could not auto-detect AWS account ID")?
    };

    // ── Resolve caller ARN (used as trust-policy principal) ─────────────────
    let caller_arn = get_caller_arn(&sts)
        .await
        .context("Could not resolve caller ARN for trust policy")?;
    debug!("   Caller ARN: {}", redact_arn(&caller_arn));

    let session_timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();

    println!("\n{}", "=".repeat(60));
    println!("  INTEGRATION-TEST RUNNER");
    println!("  Runs:      {}", run_names.join(", "));
    println!("  Languages: {}", languages.join(", "));
    println!("{}\n", "=".repeat(60));

    // ── Run each example in sequence ─────────────────────────────────────────
    let mut all_reports: Vec<RunReport> = Vec::new();
    for run_name in &run_names {
        let run_dir = projects_dir.join(run_name);
        let results_run_dir = results_base.join(&session_timestamp).join(run_name);
        fs::create_dir_all(&results_run_dir)
            .with_context(|| format!("Could not create results dir {results_run_dir:?}"))?;

        // ── Preflight: check for existing minimal_policy.json before doing any work ──
        if cli.minimize_policy {
            let existing = run_dir.join("minimal_policy.json");
            if existing.exists() {
                if cli.skip_minimized {
                    info!(
                        "[minimizer] Skipping '{}': minimal_policy.json already exists (--skip-minimized)",
                        run_name
                    );
                    continue;
                }
                bail!(
                    "[minimizer] Aborting: {existing:?} already exists in run directory '{run_name}'.\n\
                     Delete it first if you want to re-minimize, or pass --skip-minimized \
                     to skip runs that are already minimized."
                );
            }
        }

        println!("\n{}", "=".repeat(60));
        println!("  Starting run: {run_name}");
        println!("  Results:      {results_run_dir:?}");
        println!("{}\n", "=".repeat(60));

        let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let mut report = RunReport {
            run_name: run_name.clone(),
            timestamp: timestamp.clone(),
            region: cli.region.clone(),
            languages: languages
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            cdk_deploy: CdkStepResult::skipped(),
            language_results: HashMap::new(),
            cdk_destroy: CdkStepResult::skipped(),
            overall_success: false,
            start_time: chrono::Utc::now().to_rfc3339(),
            end_time: None,
        };

        // ── Step 0: Create deploy-role from deploy_policy.json ───────────────
        let deploy_policy_path = run_dir.join("deploy_policy.json");
        let deploy_role_info = if deploy_policy_path.exists() && !cli.skip_deploy {
            let data = fs::read_to_string(&deploy_policy_path)
                .with_context(|| format!("reading {deploy_policy_path:?}"))?;
            let policy: serde_json::Value = serde_json::from_str(&data)
                .with_context(|| format!("parsing {deploy_policy_path:?}"))?;
            let suffix = format!("deploy-{run_name}");
            match create_deploy_role(&iam, policy, &suffix, &account, &caller_arn).await {
                Ok(info) => {
                    debug!("[IAM] Created deploy role: {}", redact_arn(&info.role_arn));
                    Some(info)
                }
                Err(e) => {
                    error!("Failed to create deploy role for {}: {}", run_name, e);
                    report.end_time = Some(chrono::Utc::now().to_rfc3339());
                    save_report(&results_run_dir, &report);
                    all_reports.push(report);
                    continue;
                }
            }
        } else {
            if !deploy_policy_path.exists() && !cli.skip_deploy {
                warn!(
                    "No deploy_policy.json found for {} — deploying without a scoped role",
                    run_name
                );
            }
            None
        };
        let deploy_role_arn = deploy_role_info.as_ref().map(|r| r.role_arn.as_str());

        // ── Step 1: CDK Deploy ──────────────────────────────────────────────
        if cli.skip_deploy {
            info!("[SKIP] CDK deployment (--skip-deploy)");
            report.cdk_deploy = CdkStepResult::skipped();
        } else {
            let ok = cdk_deploy(&run_dir, deploy_role_arn, &cli.region, &sts).await?;
            report.cdk_deploy = CdkStepResult::done(ok);
            if !ok {
                error!(
                    "CDK deployment failed for {} -- skipping to next run",
                    run_name
                );
                // Cleanup deploy role before continuing
                if let Some(ref info) = deploy_role_info {
                    cleanup_execution_role(&iam, info).await;
                }
                report.end_time = Some(chrono::Utc::now().to_rfc3339());
                save_report(&results_run_dir, &report);
                all_reports.push(report);
                continue;
            }
        }

        // ── Step 2: Per-language pipeline ──────────────────────────────────
        let mut any_success = false;
        for lang in &languages {
            let lang_cfg = &all_configs[lang];
            let lang_results_dir = results_run_dir.join(lang);
            let result = run_language(
                lang,
                &run_dir,
                &lang_results_dir,
                &cli.region,
                &account,
                !cli.no_cleanup_roles,
                cli.verbose_logs,
                &iam,
                &sts,
                lang_cfg,
                &caller_arn,
            )
            .await;
            if result.success {
                any_success = true;
            }
            report.language_results.insert(lang.to_string(), result);
        }
        report.overall_success = any_success;

        // ── Step 2b: Policy minimization (opt-in, Java only) ───────────────
        if cli.minimize_policy {
            if cli.candidate_policy.is_some() {
                // When a candidate policy is provided, skip the Java success
                // check — the user is supplying the starting policy directly.
                run_minimizer(
                    &run_dir,
                    &results_run_dir,
                    &cli.region,
                    &account,
                    &iam,
                    &sts,
                    cli.candidate_policy.as_deref(),
                    &caller_arn,
                )
                .await;
            } else if let Some(java_result) = report.language_results.get("java") {
                if java_result.success {
                    run_minimizer(
                        &run_dir,
                        &results_run_dir,
                        &cli.region,
                        &account,
                        &iam,
                        &sts,
                        None,
                        &caller_arn,
                    )
                    .await;
                } else {
                    bail!("Cannot minimize policy: Java run did not succeed.");
                }
            } else {
                bail!(
                    "Cannot minimize policy: no Java result found. \
                     Ensure 'java' is included in the language list or provide --candidate-policy."
                );
            }
        }

        // ── Step 3: CDK Destroy ─────────────────────────────────────────────
        if cli.skip_destroy {
            info!("[SKIP] CDK destroy (--skip-destroy)");
            report.cdk_destroy = CdkStepResult::skipped();
        } else {
            let ok = cdk_destroy(&run_dir, deploy_role_arn, &cli.region, &sts).await?;
            report.cdk_destroy = CdkStepResult::done(ok);
        }

        // ── Step 3b: Cleanup deploy role ────────────────────────────────────
        if let Some(ref info) = deploy_role_info {
            if cli.no_cleanup_roles {
                info!(
                    "[SKIP] Deploy role cleanup (--no-cleanup-roles): {}",
                    info.role_name
                );
            } else {
                cleanup_execution_role(&iam, info).await;
            }
        }

        // ── Step 4: Per-run report ──────────────────────────────────────────
        report.end_time = Some(chrono::Utc::now().to_rfc3339());
        save_report(&results_run_dir, &report);
        print_final_report(&report);
        all_reports.push(report);
    }

    // ── Aggregate summary ───────────────────────────────────────────────────
    print_aggregate_report(&all_reports);

    let failed_runs: Vec<&RunReport> = all_reports.iter().filter(|r| !r.overall_success).collect();

    if failed_runs.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} of {} run(s) failed",
            failed_runs.len(),
            all_reports.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Candidate policy loader
// ---------------------------------------------------------------------------

/// Load a candidate IAM policy document from *path* and split each `Statement`
/// into its own individual policy document (one per statement).
///
/// This produces the same shape as `generate_individual_policies()` — a `Vec`
/// of single-statement policy documents — so the minimizer can remove them
/// independently via delta-debugging.
fn load_candidate_policy(path: &Path) -> Option<Vec<serde_json::Value>> {
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) => {
            error!(
                "[minimizer] Could not read candidate policy {:?}: {}",
                path, e
            );
            return None;
        }
    };

    let doc: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            error!(
                "[minimizer] Could not parse candidate policy {:?}: {}",
                path, e
            );
            return None;
        }
    };

    let statements = if let Some(stmts) = doc.get("Statement").and_then(|s| s.as_array()) {
        stmts.clone()
    } else {
        error!(
            "[minimizer] Candidate policy {:?} has no top-level \"Statement\" array",
            path
        );
        return None;
    };

    if statements.is_empty() {
        error!(
            "[minimizer] Candidate policy {:?} has an empty Statement array",
            path
        );
        return None;
    }

    // Wrap each statement in its own policy document.
    let individual: Vec<serde_json::Value> = statements
        .into_iter()
        .map(|stmt| {
            serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [stmt],
            })
        })
        .collect();

    info!(
        "[minimizer] Loaded {} individual policies from candidate {:?}",
        individual.len(),
        path
    );
    Some(individual)
}

// ---------------------------------------------------------------------------
// Policy minimizer wiring (runner binary only)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_minimizer(
    run_dir: &Path,
    results_run_dir: &Path,
    region: &str,
    account: &str,
    iam: &IamClient,
    sts: &StsClient,
    candidate_policy_path: Option<&Path>,
    caller_arn: &str,
) {
    let run_dir_minimal_path = run_dir.join("minimal_policy.json");

    // Step 1: Obtain individual policies — either from a candidate file or
    //         by running iam-policy-autopilot on the Java script.
    let individual_policies = if let Some(path) = candidate_policy_path {
        // Load the candidate policy and split each Statement into its own
        // individual policy document (one per statement) so the minimizer
        // can remove them independently.
        info!("[minimizer] Loading candidate policy from {:?}", path);
        if let Some(p) = load_candidate_policy(path) {
            p
        } else {
            error!(
                "[minimizer] Failed to load candidate policy from {:?}",
                path
            );
            return;
        }
    } else {
        let java_script = run_dir.join("java").join("Script.java");
        if let Some(p) = generate_individual_policies(&java_script, region, account) {
            p
        } else {
            error!("[minimizer] Failed to generate individual policies");
            return;
        }
    };

    // Step 2: Build the run_fn closure.
    let lang_configs = language_configs();
    let lang_cfg = lang_configs["java"].clone();
    let run_dir_clone = run_dir.to_path_buf();
    let results_minimizer_dir = results_run_dir.join("minimizer_runs");
    let region_str = region.to_string();
    let account_str = account.to_string();
    let iam_clone = iam.clone();
    let sts_clone = sts.clone();
    let run_counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    let caller_arn_str = caller_arn.to_string();
    let run_fn = {
        let run_counter = run_counter.clone();
        move |policy_docs: Vec<serde_json::Value>| {
            let run_dir = run_dir_clone.clone();
            let results_dir = results_minimizer_dir.clone();
            let region = region_str.clone();
            let account = account_str.clone();
            let iam = iam_clone.clone();
            let sts = sts_clone.clone();
            let lang_cfg = lang_cfg.clone();
            let caller_arn = caller_arn_str.clone();
            let n = run_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                let attempt_dir = results_dir.join(format!("run_{n:04}"));
                std::fs::create_dir_all(&attempt_dir).ok();
                // Save the policy documents used for this attempt.
                if let Ok(policy_json) =
                    serde_json::to_string_pretty(&serde_json::Value::Array(policy_docs.clone()))
                {
                    let policy_path = attempt_dir.join("policies.json");
                    if let Err(e) = std::fs::write(&policy_path, &policy_json) {
                        warn!(
                            "[minimizer] Failed to write policies.json to {:?}: {}",
                            policy_path, e
                        );
                    }
                }
                let summary = run_language_with_policies(
                    "java",
                    &run_dir,
                    &attempt_dir,
                    policy_docs,
                    &region,
                    &account,
                    false, // always clean up roles during minimization
                    false, // never write verbose logs during minimization
                    &iam,
                    &sts,
                    &lang_cfg,
                    &caller_arn,
                )
                .await;
                // Treat any non-zero exit as insufficient permissions (Ok(false)).
                // The minimizer only runs after the full policy set has been verified
                // to pass, so any failure during minimization is assumed to be an
                // IAM permission error rather than a deployment or script bug.
                Ok(summary.success)
            }
        }
    };

    // Step 3: Run the minimizer.
    info!("[minimizer] Starting policy minimization ...");
    let result = runner::minimizer::minimize_policy(individual_policies, run_fn).await;

    info!(
        "[minimizer] Done: {} actions removed, {} runs performed",
        result.actions_removed, result.runs_performed,
    );

    // Step 5: Save minimal_policy.json to the results run directory AND the source run directory.
    fs::create_dir_all(results_run_dir).ok();

    let minimal_json = match serde_json::to_string_pretty(&result.minimal_policy) {
        Ok(json) => json,
        Err(e) => {
            error!("[minimizer] Failed to serialize minimal_policy.json: {}", e);
            return;
        }
    };

    // 5a: results_run_dir copy (timestamped results tree)
    let minimal_path = results_run_dir.join("minimal_policy.json");
    match fs::write(&minimal_path, &minimal_json) {
        Ok(()) => info!(
            "[minimizer] Saved minimal_policy.json to {:?}",
            minimal_path
        ),
        Err(e) => error!(
            "[minimizer] Failed to write minimal_policy.json to results dir: {}",
            e
        ),
    }

    // 5b: source run_dir copy (lives alongside Script.java / cdk/)
    match fs::write(&run_dir_minimal_path, &minimal_json) {
        Ok(()) => info!(
            "[minimizer] Saved minimal_policy.json to {:?}",
            run_dir_minimal_path
        ),
        Err(e) => error!(
            "[minimizer] Failed to write minimal_policy.json to run dir: {}",
            e
        ),
    }

    // Step 6: Save minimization_result.json to the run root directory.
    let result_path = results_run_dir.join("minimization_result.json");
    match serde_json::to_string_pretty(&result) {
        Ok(json) => {
            let _ = fs::write(&result_path, json);
            info!(
                "[minimizer] Saved minimization_result.json to {:?}",
                result_path
            );
        }
        Err(e) => error!(
            "[minimizer] Failed to serialize minimization_result.json: {}",
            e
        ),
    }
}

// ---------------------------------------------------------------------------
// Report helpers
// ---------------------------------------------------------------------------

fn print_aggregate_report(reports: &[RunReport]) {
    println!("\n{}", "#".repeat(60));
    println!("  AGGREGATE TEST RESULTS");
    println!("{}", "#".repeat(60));
    println!();

    let total = reports.len();
    let passed: Vec<&RunReport> = reports.iter().filter(|r| r.overall_success).collect();
    let failed: Vec<&RunReport> = reports.iter().filter(|r| !r.overall_success).collect();

    println!(
        "  Total: {}  |  Passed: {}  |  Failed: {}",
        total,
        passed.len(),
        failed.len()
    );
    println!();

    if !failed.is_empty() {
        println!("  FAILED RUNS:");
        println!("  {}", "-".repeat(56));
        for report in &failed {
            println!("  ❌ {}", report.run_name);

            // CDK deploy failure
            if !report.cdk_deploy.is_skipped() && !report.cdk_deploy.is_ok() {
                println!("       CDK Deploy: FAILED");
            }

            // Language failures
            let mut langs: Vec<&String> = report.language_results.keys().collect();
            langs.sort();
            for lang in langs {
                let result = &report.language_results[lang];
                if !result.success {
                    let reason = result.failure_reason.as_deref().unwrap_or("unknown reason");
                    println!("       [FAIL] {lang} -- {reason}");
                }
            }

            // CDK destroy failure
            if !report.cdk_destroy.is_skipped() && !report.cdk_destroy.is_ok() {
                println!("       CDK Destroy: FAILED");
            }
        }
        println!();
    }

    if !passed.is_empty() {
        println!("  PASSED RUNS:");
        println!("  {}", "-".repeat(56));
        for report in &passed {
            println!("  ✅ {}", report.run_name);
        }
        println!();
    }

    println!("{}\n", "#".repeat(60));
}

fn save_report(results_dir: &std::path::Path, report: &RunReport) {
    let path = results_dir.join("run_report.json");
    match serde_json::to_string_pretty(report) {
        Ok(json_str) => {
            let _ = fs::write(&path, json_str);
            info!("[saved] Final report: {:?}", path);
        }
        Err(e) => error!("Failed to serialize run report: {}", e),
    }
}

fn print_final_report(report: &RunReport) {
    println!("\n{}", "=".repeat(60));
    println!("  INTEGRATION-TEST RUNNER -- FINAL REPORT");
    println!("{}", "=".repeat(60));
    println!("  Run:       {}", report.run_name);
    println!("  Timestamp: {}", report.timestamp);
    println!("  Region:    {}", report.region);
    println!();

    if report.cdk_deploy.is_skipped() {
        println!("  CDK Deploy:   SKIPPED");
    } else {
        println!(
            "  CDK Deploy:   {}",
            if report.cdk_deploy.is_ok() {
                "OK"
            } else {
                "FAILED"
            }
        );
    }

    println!();
    println!("  Language Results:");
    let mut langs: Vec<&String> = report.language_results.keys().collect();
    langs.sort();
    for lang in langs {
        let result = &report.language_results[lang];
        let status = if result.success { "PASS" } else { "FAIL" };
        let reason = result
            .failure_reason
            .as_deref()
            .map(|r| format!(" -- {r}"))
            .unwrap_or_default();
        println!("    [{status}] {lang}{reason}");
    }

    println!();
    if report.cdk_destroy.is_skipped() {
        println!("  CDK Destroy:  SKIPPED");
    } else {
        println!(
            "  CDK Destroy:  {}",
            if report.cdk_destroy.is_ok() {
                "OK"
            } else {
                "FAILED"
            }
        );
    }

    println!();
    println!("{}\n", "=".repeat(60));
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    if let Err(e) = run_all(&cli).await {
        eprintln!("[ERROR] {e:#}");
        std::process::exit(1);
    }
}
