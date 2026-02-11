use std::fmt::Display;

use anyhow::{Context, Result};
use clap::ValueEnum;
use log::info;

pub mod mcp;
pub(crate) mod tools;

static BIND_ADDRESS: &str = "127.0.0.1";

#[derive(Clone, Debug, ValueEnum)]
pub enum McpTransport {
    Stdio,
    Http,
}

impl Display for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio => write!(f, "stdio"),
            Self::Http => write!(f, "http"),
        }
    }
}

pub async fn start_mcp_server(transport: McpTransport, port: u16) -> Result<()> {
    info!("Starting MCP server with transport: {transport}");

    let env = env_logger::Env::default().filter_or("IAMPA_LOG_LEVEL", "debug");

    let timestamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string();
    let prefix = format!("iam-policy-autopilot-mcp-{timestamp}-");

    // Set up logging to temp file
    let path_str: Option<String> = {
        let temp_file = tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(".log")
            .tempfile()
            .with_context(|| "Failed to create temp file for logging.")?;

        let (log_file, path) = temp_file.keep()?;
        let path_str = path.display().to_string();

        let mut builder = env_logger::Builder::from_env(env);
        builder
            .target(env_logger::Target::Pipe(Box::new(log_file)))
            .init();

        std::panic::set_hook(Box::new(move |panic_info| {
            log::error!("Panic occurred: {panic_info}");
            eprintln!("Panic occurred: {panic_info}");
            eprintln!("For full log details see: {}", path.display());
        }));

        // It's fine to print to stderr
        eprintln!("Debug logs written to: {path_str}");

        Some(path_str)
    };

    match transport {
        McpTransport::Http => {
            let bind_address: String = format!("{BIND_ADDRESS}:{port}");
            info!("Starting HTTP MCP server at {bind_address}");

            crate::mcp::begin_http_transport(bind_address.as_str(), path_str)
                .await
                .with_context(|| format!("Failed to start HTTP Server at '{bind_address}'"))?;
        }
        McpTransport::Stdio => {
            info!("Starting STDIO MCP server");

            crate::mcp::begin_stdio_transport(path_str)
                .await
                .with_context(|| "Failed to start STDIO Server".to_string())?;
        }
    }

    Ok(())
}
