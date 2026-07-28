//! `iac-benchmarker` — minimum managed-policy finder and over-permission analyser.
//!
//! Single-run workflow (--run-dir):
//!   0. Bootstrap (AWS clients, cache, catalogue, managed policies, index)
//!   1. Determine required actions from the minimal_policy.json in the run directory
//!   2. Pre-filter candidate managed policies
//!   3. Set-cover (greedy, exact, or min-actions)
//!   3.5. CDK deploy (stack must be up for all live-execution phases below)
//!   4. Empirical validation via live AWS execution (optional)
//!   4b. Validate minimal_policy.json via live execution
//!   5. Action-count comparison (managed vs minimal policy)
//!   5b. Per-language autopilot overpermissioning (generate policies for all languages,
//!       compare concrete action counts against the minimal policy,
//!       run Access Analyzer ValidatePolicy on each generated policy)
//!   5c. Per-language LLM overpermissioning (call Bedrock for each language,
//!       validate with live execution, run Access Analyzer ValidatePolicy)
//!   5d. Per-language context-filled LLM overpermissioning
//!   4.5. CDK destroy (runs AFTER 5c/5d so the stack is available for LLM validation)
//!   6. Write `benchmark_managed_policies.json` report
//!
//! Batch workflow (--runs-dir):
//!   Bootstraps AWS clients, catalogue, managed policies, and policy index ONCE,
//!   then iterates over all `run_*/` subdirectories and runs the full benchmark
//!   pipeline (phases 1-6) for each one.  Writes per-run `benchmark_managed_policies.json`
//!   files and a single `aggregate_report.json` to the output directory.

use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_sdk_accessanalyzer::Client as AaClient;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_iam::Client as IamClient;
use aws_sdk_sts::Client as StsClient;
use chrono::Utc;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use iac_runner::get_aws_account_id;
use iac_benchmarker::{
    managed_policies::load_or_fetch_managed_policies,
    pipeline::{build_aggregate_report, run_single, PipelineConfig, SharedContext},
    policy_index::load_or_build_index,
    printing::print_aggregate_summary,
    service_ref::load_or_fetch_catalogue,
    llm_policy::{default_bedrock_model_id, ResourcePromptStrategy},
};
use iac_benchmarker::aggregate::RunSummary;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "iac-benchmarker",
    about = "Find the minimum set of AWS managed policies that covers a run's required IAM actions.\n\
             Use --run-dir for a single run, or --runs-dir to run the full benchmark pipeline\n\
             across many run_* subdirectories and produce an aggregate_report.json."
)]
struct Cli {
    /// Path to a single run directory to benchmark (e.g. integration-tests/projects/run_001).
    /// Must contain minimal_policy.json.
    /// Mutually exclusive with --runs-dir.
    #[arg(long, conflicts_with = "runs_dir")]
    run_dir: Option<PathBuf>,

    /// Path to a directory containing multiple run_* subdirectories.
    /// Runs the full benchmark pipeline (AWS calls + set-cover + autopilot) on each
    /// subdirectory and writes aggregate_report.json to --output-dir.
    /// Mutually exclusive with --run-dir.
    #[arg(long, conflicts_with = "run_dir")]
    runs_dir: Option<PathBuf>,

    /// Language to use for validation run [default: java].
    #[arg(long, default_value = "java")]
    language: String,

    /// Comma-separated list of languages to generate autopilot policies for
    /// when computing per-language overpermissioning [default: python,go,java,typescript].
    #[arg(long, default_value = "python,go,java,typescript")]
    languages: String,

    /// Path to the iam-policy-autopilot binary used to generate per-language policies.
    #[arg(long, default_value = "iam-policy-autopilot")]
    autopilot_binary: String,

    /// AWS region [default: us-east-1 / AWS_DEFAULT_REGION env var].
    #[arg(long, env = "AWS_DEFAULT_REGION", default_value = "us-east-1")]
    region: String,

    /// AWS account ID (auto-detected via STS if omitted).
    #[arg(long)]
    account: Option<String>,

    /// Skip the live AWS execution validation (Phase 4).
    #[arg(long)]
    skip_validation: bool,

    /// Do not delete temporary IAM roles after validation.
    #[arg(long)]
    no_cleanup_roles: bool,

    /// Where to write benchmark_managed_policies.json and validation logs.
    /// [default: ./benchmarker_results/<timestamp>/<run>/]
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Directory for persistent cache files.
    /// [default: ~/.iac-benchmarker/]
    #[arg(long)]
    cache_dir: Option<PathBuf>,

    /// Force rebuild of policy_index.json even if cache is fresh.
    #[arg(long)]
    rebuild_index: bool,

    /// Max policy-set expansion retries on validation failure [default: 3].
    #[arg(long, default_value_t = 3)]
    max_validation_retries: usize,

    /// Set-cover strategy to use:
    ///   greedy      — greedy set-cover
    ///   exact       — branch-and-bound minimising number of policies
    ///   min-actions — branch-and-bound minimising total concrete action count (default)
    #[arg(long, default_value = "min-actions")]
    cover_mode: String,

    /// IAM role ARN for CDK operations.
    #[arg(long)]
    cdk_role_arn: Option<String>,

    /// Skip CDK deployment (assume stack is already deployed).
    #[arg(long)]
    skip_deploy: bool,

    /// Skip CDK destroy after validation.
    #[arg(long)]
    skip_destroy: bool,

    /// Skip LLM policy generation via Bedrock (Phase 5c).
    #[arg(long)]
    skip_llm: bool,

    /// Number of times to repeat LLM policy generation for each language
    /// **per experiment** (script-only, script+context).
    /// Each repetition generates a fresh policy and optionally validates it.
    /// Results from all repetitions are recorded; the median is used as the
    /// representative value.  [default: 5]
    #[arg(long, default_value_t = 5)]
    llm_repetitions: usize,

    /// Skip iamfast static-analysis policy generation (Phase 5e).
    #[arg(long)]
    skip_iamfast: bool,

    /// Comma-separated list of languages to run iamfast on.
    /// iamfast only supports Go and Java, so Python and TypeScript are excluded
    /// by default [default: go,java].
    #[arg(long, default_value = "go,java")]
    iamfast_languages: String,

    /// Path to the iamfast binary (or npx-resolvable name).
    /// iamfast performs static analysis on source code to generate IAM policies.
    #[arg(long, default_value = "iamfast")]
    iamfast_binary: String,

    /// AWS Bedrock region for LLM calls (defaults to --region).
    /// Some models are only available in specific regions (e.g. us-east-1).
    #[arg(long)]
    bedrock_region: Option<String>,

    /// Bedrock model ID or inference profile ARN to use for LLM policy generation.
    /// Defaults to an inference profile ARN constructed from --bedrock-region and the
    /// resolved AWS account ID.
    #[arg(long)]
    bedrock_model_id: Option<String>,

    /// Skip one or more run directories by ID (e.g. --skip run_001-3478634b).
    /// Can be specified multiple times or as a space-separated list.
    /// Only applies in --runs-dir (batch) mode.
    #[arg(long = "skip", value_name = "RUN_ID", num_args = 1..)]
    skip_runs: Vec<String>,

    /// Path to the directory containing per-language scenario files used for
    /// the context-filling LLM experiment.  Expected layout:
    ///   <dir>/python/   — Python scenario files
    ///   <dir>/go/       — Go scenario files
    ///   <dir>/java/     — Java scenario files
    ///   <dir>/typescript/ — TypeScript scenario files
    /// When provided, LLM generation runs twice per language: once with the
    /// simple prompt and once with all scenario files pre-loaded as context
    /// messages via the Bedrock converse API.
    /// Defaults to `iac-benchmarker/scenarios/` relative to the binary.
    #[arg(long)]
    context_scenarios_dir: Option<PathBuf>,

    /// Resource prompt strategies to benchmark (comma-separated).
    /// Options: bare, wildcards, resource-star.
    /// Each strategy is crossed with each context scenario to form the 3×3 matrix.
    #[arg(long, default_value = "bare,wildcards,resource-star")]
    resource_strategies: String,

    /// Re-aggregate from previously written run_summary.json files.
    /// Pass the path to a results directory containing run_*/run_summary.json files.
    /// Example: benchmarker_results/20260417T111513/aggregate/
    /// Mutually exclusive with --run-dir and --runs-dir.
    #[arg(long, conflicts_with_all = ["run_dir", "runs_dir"])]
    reaggregate: Option<PathBuf>,

    /// Where to write the new aggregate_report.json when using --reaggregate.
    /// [default: <reaggregate>/aggregate_report.json]
    #[arg(long, requires = "reaggregate")]
    reaggregate_output: Option<PathBuf>,
}

impl Cli {
    /// Convert CLI flags into a [`PipelineConfig`] for the pipeline module.
    fn to_pipeline_config(&self) -> PipelineConfig {
        let resource_prompt_strategies: Vec<ResourcePromptStrategy> = self
            .resource_strategies
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                match ResourcePromptStrategy::from_str_loose(s) {
                    Some(strategy) => Some(strategy),
                    None => {
                        warn!("Unknown resource strategy '{}' — ignoring", s);
                        None
                    }
                }
            })
            .collect();

        PipelineConfig {
            language: self.language.clone(),
            languages: self
                .languages
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            iamfast_languages: self
                .iamfast_languages
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            autopilot_binary: self.autopilot_binary.clone(),
            skip_validation: self.skip_validation,
            no_cleanup_roles: self.no_cleanup_roles,
            cover_mode: self.cover_mode.clone(),
            cdk_role_arn: self.cdk_role_arn.clone(),
            skip_deploy: self.skip_deploy,
            skip_destroy: self.skip_destroy,
            skip_llm: self.skip_llm,
            skip_iamfast: self.skip_iamfast,
            iamfast_binary: self.iamfast_binary.clone(),
            llm_repetitions: self.llm_repetitions,
            resource_prompt_strategies,
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // --- Reaggregate mode: no AWS, no output dir, no shutdown handler ------
    if let Some(ref results_dir) = cli.reaggregate {
        // Lightweight logging to stderr only (no log file needed).
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"));
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_writer(std::io::stderr))
            .init();

        return reaggregate(results_dir.clone(), cli.reaggregate_output.clone());
    }

    // --- Compute the output directory early so we can log to it. -----------
    let output_dir = resolve_output_dir(&cli)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir {:?}", output_dir))?;

    // --- Dual logging: stderr (terminal) + file (benchmarker.log) ----------
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_appender = tracing_appender::rolling::never(&output_dir, "benchmarker.log");
    let (non_blocking_file, _file_guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(
            fmt::layer()
                .with_writer(non_blocking_file)
                .with_ansi(false),
        )
        .init();

    info!("Output dir: {:?}", output_dir);
    info!("Log file:   {:?}", output_dir.join("benchmarker.log"));

    // --- Graceful shutdown: intercept Ctrl+C (SIGINT) ----------------------
    // First Ctrl+C:  cancel the token → pipeline skips remaining validation
    //                phases but still runs CDK destroy.
    // Second Ctrl+C: force-exit immediately.
    let shutdown = CancellationToken::new();
    let shutdown_for_handler = shutdown.clone();
    tokio::spawn(async move {
        // First SIGINT
        if tokio::signal::ctrl_c().await.is_ok() {
            warn!("SIGINT received — cancelling validation phases (CDK destroy will still run)");
            shutdown_for_handler.cancel();
        }
        // Second SIGINT → hard exit
        if tokio::signal::ctrl_c().await.is_ok() {
            error!("Second SIGINT received — forcing immediate exit");
            std::process::exit(130);
        }
    });

    if cli.runs_dir.is_some() {
        run_batch(cli, shutdown, &output_dir).await
    } else if cli.run_dir.is_some() {
        run(cli, shutdown, &output_dir).await
    } else {
        anyhow::bail!("Either --run-dir, --runs-dir, or --reaggregate must be specified.");
    }
}

/// Compute the top-level output directory from CLI flags.
///
/// In batch mode the output dir is `<output_dir>/` (or `benchmarker_results/<ts>/aggregate/`).
/// In single-run mode it is `<output_dir>/` (or `benchmarker_results/<ts>/<run_name>/`).
fn resolve_output_dir(cli: &Cli) -> Result<PathBuf> {
    if let Some(ref dir) = cli.output_dir {
        return Ok(dir.clone());
    }
    let ts = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    if cli.runs_dir.is_some() {
        Ok(PathBuf::from("benchmarker_results").join(&ts).join("aggregate"))
    } else if let Some(ref run_dir) = cli.run_dir {
        let run_name = run_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        Ok(PathBuf::from("benchmarker_results").join(&ts).join(run_name))
    } else {
        // Will be caught later by the main dispatch.
        Ok(PathBuf::from("benchmarker_results").join(&ts))
    }
}

// ---------------------------------------------------------------------------
// Bootstrap: build shared AWS clients, catalogue, managed policies, index
// ---------------------------------------------------------------------------

async fn bootstrap(cli: &Cli, shutdown: CancellationToken) -> Result<SharedContext> {
    info!("=== Bootstrap: AWS clients, catalogue, managed policies, index ===");

    let aws_cfg = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(cli.region.clone()))
        .load()
        .await;
    let sts = StsClient::new(&aws_cfg);
    let iam = IamClient::new(&aws_cfg);
    let aa = AaClient::new(&aws_cfg);

    // Bedrock may use a different region (some models are only in us-east-1).
    let bedrock_region = cli
        .bedrock_region
        .clone()
        .unwrap_or_else(|| cli.region.clone());
    let bedrock_cfg = if bedrock_region == cli.region {
        aws_cfg.clone()
    } else {
        aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(bedrock_region.clone()))
            .load()
            .await
    };
    let bedrock = BedrockClient::new(&bedrock_cfg);
    info!("Bedrock region: {}", bedrock_region);

    let account = match cli.account {
        Some(ref a) => a.clone(),
        None => {
            info!("Resolving AWS account ID via STS ...");
            get_aws_account_id(&sts)
                .await
                .context("Failed to resolve AWS account ID")?
        }
    };
    info!("Account: {}", account);

    let cache_dir = cli.cache_dir.clone().unwrap_or_else(|| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".iac-benchmarker")
    });
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create cache dir {:?}", cache_dir))?;
    info!("Cache dir: {:?}", cache_dir);

    info!("Loading service catalogue ...");
    let catalogue = load_or_fetch_catalogue(&cache_dir.join("service_reference_cache.json"))
        .await
        .context("Failed to load service catalogue")?;
    info!("Service catalogue: {} services", catalogue.len());

    info!("Loading managed policies ...");
    let managed_policies =
        load_or_fetch_managed_policies(&iam, &cache_dir.join("managed_policy_cache.json"))
            .await
            .context("Failed to load managed policies")?;
    info!("Managed policies: {}", managed_policies.len());

    let index_path = cache_dir.join("policy_index.json");
    let policy_cache_path = cache_dir.join("managed_policy_cache.json");

    if cli.rebuild_index && index_path.exists() {
        info!("--rebuild-index: removing existing index ...");
        fs::remove_file(&index_path).ok();
    }

    info!("Loading policy index ...");
    let index = load_or_build_index(&index_path, &policy_cache_path, &managed_policies, &catalogue)
        .context("Failed to load/build policy index")?;
    info!(
        "Policy index: {} policies, {} service prefixes",
        index.policy_arn_to_name.len(),
        index.service_prefix_to_policy_arns.len()
    );

    let bedrock_model_id = cli.bedrock_model_id.clone().unwrap_or_else(|| {
        let id = default_bedrock_model_id(&bedrock_region, &account);
        info!("Bedrock model ID (auto): {}", id);
        id
    });
    if cli.bedrock_model_id.is_some() {
        info!("Bedrock model ID (override): {}", bedrock_model_id);
    }

    // Resolve context scenarios directory.
    let context_scenarios_dir = cli.context_scenarios_dir.clone().or_else(|| {
        let default = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("scenarios")));
        default.filter(|p| p.is_dir())
    });
    if let Some(ref d) = context_scenarios_dir {
        info!("Context scenarios dir: {:?}", d);
    } else {
        info!("Context scenarios dir: not set — context-filling LLM experiment will be skipped");
    }

    Ok(SharedContext {
        iam,
        sts,
        bedrock,
        aa,
        account,
        region: cli.region.clone(),
        bedrock_model_id,
        catalogue,
        index,
        cache_dir,
        context_scenarios_dir,
        shutdown,
    })
}

// ---------------------------------------------------------------------------
// Batch mode
// ---------------------------------------------------------------------------

/// Return `true` if `name` is a benchmark run directory.
///
/// Matches `run_<digits>` with an optional `-<suffix>` (e.g. `run_001` or
/// `run_001-3478634b`), but not names like `run_results`.
fn is_run_dir_name(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("run_") {
        // The numeric part is everything up to the first '-' (if any).
        let numeric = rest.split('-').next().unwrap_or("");
        !numeric.is_empty() && numeric.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

async fn run_batch(cli: Cli, shutdown: CancellationToken, output_dir: &PathBuf) -> Result<()> {
    let runs_dir = cli.runs_dir.as_ref().unwrap().clone();
    if !runs_dir.is_dir() {
        anyhow::bail!("--runs-dir {:?} is not a directory", runs_dir);
    }

    let batch_output_dir = output_dir.clone();

    let ctx = bootstrap(&cli, shutdown).await?;
    let cfg = cli.to_pipeline_config();

    info!("Scanning {:?} for run_* subdirectories ...", runs_dir);

    let skip_set: HashSet<String> = cli.skip_runs.iter().cloned().collect();

    let mut run_dirs: Vec<PathBuf> = fs::read_dir(&runs_dir)
        .with_context(|| format!("Cannot read {:?}", runs_dir))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            p.is_dir() && is_run_dir_name(name) && !skip_set.contains(name)
        })
        .collect();
    run_dirs.sort();

    if !skip_set.is_empty() {
        info!("Skipping {} run(s): {:?}", skip_set.len(), skip_set);
    }
    info!("Found {} run_* directories (after skip filter)", run_dirs.len());

    let total_runs = run_dirs.len();
    let mut run_summaries: Vec<RunSummary> = Vec::new();

    for run_dir in &run_dirs {
        // Check for graceful shutdown before starting a new run.
        if ctx.shutdown.is_cancelled() {
            warn!("Shutdown requested — skipping remaining {} run(s)",
                  run_dirs.len() - run_summaries.len());
            break;
        }

        let run_name = run_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let run_output_dir = batch_output_dir.join(&run_name);
        fs::create_dir_all(&run_output_dir)
            .with_context(|| format!("Failed to create run output dir {:?}", run_output_dir))?;

        info!("[{}] Running full benchmark pipeline ...", run_name);

        match run_single(&ctx, &cfg, run_dir, &run_output_dir).await {
            Ok(summary) => {
                info!("[{}] Benchmark complete", run_name);

                // Persist RunSummary so it can be re-aggregated later.
                let summary_path = run_output_dir.join("run_summary.json");
                if let Ok(json) = serde_json::to_string_pretty(&summary) {
                    let _ = fs::write(&summary_path, json);
                    info!("[{}] Saved run_summary.json", run_name);
                }

                run_summaries.push(summary);
            }
            Err(e) => {
                warn!("[{}] Benchmark failed: {:#}", run_name, e);
            }
        }
    }

    let successful_runs = run_summaries.len();
    info!(
        "Completed {}/{} runs successfully",
        successful_runs, total_runs
    );

    if run_summaries.is_empty() {
        anyhow::bail!(
            "All benchmark runs failed under {:?}. Check logs above for details.",
            runs_dir
        );
    }

    let aggregate = build_aggregate_report(&runs_dir, total_runs, successful_runs, run_summaries)?;

    let report_path = batch_output_dir.join("aggregate_report.json");
    let json = serde_json::to_string_pretty(&aggregate)
        .context("Failed to serialise aggregate report")?;
    fs::write(&report_path, &json)
        .with_context(|| format!("Failed to write {:?}", report_path))?;
    info!("[saved] {:?}", report_path);

    print_aggregate_summary(&aggregate);
    Ok(())
}

// ---------------------------------------------------------------------------
// Single-run mode
// ---------------------------------------------------------------------------

async fn run(cli: Cli, shutdown: CancellationToken, output_dir: &PathBuf) -> Result<()> {
    let run_dir = cli
        .run_dir
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--run-dir is required in single-run mode"))?;
    if !run_dir.is_dir() {
        anyhow::bail!("Run directory not found: {:?}", run_dir);
    }
    let run_name = run_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    info!("Run directory: {:?} (name: {})", run_dir, run_name);

    let ctx = bootstrap(&cli, shutdown).await?;
    let cfg = cli.to_pipeline_config();
    let summary = run_single(&ctx, &cfg, &run_dir, output_dir).await?;

    // Persist RunSummary so it can be re-aggregated later.
    let summary_path = output_dir.join("run_summary.json");
    if let Ok(json) = serde_json::to_string_pretty(&summary) {
        let _ = fs::write(&summary_path, json);
        info!("Saved run_summary.json");
    }

    info!(
        "Run complete: minimal={} managed={} ratio={:.2}",
        summary.minimal_concrete_actions,
        summary.managed_policy_concrete_actions,
        summary.over_permission_ratio_managed_vs_minimal
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Reaggregate mode
// ---------------------------------------------------------------------------

/// Re-aggregate from previously written `run_summary.json` files.
///
/// Scans `results_dir` for subdirectories containing `run_summary.json`,
/// deserializes each into a [`RunSummary`], and produces a fresh
/// `aggregate_report.json` using the same [`build_aggregate_report()`]
/// function used during a normal batch run.
///
/// Requires **no AWS clients** — just pure deserialization and statistics.
fn reaggregate(results_dir: PathBuf, output: Option<PathBuf>) -> Result<()> {
    let mut run_summaries: Vec<RunSummary> = Vec::new();

    for entry in fs::read_dir(&results_dir)
        .with_context(|| format!("Cannot read results dir {:?}", results_dir))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let summary_path = path.join("run_summary.json");
        if !summary_path.exists() {
            warn!("No run_summary.json in {:?} — skipping", path);
            continue;
        }

        let json = fs::read_to_string(&summary_path)
            .with_context(|| format!("Failed to read {:?}", summary_path))?;
        let summary: RunSummary = serde_json::from_str(&json)
            .with_context(|| format!("Failed to parse {:?}", summary_path))?;
        info!("Loaded: {}", summary.run_name);
        run_summaries.push(summary);
    }

    if run_summaries.is_empty() {
        anyhow::bail!(
            "No run_summary.json files found in {:?}. \
             Ensure the directory contains run_*/run_summary.json files.",
            results_dir
        );
    }

    run_summaries.sort_by(|a, b| a.run_name.cmp(&b.run_name));
    let total = run_summaries.len();

    info!("Loaded {} run summaries — building aggregate ...", total);

    let aggregate = build_aggregate_report(&results_dir, total, total, run_summaries)?;

    let output_path = output.unwrap_or_else(|| results_dir.join("aggregate_report.json"));
    let json = serde_json::to_string_pretty(&aggregate)
        .context("Failed to serialise aggregate report")?;
    fs::write(&output_path, &json)
        .with_context(|| format!("Failed to write {:?}", output_path))?;
    info!("Wrote {:?}", output_path);

    print_aggregate_summary(&aggregate);
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::is_run_dir_name;
    use rstest::rstest;

    #[rstest]
    // Open-source project layout: run_<digits>, no hash suffix.
    #[case::plain("run_001", true)]
    #[case::plain_two_digit("run_010", true)]
    // Internal layout: run_<digits>-<hash> suffix.
    #[case::hashed("run_001-3478634b", true)]
    // Non-run entries that share the directory.
    #[case::readme("README.md", false)]
    #[case::scan_script("scan_cdk_security.sh", false)]
    #[case::run_results("run_results", false)]
    #[case::no_digits("run_abc", false)]
    #[case::bare_prefix("run_", false)]
    #[case::unrelated("projects", false)]
    fn matches_run_directories(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(is_run_dir_name(name), expected, "name={name:?}");
    }
}
