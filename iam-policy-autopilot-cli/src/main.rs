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

use iam_policy_autopilot_mcp_server::{start_mcp_server, McpTransport};
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
}

impl GeneratePolicyCliConfig {
    /// Validate the configuration
    fn validate(&self) -> Result<()> {
        self.shared.validate()
    }
}

const SERVICE_HINTS_LONG_HELP: &str =
    "Space-separated list of AWS service names to filter which SDK calls are analyzed. \
This helps reduce unnecessary permissions by limiting analysis to only the services your application actually uses. \
For example, if your code only uses S3 and IAM services, specify '--service-hints s3 iam' to avoid \
analyzing unrelated method calls that might match other services like Chime. \
Note: The final policy may still include actions from services not in your hints if they are \
required for the operations you perform (e.g., KMS actions for S3 encryption).";

#[derive(Parser, Debug)]
#[command(
    name = "iam-policy-autopilot",
    author,
    version,
    disable_version_flag = true,
    about = "Generate IAM policies from source code and fix AccessDenied errors",
    long_about = "Unified tool that combines IAM policy generation from source code analysis \
with automatic AccessDenied error fixing. Supports three main operations:\n\n\
• fix-access-denied: Fix AccessDenied errors by analyzing and applying IAM policy changes\n\
• generate-policies: Complete pipeline with enrichment for policy generation\n\
• mcp-server: Start MCP server for IDE integration. Uses STDIO transport by default.\n\n\
iam-policy-autopilot fix-access-denied 'User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key because no identity-based policy allows the s3:GetObject action'\n  \
iam-policy-autopilot generate-policies tests/resources/test_example.py --region us-east-1 --account 123456789012 --pretty\n  \
iam-policy-autopilot generate-policies tests/resources/test_example.py --service-hints s3 iam --region us-east-1 --account 123456789012 --pretty\n  \
iam-policy-autopilot mcp-server\n  \
iam-policy-autopilot mcp-server --transport http --port 8001"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Fix AccessDenied errors by analyzing and optionally applying IAM policy changes
    #[command(
        long_about = "Parses AccessDenied error messages to identify missing IAM permissions, \
generates the minimal required policy statements, and optionally applies them automatically. \
Supports both explicit denials (with action/resource details) and implicit denials (requiring analysis). \
When not using --yes, provides interactive confirmation before applying changes."
    )]
    FixAccessDenied {
        /// Error text containing AccessDenied message. If not provided, reads from stdin.
        #[arg(
            long_help = "The AccessDenied error text to analyze. Can be a full CloudTrail log entry, \
Lambda error message, or raw IAM error message. If not provided as an argument, \
the tool will read from stdin, allowing you to pipe error messages directly."
        )]
        source: Option<String>,

        /// Skip confirmation prompt and apply fix automatically (only for ImplicitIdentity denials)
        #[arg(
            short = 'y',
            long = "yes",
            long_help = "Automatically applies the policy fix without \
prompting for confirmation. Only works for implicit identity denials where the fix can be \
safely automated. For other denial types, you'll still need to review and apply changes manually."
        )]
        yes: bool,
    },

    /// Extracts AWS SDK method calls from source code files
    #[command(
        hide = true,
        long_about = "Extracts AWS SDK method calls from source code files and outputs them as JSON. \
This is the basic extraction functionality that identifies method calls, parameters, \
and basic metadata without enrichment."
    )]
    ExtractSdkCalls {
        /// Source files to analyze for SDK method extraction
        #[arg(required = true, num_args = 1.., long_help = "One or more source code files to analyze. \
Supports multiple programming languages including Python (.py), TypeScript (.ts), JavaScript (.js), \
Go (.go), and others. Files are processed concurrently for better performance.")]
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

    /// Generates complete IAM policy documents from source files
    #[command(long_about = "\
Generates complete IAM policy documents from source files. By default, all \
policies are merged into a single optimized policy document. \
Optionally takes AWS context (region and account) for accurate ARN generation.\n\n\
TIP: Use --service-hints to specify the particular AWS services that your application uses if you know them. \
The final policy may still include actions from other services if required for your operations.")]
    GeneratePolicies {
        /// Source files to analyze for SDK method extraction
        #[arg(required = true, num_args = 1..)]
        source_files: Vec<PathBuf>,

        /// Enable debug logging output to stderr (most verbose)
        #[arg(hide = true, short = 'd', long = "debug")]
        debug: bool,

        /// Format JSON output with indentation for readability
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// Override programming language detection
        #[arg(short = 'l', long = "language")]
        language: Option<String>,

        /// Output full ExtractedMethods instead of simplified operations
        #[arg(long = "full-output")]
        full_output: bool,

        /// AWS region
        #[arg(
            short = 'r',
            long = "region",
            default_value = "*",
            long_help = "AWS region to use for ARN generation. \
Examples: us-east-1, us-west-2, eu-west-1."
        )]
        region: String,

        /// AWS account ID
        #[arg(
            short = 'a',
            long = "account",
            default_value = "*",
            long_help = "AWS account ID to use for ARN generation."
        )]
        account: String,

        /// Output separate policies for each method call instead of a single merged policy
        #[arg(
            hide = true,
            long = "individual-policies",
            long_help = "When enabled, outputs individual IAM policies \
for each method call. Disables --upload-policy, if provided."
        )]
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
        upload_policies: Option<String>,

        /// Enable minimal policy size by allowing cross-service action merging
        #[arg(
            long = "minimize-policy-size",
            long_help = "When enabled, allows merging of actions from \
different AWS services into the same policy statement. This can result in smaller, more compact policies \
but may be less readable. By default, actions from different services are kept in separate statements \
for better organization."
        )]
        minimal_policy_size: bool,

        /// Disable file system caching for service references
        #[arg(
            long = "disable-cache",
            long_help = "When enabled, disables file system caching for service reference data. \
By default, service reference data is cached in the system temp directory for 6 hours to improve performance. \
Use this flag to force fresh data retrieval on every run."
        )]
        disable_cache: bool,

        /// Filter extracted SDK calls to specific AWS services
        #[arg(
            long = "service-hints",
            num_args = 1..,
            long_help = SERVICE_HINTS_LONG_HELP,
        )]
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
        explain: Option<Vec<String>>,
    },

    /// Start MCP server
    #[command(
        long_about = "Starts an MCP server that provides IAM policy generation \
and AccessDenied error fixing capabilities to IDEs and other tools. The server can run in stdio mode \
for direct integration or HTTP mode for network-based communication. \
Supports both transport mechanisms with configurable logging."
    )]
    McpServer {
        /// Transport mechanism for MCP communication
        #[arg(short = 't', long = "transport", default_value_t = McpTransport::Stdio,
              long_help = "Transport mechanism for MCP communication. 'stdio' uses standard input/output \
for direct integration with IDEs and tools. 'http' starts an HTTP server for network-based communication.")]
        transport: McpTransport,

        /// Port number for HTTP transport (ignored for stdio transport)
        #[arg(short = 'p', long = "port", default_value_t = MCP_HTTP_DEFAULT_PORT,
              long_help = "Port number to bind the HTTP server to when using HTTP transport. \
Only used when --transport=http. The server will bind to 127.0.0.1 (localhost) on the specified port.")]
        port: u16,
    },

    #[command(
        about = "Print version information.",
        short_flag = 'V',
        long_flag = "version"
    )]
    Version {
        #[arg(long = "verbose", default_value_t = false, hide = true)]
        verbose: bool,
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

/// Handle the generate-policies subcommand
async fn handle_generate_policy(config: &GeneratePolicyCliConfig) -> Result<()> {
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
    })
    .await?;

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

    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

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
            };

            match handle_generate_policy(&config).await {
                Ok(()) => ExitCode::Success,
                Err(e) => {
                    print_cli_command_error(e);
                    ExitCode::Duplicate // Exit code 1 for generate-policies errors
                }
            }
        }

        Commands::McpServer { transport, port } => {
            match start_mcp_server(transport, port).await {
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
    };

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
