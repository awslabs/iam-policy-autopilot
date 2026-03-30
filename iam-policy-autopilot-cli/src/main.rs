//! IAM Policy Autopilot CLI
//!
//! This is the main entry point for the iam-policy-autopilot command-line tool.
//!
//! # Exit Codes
//!
//! The CLI uses the `ExitCode` enum which maps to the following exit codes:
//!
//! - `ExitCode::Success` (0): Operation completed successfully
//! - `ExitCode::Duplicate` (1): Duplicate statement - permission already exists
//! - `ExitCode::Error` (2): User refused, validation failed, non-interactive environment,
//!   or manual action required
//!
//! These exit codes are used consistently throughout the CLI to allow shell scripts
//! and automation tools to distinguish between different failure modes.
//!
//! See `types::ExitCode` for the enum definition.

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iam_policy_autopilot_common::telemetry::{
    self, TelemetryChoice, ToTelemetryEvent, TelemetryEventDerive,
};
use iam_policy_autopilot_policy_generation::api::model::{
    AwsContext, ExtractSdkCallsConfig, GeneratePolicyConfig,
};
use iam_policy_autopilot_policy_generation::api::{extract_sdk_calls, generate_policies};
use iam_policy_autopilot_policy_generation::extraction::SdkMethodCall;
use iam_policy_autopilot_tools::PolicyUploader;
use log::{debug, info, trace};

mod commands;
mod output;
mod types;

use iam_policy_autopilot_mcp_server::{start_mcp_server, McpTransport, DEFAULT_BIND_ADDRESS};
use types::ExitCode;

use crate::commands::print_version_info;

/// Default port for mcp server for Http Transport
static MCP_HTTP_DEFAULT_PORT: u16 = 8001;

/// Shared CLI configuration for both subcommands
#[derive(Debug, Clone)]
struct SharedConfig {
    /// Source files to analyze
    source_files: Vec<PathBuf>,
    /// Enable pretty JSON output formatting
    pretty: bool,
    /// Override programming language detection
    language: Option<String>,
    /// Output full ExtractedMethods instead of simplified operations (extract-sdk-calls only)
    full_output: bool,
    /// Optional service hints for filtering
    service_hints: Option<Vec<String>>,
}

impl SharedConfig {
    /// Validate the configuration
    fn validate(&self) -> Result<()> {
        // Check that all source files exist
        for file in &self.source_files {
            if !file.exists() {
                anyhow::bail!("Source file does not exist: {}", file.display());
            }
            if !file.is_file() {
                anyhow::bail!("Path is not a file: {}", file.display());
            }
        }

        Ok(())
    }
}

/// Configuration specific to generate-policies subcommand
#[derive(Debug, Clone)]
struct GeneratePolicyCliConfig {
    /// Shared configuration
    shared: SharedConfig,
    /// AWS region
    region: String,
    /// AWS account ID
    account: String,
    /// Output individual policies instead of merged policy
    individual_policies: bool,
    /// Upload policies to AWS with optional custom name prefix
    upload_policies: Option<String>,
    /// Enable minimal policy size by allowing cross-service merging
    minimal_policy_size: bool,
    /// Disable file system caching for service references
    disable_cache: bool,
    /// Generate explanations for why actions were added (with optional action filters)
    explain: Option<Vec<String>>,
    /// Optional Terraform project directory
    terraform_dir: Option<PathBuf>,
    /// Optional individual Terraform files
    terraform_files: Vec<PathBuf>,
    /// Optional paths to terraform.tfstate files
    tfstate: Vec<PathBuf>,
    /// Optional explicit .tfvars file paths
    tfvars: Vec<PathBuf>,
    /// Optional ARN patterns to filter resource binding explanations
    explain_resources: Option<Vec<String>>,
}

impl GeneratePolicyCliConfig {
    /// Validate the configuration
    fn validate(&self) -> Result<()> {
        self.shared.validate()
    }
}

const SERVICE_HINTS_LONG_HELP: &str = "Space-separated list of AWS service names to filter \
which SDK calls are analyzed. This helps reduce unnecessary permissions by limiting analysis to \
only the services your application actually uses. For example, if your code only uses S3 and IAM \
services, specify '--service-hints s3 iam' to avoid analyzing unrelated method calls that might \
match other services like Chime. Note: The final policy may still include actions from services \
not in your hints if they are required for the operations you perform (e.g., KMS actions for S3 \
encryption).";

const LONG_ABOUT: &str = r"Unified tool that combines IAM policy generation from source code analysis with
automatic AccessDenied error fixing.

Examples:

  iam-policy-autopilot fix-access-denied \
    'User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject \
    on resource: arn:aws:s3:::my-bucket/my-key because no identity-based policy allows the \
    s3:GetObject action'

  iam-policy-autopilot generate-policies example.py \
    --region us-east-1 --account 123456789012 --pretty

  iam-policy-autopilot generate-policies src/**/*.py \
    --service-hints s3 iam --region us-east-1 --account 123456789012 --pretty

  iam-policy-autopilot mcp-server

  iam-policy-autopilot mcp-server --transport http --port 8001";

#[derive(Parser, Debug)]
#[command(
    name = "iam-policy-autopilot",
    author,
    version,
    disable_version_flag = true,
    about = "Generate IAM policies from source code and fix AccessDenied errors",
    long_about = LONG_ABOUT,
    before_help = "iam-policy-autopilot (IAM Policy Autopilot)",
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, TelemetryEventDerive)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Fix AccessDenied errors by analyzing and optionally applying IAM policy changes
    #[command(
        long_about = "Parses AccessDenied error messages to identify missing IAM permissions, \
generates the minimal required policy statements, and optionally applies them automatically. \
Supports both explicit denials (with action/resource details) and implicit denials (requiring analysis). \
When not using --yes, provides interactive confirmation before applying changes."
    )]
    #[telemetry(command = "fix-access-denied")]
    FixAccessDenied {
        /// Error text containing AccessDenied message. If not provided, reads from stdin.
        #[arg(
            long_help = "The AccessDenied error text to analyze. Can be a full CloudTrail log entry, \
Lambda error message, or raw IAM error message. If not provided as an argument, \
the tool will read from stdin, allowing you to pipe error messages directly."
        )]
        #[telemetry(presence)]
        source: Option<String>,

        /// Skip confirmation prompt and apply fix automatically (only for ImplicitIdentity denials)
        #[arg(
            short = 'y',
            long = "yes",
            long_help = "Automatically applies the policy fix without \
prompting for confirmation. Only works for implicit identity denials where the fix can be \
safely automated. For other denial types, you'll still need to review and apply changes manually."
        )]
        #[telemetry(value)]
        yes: bool,
    },

    /// Extracts AWS SDK method calls from source code files
    #[command(
        hide = true,
        long_about = "Extracts AWS SDK method calls from source code files and outputs them as JSON. \
This is the basic extraction functionality that identifies method calls, parameters, \
and basic metadata without enrichment."
    )]
    #[telemetry(skip)]
    ExtractSdkCalls {
        /// Source files to analyze for SDK method extraction
        #[arg(required = true, num_args = 1.., long_help = "One or more source code files to analyze. \
Supports multiple programming languages including Python (.py), TypeScript (.ts), JavaScript (.js), \
Go (.go), Java (.java), and others. Files are processed concurrently for better performance.")]
        source_files: Vec<PathBuf>,

        /// Enable debug logging output to stderr (most verbose)
        #[arg(
            hide = true,
            short = 'd',
            long = "debug",
            long_help = "Enables the most detailed logging information \
including TRACE, DEBUG, INFO, WARN, and ERROR messages with comprehensive file processing progress, \
method extraction details, and performance metrics. This is the most verbose logging level. \
All log output is sent to stderr to keep stdout clean for JSON output. \
If both --debug and --verbose are specified, --debug takes precedence."
        )]
        debug: bool,

        /// Format JSON output with indentation for readability
        #[arg(
            short = 'p',
            long = "pretty",
            long_help = "Formats the JSON output with proper indentation \
and line breaks for human readability. When disabled, outputs compact JSON suitable for \
machine processing and pipelines."
        )]
        pretty: bool,

        /// Override programming language detection
        #[arg(
            short = 'l',
            long = "language",
            long_help = "Manually specify the programming language \
instead of auto-detecting from file extensions. Supported languages: python, typescript, javascript, \
go, rust, java, cpp, c, csharp. When not specified, all source files must have the same detected language."
        )]
        language: Option<String>,

        /// Output complete ExtractedMethods with metadata
        #[arg(
            long = "full-output",
            long_help = "When enabled, outputs the complete ExtractedMethods \
structure including metadata about extraction time, source files, and warnings. By default, \
extract-sdk-calls outputs a simplified list of operations with their possible services. \
This flag has no effect on the generate-policies subcommand."
        )]
        full_output: bool,

        /// Filter extracted SDK calls to specific AWS services
        #[arg(
            long = "service-hints",
            num_args = 1..,
            long_help = SERVICE_HINTS_LONG_HELP,
        )]
        service_hints: Option<Vec<String>>,
    },

    /// Generates baseline IAM policy documents from source files
    #[command(
        long_about = r#"Generates baseline IAM policy documents from source files using
deterministic static analysis. Optionally takes AWS context (region and account)
for accurate ARN generation.

Supported languages and SDKs:
  Go          Go v2
  Java        Java v2
  JavaScript  JavaScript v3
  TypeScript  JavaScript v3
  Python      Boto3, Botocore

TIP: Use --service-hints to specify the AWS services your application uses. The
final policy may still include actions from other services if required."#
    )]
    #[telemetry(command = "generate-policies")]
    GeneratePolicies {
        /// Source files to analyze for SDK method extraction
        #[arg(required = true, num_args = 1..)]
        #[telemetry(presence)]
        source_files: Vec<PathBuf>,

        /// Enable debug logging output to stderr (most verbose)
        #[arg(hide = true, short = 'd', long = "debug")]
        debug: bool,

        /// Format JSON output with indentation for readability
        #[arg(short = 'p', long = "pretty")]
        #[telemetry(value)]
        pretty: bool,

        /// Override programming language detection
        #[arg(short = 'l', long = "language")]
        #[telemetry(value, if_present)]
        language: Option<String>,

        /// Output full ExtractedMethods instead of simplified operations
        #[arg(long = "full-output")]
        #[telemetry(value)]
        full_output: bool,

        /// AWS region
        #[arg(
            short = 'r',
            long = "region",
            default_value = "*",
            long_help = "AWS region to use for ARN generation. \
Examples: us-east-1, us-west-2, eu-west-1."
        )]
        #[telemetry(presence, default = "*")]
        region: String,

        /// AWS account ID
        #[arg(
            short = 'a',
            long = "account",
            default_value = "*",
            long_help = "AWS account ID to use for ARN generation."
        )]
        #[telemetry(presence, default = "*")]
        account: String,

        /// Output separate policies for each method call instead of a single merged policy
        #[arg(
            hide = true,
            long = "individual-policies",
            long_help = "When enabled, outputs individual IAM policies \
for each method call. Disables --upload-policy, if provided."
        )]
        #[telemetry(value)]
        individual_policies: bool,

        /// Upload generated policies to AWS IAM with optional custom name prefix
        #[arg(long = "upload-policies", num_args = 0..=1, require_equals = false, default_missing_value = "",
              long_help = "Upload the generated policies to AWS IAM using the iam:CreatePolicy API. \
Optionally specify a custom name prefix for the uploaded policies. \
If not provided, policies will be named using the default pattern: \
IamPolicyAutopilotGeneratedPolicy_1, IamPolicyAutopilotGeneratedPolicy_2, etc. \
If a custom prefix is provided, policies will be named: \
<CUSTOM_PREFIX>_1, <CUSTOM_PREFIX>_2, etc. \
The tool automatically finds the lowest available number for each policy name.")]
        #[telemetry(presence)]
        upload_policies: Option<String>,

        /// Enable minimal policy size by allowing cross-service action merging
        #[arg(
            long = "minimize-policy-size",
            long_help = "When enabled, allows merging of actions from \
different AWS services into the same policy statement. This can result in smaller, more compact policies \
but may be less readable. By default, actions from different services are kept in separate statements \
for better organization."
        )]
        #[telemetry(value)]
        minimal_policy_size: bool,

        /// Disable file system caching for service references
        #[arg(
            long = "disable-cache",
            long_help = "When enabled, disables file system caching for service reference data. \
By default, service reference data is cached in the system temp directory for 6 hours to improve performance. \
Use this flag to force fresh data retrieval on every run."
        )]
        #[telemetry(value)]
        disable_cache: bool,

        /// Filter extracted SDK calls to specific AWS services
        #[arg(
            long = "service-hints",
            num_args = 1..,
            long_help = SERVICE_HINTS_LONG_HELP,
        )]
        #[telemetry(list)]
        service_hints: Option<Vec<String>>,

        /// Generate explanations for why actions were added, filtered to specific action patterns
        #[arg(
            long = "explain",
            num_args = 1..,
            value_name = "ACTION_PATTERNS",
            long_help = "Generates detailed explanations for why IAM actions were added to the policy. \
Requires one or more action patterns. Use '*' to explain all actions. \
Patterns support wildcards (*) that match any sequence of characters. \
Examples:\n  \
--explain '*'                 # Explain all actions\n  \
--explain 's3:*'              # Explain only S3 actions\n  \
--explain s3:PutObject        # Explain only s3:PutObject\n  \
--explain 'ec2:Describe*'     # Explain EC2 Describe actions\n  \
--explain 's3:*' 'dynamodb:*' # Explain S3 and DynamoDB actions"
        )]
        #[telemetry(list)]
        explain: Option<Vec<String>>,

        /// Terraform project directory for resolving ARNs to use in resource block in generated policies
        #[arg(
            long = "tf-dir",
            long_help = "Directory containing Terraform .tf files. When provided, the tool parses \
Terraform resources to discover AWS infrastructure and generates more precise IAM policies by \
using concrete resource names in ARNs, when possible. .tf files discovered in the Terraform \
directory are combined with any files specified via --tf-files."
        )]
        terraform_dir: Option<PathBuf>,

        /// One or more .tf file(s) for resolving ARNs to use in resource block in generated policies
        #[arg(
            long = "tf-files",
            num_args = 1..,
            long_help = "One or more individual Terraform .tf files to parse for AWS resource definitions. \
When provided, the tool parses Terraform resources to discover AWS infrastructure and generates \
more precise IAM policies by using concrete resource names in ARNs, when possible. These files \
are combined with any directory specified via --tf-dir."
        )]
        terraform_files: Vec<PathBuf>,

        /// One or more .tfvars file(s) for variable overrides
        #[arg(
            long = "tfvars",
            num_args = 1..,
            long_help = "One or more .tfvars files for overriding Terraform variable values. When \
provided, these files are used to resolve variable references in resource definitions, enabling \
more precise IAM policies by using concrete resource names in ARNs. These files take precedence \
over auto-discovered terraform.tfvars and *.auto.tfvars files from the Terraform directory. \
Applied in order (later files override earlier ones). This is equivalent to Terraform's \
-var-file= CLI flag."
        )]
        tfvars: Vec<PathBuf>,

        /// One or more .tfstate file(s) for resolving exact deployed ARNs to use in resource block in generated policies
        #[arg(
            long = "tfstate",
            num_args = 1..,
            long_help = "One or more terraform.tfstate files containing deployed resource state. \
When provided, the tool uses actual deployed resource ARNs to generate more precise IAM policies. \
State-derived ARNs take precedence over those derived from .tf files. Can be used with --tf-dir, \
--tf-files, or independently."
        )]
        tfstate: Vec<PathBuf>,

        /// Generate explanations for why resource ARNs were added, filtered to specified patterns
        #[arg(
            long = "explain-resources",
            num_args = 1..,
            long_help = "Show where concrete resource ARNs in the generated policy came from \
(Terraform source file, state file, etc.). Accepts one or more ARN glob patterns to filter which \
resources are explained. Only works when Terraform inputs (--tf-dir, --tf-files, or --tfstate) \
are also provided.\n\n\
Examples:\n  \
--explain-resources '*'                                                        # Explain all resource ARNs\n  \
--explain-resources 'arn:aws:s3:::*'                                           # Explain only S3 bucket ARNs\n  \
--explain-resources 'arn:*:dynamodb:*'                                         # Explain only DynamoDB ARNs\n  \
--explain-resources 'arn:aws:s3:::*' 'arn:aws:sqs:*'                           # Explain S3 and SQS ARNs\n \
--explain-resources 'arn:aws:dynamodb:us-east-1:123456789012:table/users-prod' # Explain specific resource ARNs"
        )]
        explain_resources: Option<Vec<String>>,
    },

    /// Start MCP server
    #[command(
        long_about = "Starts an MCP server that provides IAM policy generation \
and AccessDenied error fixing capabilities to IDEs and other tools. The server can run in stdio mode \
for direct integration or HTTP mode for network-based communication. \
Supports both transport mechanisms with configurable logging."
    )]
    // MCP server notice is sent through `notifications/message` on initialization
    #[telemetry(command = "mcp-server", skip_notice)]
    McpServer {
        /// Transport mechanism for MCP communication
        #[arg(short = 't', long = "transport", default_value_t = McpTransport::Stdio,
              long_help = "Transport mechanism for MCP communication. 'stdio' uses standard input/output \
for direct integration with IDEs and tools. 'http' starts an HTTP server for network-based communication.")]
        #[telemetry(value)]
        transport: McpTransport,

        /// Port number for HTTP transport (ignored for stdio transport)
        #[arg(short = 'p', long = "port", default_value_t = MCP_HTTP_DEFAULT_PORT,
              long_help = "Port number to bind the HTTP server to when using HTTP transport. \
Only used when --transport=http. The server will bind to the specified address on the specified port.")]
        #[telemetry(skip)]
        port: u16,

        /// Bind address for HTTP transport (ignored for stdio transport)
        #[arg(short = 'b', long = "bind-address", default_value_t = DEFAULT_BIND_ADDRESS.to_string(),
              long_help = "IP address to bind the HTTP server to when using HTTP transport. \
Only used when --transport=http. Defaults to 127.0.0.1 (localhost). \
Use 0.0.0.0 to listen on all interfaces.")]
        bind_address: String,
    },

    #[command(
        about = "Print version information.",
        short_flag = 'V',
        long_flag = "version"
    )]
    #[telemetry(skip)]
    Version {
        #[arg(long = "verbose", default_value_t = false, hide = true)]
        verbose: bool,
    },

    /// Manage anonymous telemetry settings
    #[command(
        long_about = "View or change anonymous telemetry settings.\n\n\
IAM Policy Autopilot collects anonymous usage metrics to improve the tool.\n\
No file paths, policy content, AWS account IDs, or credentials are ever collected.\n\n\
Use --enable or --disable to persist your preference to ~/.iam-policy-autopilot/telemetry.json.\n\
Use --status to view the current telemetry state.\n\n\
The IAM_POLICY_AUTOPILOT_TELEMETRY environment variable (0/1) always takes precedence over the config file."
    )]
    #[telemetry(skip)]
    Telemetry {
        /// Enable anonymous telemetry
        #[arg(long = "enable", conflicts_with = "disable")]
        enable: bool,

        /// Disable anonymous telemetry
        #[arg(long = "disable", conflicts_with = "enable")]
        disable: bool,

        /// Show current telemetry status
        #[arg(long = "status")]
        status: bool,
    },
}

/// Initialize logging based on configuration
fn init_logging(debug: bool) -> Result<()> {
    let log_level = if debug {
        // Debug takes precedence - most verbose logging including TRACE
        log::LevelFilter::Trace
    } else {
        // Default: only ERROR messages
        log::LevelFilter::Error
    };

    env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .format_target(false)
        .format_timestamp_secs()
        .init();

    Ok(())
}

/// Handle the extract-sdk-calls subcommand
async fn handle_extract_sdk_calls(config: &SharedConfig) -> Result<()> {
    use iam_policy_autopilot_policy_generation::api::model::ServiceHints;

    info!("Running extract-sdk-calls command");

    // Validate configuration
    config
        .validate()
        .context("Configuration validation failed")?;

    let service_hints = config.service_hints.as_ref().map(|names| ServiceHints {
        service_names: names.clone(),
    });

    let results = extract_sdk_calls(&ExtractSdkCallsConfig {
        source_files: config.source_files.clone(),
        language: config.language.clone(),
        service_hints,
    })
    .await?;

    let json_output =
        SdkMethodCall::serialize_list(&results.methods, config.full_output, config.pretty)
            .context("Failed to output extracted operations")?;

    // Output to stdout (not using println! to avoid extra newline in compact mode)
    print!("{json_output}");
    if config.pretty {
        println!(); // Add newline for pretty output
    }

    trace!("Extracted methods JSON output written to stdout");
    Ok(())
}

/// Handle the generate-policies subcommand.
/// Returns the number of policies generated (used for telemetry).
async fn handle_generate_policy(config: &GeneratePolicyCliConfig) -> Result<usize> {
    use iam_policy_autopilot_policy_generation::api::model::ServiceHints;

    info!("Running generate-policies command");

    // Validate configuration
    config
        .validate()
        .context("Configuration validation failed")?;

    let service_hints = config
        .shared
        .service_hints
        .as_ref()
        .map(|names| ServiceHints {
            service_names: names.clone(),
        });

    let result = generate_policies(&GeneratePolicyConfig {
        extract_sdk_calls_config: ExtractSdkCallsConfig {
            source_files: config.shared.source_files.clone(),
            language: config.shared.language.clone(),
            service_hints,
        },
        aws_context: AwsContext::new(config.region.clone(), config.account.clone())?,
        individual_policies: config.individual_policies,
        minimize_policy_size: config.minimal_policy_size,
        disable_file_system_cache: config.disable_cache,
        explain_filters: config.explain.clone(),
        terraform_dir: config.terraform_dir.clone(),
        terraform_files: config.terraform_files.clone(),
        tfstate_paths: config.tfstate.clone(),
        tfvars_files: config.tfvars.clone(),
        explain_resource_filters: config.explain_resources.clone(),
    })
    .await?;

    let num_policies = result.policies.len();

    if config.individual_policies {
        // Output individual policies
        trace!("Outputting {} individual policies", result.policies.len());
        output::output_iam_policies(result, None, config.shared.pretty)
            .context("Failed to output individual IAM policies")?;
    } else {
        // Default behavior: output merged policy with optional upload
        let upload_result = if config.upload_policies.is_some() {
            trace!("Uploading policies to AWS IAM");

            let uploader = PolicyUploader::new()
                .await
                .context("Failed to create policy uploader")?;

            let custom_name = config.upload_policies.as_deref().filter(|s| !s.is_empty());
            let batch_response = uploader
                .upload_policies(&result.policies, custom_name)
                .await
                .context("Failed to upload policies to AWS IAM")?;

            debug!(
                "Upload completed: {} successful, {} failed",
                batch_response.successful.len(),
                batch_response.failed.len()
            );

            // Log upload results
            for upload in &batch_response.successful {
                debug!(
                    "Successfully uploaded policy: {} (ARN: {})",
                    upload.policy_name, upload.policy_arn
                );
            }
            for (index, error) in &batch_response.failed {
                debug!("Failed to upload policy {index}: {error}");
            }

            Some(batch_response)
        } else {
            None
        };

        output::output_iam_policies(result, upload_result, config.shared.pretty)
            .context("Failed to output merged IAM policy")?;
    }

    Ok(num_policies)
}

fn show_telemetry_notice(cli: &Cli) {
    // --- Telemetry: show notice (before execution) ---
    // Skip CLI notice for variants annotated with #[telemetry(skip)] or #[telemetry(skip_notice)]:
    //   - `telemetry` subcommand (user is already managing telemetry) — via skip
    //   - `mcp-server` subcommand (notice is sent via MCP notifications/message instead) — via skip_notice
    if !cli.command.should_skip_notice() {
        if let Some(notice) = telemetry::telemetry_notice() {
            eprintln!("\n{notice}\n");
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    show_telemetry_notice(&cli);

    // Build telemetry event using the derived ToTelemetryEvent trait (result data added after execution).
    // to_telemetry_event() returns None automatically when telemetry is disabled or for #[telemetry(skip)] variants.
    let mut telemetry_event = cli.command.to_telemetry_event();

    let code = match cli.command {
        Commands::FixAccessDenied { source, yes } => {
            let error_text = match source {
                None => {
                    // No argument provided - read from stdin
                    use std::io::{self, Read};
                    let mut buffer = String::new();
                    match io::stdin().read_to_string(&mut buffer) {
                        Ok(_) => buffer,
                        Err(e) => {
                            eprintln!("iam-policy-autopilot: Failed to read from stdin: {e}");
                            process::exit(ExitCode::Error.into());
                        }
                    }
                }
                Some(text) => text,
            };

            commands::fix_access_denied(&error_text, yes).await
        }

        Commands::ExtractSdkCalls {
            source_files,
            debug,
            pretty,
            language,
            full_output,
            service_hints,
        } => {
            // Initialize logging
            if let Err(e) = init_logging(debug) {
                eprintln!("iam-policy-autopilot: Failed to initialize logging: {e}");
                process::exit(1);
            }

            let config = SharedConfig {
                source_files,
                pretty,
                language,
                full_output,
                service_hints,
            };

            match handle_extract_sdk_calls(&config).await {
                Ok(()) => ExitCode::Success,
                Err(e) => {
                    print_cli_command_error(e);
                    ExitCode::Duplicate // Exit code 1 for extract-sdk-calls errors
                }
            }
        }

        Commands::GeneratePolicies {
            source_files,
            debug,
            pretty,
            language,
            full_output,
            region,
            account,
            individual_policies,
            upload_policies,
            minimal_policy_size,
            disable_cache,
            service_hints,
            explain,
            terraform_dir,
            terraform_files,
            tfstate,
            tfvars,
            explain_resources,
        } => {
            // Initialize logging
            if let Err(e) = init_logging(debug) {
                eprintln!("iam-policy-autopilot: Failed to initialize logging: {e}");
                process::exit(1);
            }

            let config = GeneratePolicyCliConfig {
                shared: SharedConfig {
                    source_files,
                    pretty,
                    language,
                    full_output,
                    service_hints,
                },
                region,
                account,
                individual_policies,
                upload_policies,
                minimal_policy_size,
                disable_cache,
                explain,
                terraform_dir,
                terraform_files,
                tfstate,
                tfvars,
                explain_resources,
            };

            match handle_generate_policy(&config).await {
                Ok(num_policies) => {
                    if let Some(ref mut event) = telemetry_event {
                        event.set_result_num_policies(num_policies);
                    }
                    ExitCode::Success
                }
                Err(e) => {
                    print_cli_command_error(e);
                    ExitCode::Duplicate // Exit code 1 for generate-policies errors
                }
            }
        }

        Commands::McpServer {
            transport,
            port,
            bind_address,
        } => {
            match start_mcp_server(transport, port, &bind_address).await {
                Ok(()) => ExitCode::Success,
                Err(e) => {
                    print_cli_command_error(e);
                    ExitCode::Error // Exit code 2 for mcp-server errors
                }
            }
        }

        Commands::Version { verbose } => match print_version_info(verbose) {
            Ok(()) => ExitCode::Success,
            Err(e) => {
                print_cli_command_error(e);
                ExitCode::Error
            }
        },

        Commands::Telemetry {
            enable,
            disable,
            status,
        } => {
            if enable {
                telemetry::set_telemetry_choice(TelemetryChoice::Enabled);
                eprintln!("Telemetry enabled. Preference saved to ~/.iam-policy-autopilot/telemetry.json");
            } else if disable {
                telemetry::set_telemetry_choice(TelemetryChoice::Disabled);
                eprintln!("Telemetry disabled. Preference saved to ~/.iam-policy-autopilot/telemetry.json");
            }

            if status || (!enable && !disable) {
                eprintln!("{}", telemetry::telemetry_status_string());
            }

            ExitCode::Success
        }
    };

    // --- Telemetry: emit AFTER execution with result data ---
    telemetry::finalize_and_emit(telemetry_event, code == ExitCode::Success).await;

    process::exit(code.into());
}

fn print_cli_command_error(e: anyhow::Error) {
    eprintln!("Error: {e}");
    let mut source = e.source();
    while let Some(err) = source {
        eprintln!("  Caused by: {err}");
        source = err.source();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iam_policy_autopilot_common::telemetry::ToTelemetryEvent;

    /// Verify that every CLI telemetry field from the `Commands` enum is documented
    /// in TELEMETRY.md. Fails if a `#[telemetry(...)]` annotation is added or
    /// renamed without updating the doc.
    #[test]
    fn test_cli_telemetry_fields_documented_in_telemetry_md() {
        let fields = Commands::telemetry_fields();

        let telemetry_md = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../TELEMETRY.md"
        ))
        .expect("Failed to read TELEMETRY.md");

        for field in &fields {
            // Skip fields with "not collected" mode — they're documented as not collected
            if field.collection_mode == "not collected" {
                continue;
            }

            let header = format!("### CLI: `{}` Command", field.command);
            assert!(
                telemetry_md.contains(&header),
                "TELEMETRY.md missing section: {header}"
            );

            let field_row = format!("| `{}` |", field.field_name);
            assert!(
                telemetry_md.contains(&field_row),
                "TELEMETRY.md missing field `{}` for CLI command `{}`",
                field.field_name,
                field.command,
            );
        }
    }
}
