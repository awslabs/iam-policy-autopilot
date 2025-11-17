use std::path::Path;

use rmcp::model::InitializeRequestParam;
use rmcp::RoleClient;
use rmcp::{
    model::{CallToolRequestParam, ClientCapabilities, ClientInfo, Implementation},
    service::RunningService,
    transport::{StreamableHttpClientTransport, TokioChildProcess},
    RmcpError, ServiceExt,
};
use serde_json::json;
use serial_test::serial;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};

async fn setup_stdio() -> RunningService<RoleClient, ()> {
    // Create MCP client using TokioChildProcess with debug binary
    let mut command = Command::new("../target/debug/iam-policy-autopilot");
    command.args(&["mcp-server"]);

    ().serve(
        TokioChildProcess::new(command)
            .map_err(RmcpError::transport_creation::<TokioChildProcess>)
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn wait_for_server_ready(port: u16, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn setup_http_with_port(
    port: u16,
) -> (RunningService<RoleClient, InitializeRequestParam>, Child) {
    // Start HTTP server as a background process using debug binary
    let mut command = Command::new("../target/debug/iam-policy-autopilot");
    command
        .args(&[
            "mcp-server",
            "--transport",
            "http",
            "--port",
            &port.to_string(),
        ])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());

    let server_process = command.spawn().expect("Failed to start HTTP server");

    // Wait for server to be ready with proper timeout
    if !wait_for_server_ready(port, 100).await {
        panic!(
            "Server failed to start within timeout period on port {}",
            port
        );
    }

    // Give a bit more time for the MCP service to be fully initialized
    sleep(Duration::from_millis(500)).await;

    // Create HTTP client transport
    let transport =
        StreamableHttpClientTransport::from_uri(format!("http://127.0.0.1:{}/mcp", port));
    let client_info = ClientInfo {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "test http client".to_string(),
            title: None,
            version: "0.0.1".to_string(),
            website_url: None,
            icons: None,
        },
    };

    let client = client_info.serve(transport).await.unwrap();

    (client, server_process)
}

async fn setup_http() -> (RunningService<RoleClient, InitializeRequestParam>, Child) {
    setup_http_with_port(8001).await
}

#[tokio::test]
async fn test_stdio_list_tools() {
    let client = setup_stdio().await;

    // Call list_tools to get available tools
    let tools_result = client.list_tools(None).await.unwrap();

    // Verify we have the expected tools
    assert_eq!(tools_result.tools.len(), 3);

    // Check that all expected tools are present
    let tool_names: Vec<&str> = tools_result.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(tool_names.contains(&"generate_application_policies"));
    assert!(tool_names.contains(&"generate_policy_for_access_denied"));
    assert!(tool_names.contains(&"fix_access_denied"));

    // Verify tool descriptions are present
    for tool in &tools_result.tools {
        if let Some(description) = &tool.description {
            assert!(
                !description.is_empty(),
                "Tool {} should have a non-empty description",
                tool.name
            );
        } else {
            panic!("Tool {} should have a description", tool.name);
        }
    }
}

#[tokio::test]
async fn test_stdio_generate_policy() {
    let test_file = std::env::current_dir()
        .unwrap()
        .join(Path::new("tests/test_data/lambda.py"));

    let client = setup_stdio().await;
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "generate_application_policies".into(),
            arguments: json!({
                "source_files": [test_file],
                "partition": "aws",
                "region": "us-east-1",
                "account": "123456789012"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    assert_eq!(tool_result.is_error, Some(false));
}

#[tokio::test]
async fn test_stdio_generate_policy_for_access_denied() {
    let client = setup_stdio().await;
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "generate_policy_for_access_denied".into(),
            arguments: json!({
                "error_message": "User: arn:aws:iam::123456789012:user/test-user is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::test-bucket/test-file.txt",
                "partition": "aws",
                "region": "us-east-1",
                "account": "123456789012"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    assert_eq!(tool_result.is_error, Some(false));
}

#[tokio::test]
#[serial]
async fn test_http_list_tools() {
    let (client, mut server_process) = setup_http().await;

    // Call list_tools to get available tools
    let tools_result = client.list_tools(None).await.unwrap();

    // Verify we have the expected tools
    assert_eq!(tools_result.tools.len(), 3);

    // Check that all expected tools are present
    let tool_names: Vec<&str> = tools_result.tools.iter().map(|t| t.name.as_ref()).collect();

    println!("tool_names: {tool_names:#?}");
    assert!(tool_names.contains(&"generate_application_policies"));
    assert!(tool_names.contains(&"generate_policy_for_access_denied"));
    assert!(tool_names.contains(&"fix_access_denied"));

    // Verify tool descriptions are present
    for tool in &tools_result.tools {
        if let Some(description) = &tool.description {
            assert!(
                !description.is_empty(),
                "Tool {} should have a non-empty description",
                tool.name
            );
        } else {
            panic!("Tool {} should have a description", tool.name);
        }
    }

    // Clean up: kill the server process
    let _ = server_process.kill().await;
}

#[tokio::test]
#[serial]
async fn test_http_generate_policy() {
    let test_file = std::env::current_dir()
        .unwrap()
        .join(Path::new("tests/test_data/lambda.py"));

    let (client, mut server_process) = setup_http_with_port(8002).await;
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "generate_application_policies".into(),
            arguments: json!({
                "source_files": [test_file],
                "partition": "aws",
                "region": "us-east-1",
                "account": "123456789012"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    assert_eq!(tool_result.is_error, Some(false));

    // Clean up: kill the server process
    let _ = server_process.kill().await;
}

#[tokio::test]
#[serial]
async fn test_http_generate_policy_for_access_denied() {
    let (client, mut server_process) = setup_http_with_port(8003).await;
    let tool_result = client
        .call_tool(CallToolRequestParam {
            name: "generate_policy_for_access_denied".into(),
            arguments: json!({
                "error_message": "User: arn:aws:iam::123456789012:user/test-user is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::test-bucket/test-file.txt",
                "partition": "aws",
                "region": "us-east-1",
                "account": "123456789012"
            })
            .as_object()
            .cloned(),
        })
        .await
        .unwrap();

    assert_eq!(tool_result.is_error, Some(false));

    // Clean up: kill the server process
    let _ = server_process.start_kill();
    let _ = server_process.wait().await;
}
