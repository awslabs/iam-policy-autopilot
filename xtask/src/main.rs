use clap::{Parser, Subcommand};
use regex::Regex;
use std::path::PathBuf;
use std::{fs, path::Path};

mod build_unified_model;

#[derive(Parser)]
#[command(name = "xtask", about = "Repository maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Regenerate tests/java/service_name_test_cases.txt
    RegenerateServiceNames,

    /// Build a single ExternalLibraryModel for the whole Terraform AWS provider.
    ///
    /// Reads the reflection-derived CRUD operation model (output.json), runs
    /// model generation per service package, and unions the results into one
    /// model file.
    BuildUnifiedModel {
        /// Path to the reflection CRUD operation model (output.json).
        #[arg(long)]
        crud_operation_model: PathBuf,

        /// Root of the terraform-provider-aws checkout (contains internal/service).
        #[arg(long)]
        terraform_provider_aws_root: PathBuf,

        /// Where to write the unified model JSON.
        #[arg(long)]
        output: PathBuf,

        /// Restrict to these packages (comma-separated), for iteration/debugging.
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,

        /// Pretty-print the output JSON.
        #[arg(long)]
        pretty: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.task {
        Task::RegenerateServiceNames => regenerate_service_names(),
        Task::BuildUnifiedModel {
            crud_operation_model,
            terraform_provider_aws_root,
            output,
            only,
            pretty,
        } => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                .init();
            let opts = build_unified_model::BuildOptions {
                crud_operation_model,
                terraform_provider_aws_root,
                output,
                only_packages: only,
                pretty,
            };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio runtime");
            if let Err(e) = runtime.block_on(build_unified_model::run(opts)) {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
    }
}

/// Locates `validateServiceIdentifiersForEnvVarsAndProfileProperty()` by method name,
/// walks its body by brace depth, then extracts `(serviceId, javaServiceName)` pairs
/// from each `validateServiceIdSetting(...)` call (first and fourth string arguments).
///
/// Used by both `build.rs` (drift detection) and `xtask` (snapshot regeneration).
fn extract_service_id_settings(content: &str) -> Vec<(String, String)> {
    let re = Regex::new(
        r#"validateServiceIdSetting\(\s*"([^"]+)",\s*"[^"]+",\s*"[^"]+",\s*"([^"]+)"\s*\)"#,
    )
    .unwrap();

    // Find the target method
    let marker = "void validateServiceIdentifiersForEnvVarsAndProfileProperty()";
    let method_pos = content
        .find(marker)
        .expect("validateServiceIdentifiersForEnvVarsAndProfileProperty not found");

    // Find the opening '{' of the method body
    let body_start = content[method_pos..].find('{').unwrap() + method_pos + 1;

    // Walk forward tracking brace depth to find the closing '}'
    let mut depth = 1usize;
    let mut body_end = body_start;
    for ch in content[body_start..].chars() {
        body_end += ch.len_utf8();
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }

    re.captures_iter(&content[body_start..body_end])
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

fn regenerate_service_names() {
    // CARGO_MANIFEST_DIR is set by Cargo to the xtask crate directory at compile time,
    // so its parent is always the workspace root regardless of where the command is run from.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest dir has no parent");

    let src = workspace_root.join(
        "iam-policy-autopilot-policy-generation/resources/config/sdks/aws-sdk-java-v2/\
         codegen/src/test/java/software/amazon/awssdk/codegen/naming/DefaultNamingStrategyTest.java",
    );
    let content =
        fs::read_to_string(&src).unwrap_or_else(|e| panic!("Cannot read {}: {e}", src.display()));

    let pairs = extract_service_id_settings(&content);

    let header = "\
# Generated file — do not edit by hand.\n\
# Regenerate with: cargo xtask regenerate-service-names\n\
# Source: aws-sdk-java-v2 DefaultNamingStrategyTest.java — validateServiceIdentifiersForEnvVarsAndProfileProperty()\n\
# Format: serviceId=javaServiceName (fourth arg of validateServiceIdSetting)\n";

    let output: String = std::iter::once(header.to_string())
        .chain(pairs.iter().map(|(id, name)| format!("{id}={name}\n")))
        .collect();

    let dst = workspace_root
        .join("iam-policy-autopilot-policy-generation/tests/java/service_name_test_cases.txt");
    fs::write(&dst, &output).unwrap_or_else(|e| panic!("Cannot write {}: {e}", dst.display()));

    println!("Written {} entries to {}", pairs.len(), dst.display());
}
