//! JSON-RPC protocol implementation for LSP communication.
//!
//! This module handles the low-level details of formatting and parsing LSP messages
//! using JSON-RPC 2.0 over stdin/stdout.

#![allow(dead_code)]

use crate::lsp::error::LspError;
use std::time::Duration;
use tokio::io::BufReader;

/// Format a JSON-RPC message with Content-Length header.
///
/// LSP messages follow this format:
/// ```text
/// Content-Length: <byte_count>\r\n
/// \r\n
/// <JSON payload>
/// ```
pub fn format_message(message: &serde_json::Value) -> Vec<u8> {
    // Serialize the JSON message to a string
    let json_str = serde_json::to_string(message).expect("Failed to serialize JSON");

    // Get the byte length of the UTF-8 encoded JSON
    let content_length = json_str.len();

    log::trace!("Formatting LSP message: {content_length} bytes");

    // Format the complete message with Content-Length header
    format!("Content-Length: {content_length}\r\n\r\n{json_str}").into_bytes()
}

/// Read and parse a single LSP message from stdout.
///
/// Reads the Content-Length header, then reads the exact number of bytes
/// specified, and parses the JSON payload.
///
/// # Errors
///
/// Returns an error if:
/// - The operation times out
/// - The Content-Length header is malformed
/// - The JSON payload is invalid
pub async fn read_message<R>(
    reader: &mut BufReader<R>,
    timeout_duration: Duration,
) -> Result<serde_json::Value, LspError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncReadExt;
    use tokio::time::timeout;

    // Wrap the entire operation in a timeout
    timeout(timeout_duration, async {
        log::trace!("Reading LSP message with timeout: {timeout_duration:?}");

        // Read Content-Length header
        let mut header_line = String::new();
        reader
            .read_line(&mut header_line)
            .await
            .map_err(|e| LspError::ParseFailed(format!("Failed to read header: {e}")))?;

        // Parse content length from "Content-Length: N\r\n"
        let content_length = header_line
            .trim()
            .strip_prefix("Content-Length:")
            .ok_or_else(|| {
                log::warn!("Invalid LSP header format: {header_line}");
                LspError::ParseFailed(format!("Invalid header format: {header_line}"))
            })?
            .trim()
            .parse::<usize>()
            .map_err(|e| {
                log::warn!("Invalid content length in header: {e}");
                LspError::ParseFailed(format!("Invalid content length: {e}"))
            })?;

        log::trace!("Reading {content_length} bytes of LSP message payload");

        // Read empty line (\r\n)
        let mut empty_line = String::new();
        reader
            .read_line(&mut empty_line)
            .await
            .map_err(|e| LspError::ParseFailed(format!("Failed to read separator: {e}")))?;

        // Read exact number of bytes for the JSON payload
        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer).await.map_err(|e| {
            log::error!("Failed to read LSP message payload: {e}");
            LspError::ParseFailed(format!("Failed to read payload: {e}"))
        })?;

        // Parse JSON
        let value = serde_json::from_slice(&buffer).map_err(|e| {
            log::error!("Failed to parse LSP JSON payload: {e}");
            LspError::ParseFailed(format!("Invalid JSON: {e}"))
        })?;

        log::trace!("Successfully parsed LSP message");
        Ok(value)
    })
    .await
    .map_err(|_| {
        log::warn!("LSP message read timed out after {timeout_duration:?}");
        LspError::Timeout(timeout_duration)
    })?
}

/// Send a message to the LSP server stdin.
///
/// # Errors
///
/// Returns an error if the write operation fails.
pub async fn send_message<W>(writer: &mut W, message: &serde_json::Value) -> Result<(), LspError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    log::trace!(
        "Sending LSP message: method={}",
        message
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
    );

    // Format the message with Content-Length header
    let formatted = format_message(message);

    // Write to stdin
    writer.write_all(&formatted).await.map_err(|e| {
        log::error!("Failed to write LSP message: {e}");
        LspError::SendFailed(e)
    })?;

    // Flush to ensure the message is sent immediately
    writer.flush().await.map_err(|e| {
        log::error!("Failed to flush LSP message: {e}");
        LspError::SendFailed(e)
    })?;

    log::trace!("LSP message sent successfully");
    Ok(())
}

/// Create an initialize request using lsp-types.
///
/// # Arguments
///
/// * `id` - The JSON-RPC request ID
/// * `workspace_root` - The workspace root directory path
///
/// # Notes
///
/// This function uses `workspace_folders` (the modern LSP approach) for specifying
/// the workspace. The deprecated `root_uri` field is not set and will be serialized
/// as `null`, which is correct per the LSP specification when `workspace_folders` is provided.
#[allow(deprecated)]
pub fn create_initialize_request(id: u64, workspace_root: &str) -> serde_json::Value {
    use lsp_types::{ClientCapabilities, InitializeParams, Url, WorkspaceFolder};
    use serde_json::json;

    let workspace_uri = Url::from_file_path(workspace_root).expect("Invalid workspace path");

    let params = InitializeParams {
        process_id: Some(std::process::id()),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: workspace_uri,
            name: workspace_root
                .split('/')
                .next_back()
                .unwrap_or("workspace")
                .to_string(),
        }]),
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": params
    })
}

/// Create a textDocument/didOpen notification using lsp-types.
///
/// # Arguments
///
/// * `file_uri` - The file URI (e.g., "file:///path/to/file.py")
/// * `content` - The file content
pub fn create_did_open_notification(file_uri: &str, content: &str) -> serde_json::Value {
    use lsp_types::{DidOpenTextDocumentParams, TextDocumentItem, Url};
    use serde_json::json;

    let params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: Url::parse(file_uri).expect("Invalid file URI"),
            language_id: "python".to_string(),
            version: 1,
            text: content.to_string(),
        },
    };

    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": params
    })
}

/// Create a textDocument/hover request using lsp-types.
///
/// # Arguments
///
/// * `id` - The JSON-RPC request ID
/// * `file_uri` - The file URI
/// * `line` - Zero-based line number
/// * `character` - Zero-based character position
pub fn create_hover_request(
    id: u64,
    file_uri: &str,
    line: u32,
    character: u32,
) -> serde_json::Value {
    use lsp_types::{
        HoverParams, Position, TextDocumentIdentifier, TextDocumentPositionParams, Url,
    };
    use serde_json::json;

    let params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: Url::parse(file_uri).expect("Invalid file URI"),
            },
            position: Position { line, character },
        },
        work_done_progress_params: Default::default(),
    };

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/hover",
        "params": params
    })
}

/// Create a shutdown request.
///
/// # Arguments
///
/// * `id` - The JSON-RPC request ID
pub fn create_shutdown_request(id: u64) -> serde_json::Value {
    use serde_json::json;

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "shutdown"
    })
}

/// Create an exit notification.
pub fn create_exit_notification() -> serde_json::Value {
    use serde_json::json;

    json!({
        "jsonrpc": "2.0",
        "method": "exit"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn test_format_message_simple() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        });

        let formatted = format_message(&message);
        let formatted_str = String::from_utf8(formatted).unwrap();

        // Verify the format: Content-Length header, blank line, then JSON
        assert!(formatted_str.starts_with("Content-Length: "));
        assert!(formatted_str.contains("\r\n\r\n"));

        // Extract the content length from the header
        let parts: Vec<&str> = formatted_str.split("\r\n\r\n").collect();
        assert_eq!(parts.len(), 2);

        let header = parts[0];
        let json_body = parts[1];

        // Parse the content length
        let content_length: usize = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();

        // Verify the content length matches the actual JSON body length
        assert_eq!(content_length, json_body.len());

        // Verify the JSON body can be parsed back
        let parsed: serde_json::Value = serde_json::from_str(json_body).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["method"], "initialize");
    }

    #[test]
    fn test_format_message_with_special_characters() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "params": {
                "text": "Hello\nWorld\t\"quoted\""
            }
        });

        let formatted = format_message(&message);
        let formatted_str = String::from_utf8(formatted).unwrap();

        // Extract content length and body
        let parts: Vec<&str> = formatted_str.split("\r\n\r\n").collect();
        let header = parts[0];
        let json_body = parts[1];

        let content_length: usize = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();

        // Verify UTF-8 byte length is correct (not character count)
        assert_eq!(content_length, json_body.len());

        // Verify the JSON can be parsed and special characters are preserved
        let parsed: serde_json::Value = serde_json::from_str(json_body).unwrap();
        assert_eq!(parsed["params"]["text"], "Hello\nWorld\t\"quoted\"");
    }

    #[test]
    fn test_format_message_with_unicode() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "params": {
                "text": "Hello 世界 🚀"
            }
        });

        let formatted = format_message(&message);
        let formatted_str = String::from_utf8(formatted).unwrap();

        // Extract content length and body
        let parts: Vec<&str> = formatted_str.split("\r\n\r\n").collect();
        let header = parts[0];
        let json_body = parts[1];

        let content_length: usize = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();

        // Verify byte length is correct (Unicode characters take multiple bytes)
        assert_eq!(content_length, json_body.len());
        // The byte length should be greater than the character count
        assert!(content_length > "Hello 世界 🚀".chars().count());

        // Verify the JSON can be parsed and Unicode is preserved
        let parsed: serde_json::Value = serde_json::from_str(json_body).unwrap();
        assert_eq!(parsed["params"]["text"], "Hello 世界 🚀");
    }

    #[tokio::test]
    async fn test_read_message_simple() {
        // Create a mock LSP message
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"capabilities": {}}
        });

        // Format it as an LSP message
        let formatted = format_message(&message);

        // Create a mock reader from the formatted bytes
        let cursor = std::io::Cursor::new(formatted);
        let mut reader = BufReader::new(cursor);

        // Read and parse the message
        let parsed = read_message(&mut reader, Duration::from_secs(5))
            .await
            .unwrap();

        // Verify we got the same JSON back
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert!(parsed["result"].is_object());
    }

    #[tokio::test]
    async fn test_read_message_with_unicode() {
        // Create a message with Unicode content
        let message = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "contents": {
                    "value": "Type: str 世界 🚀"
                }
            }
        });

        let formatted = format_message(&message);
        let cursor = std::io::Cursor::new(formatted);
        let mut reader = BufReader::new(cursor);

        let parsed = read_message(&mut reader, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(parsed["result"]["contents"]["value"], "Type: str 世界 🚀");
    }

    #[tokio::test]
    async fn test_read_message_malformed_header() {
        // Create a message with invalid header
        let invalid_message = b"Invalid-Header: 10\r\n\r\n{}";
        let cursor = std::io::Cursor::new(invalid_message);
        let mut reader = BufReader::new(cursor);

        let result = read_message(&mut reader, Duration::from_secs(5)).await;

        assert!(result.is_err());
        match result {
            Err(LspError::ParseFailed(msg)) => {
                assert!(msg.contains("Invalid header format"));
            }
            _ => panic!("Expected ParseFailed error"),
        }
    }

    #[tokio::test]
    async fn test_read_message_invalid_content_length() {
        // Create a message with non-numeric content length
        let invalid_message = b"Content-Length: abc\r\n\r\n{}";
        let cursor = std::io::Cursor::new(invalid_message);
        let mut reader = BufReader::new(cursor);

        let result = read_message(&mut reader, Duration::from_secs(5)).await;

        assert!(result.is_err());
        match result {
            Err(LspError::ParseFailed(msg)) => {
                assert!(msg.contains("Invalid content length"));
            }
            _ => panic!("Expected ParseFailed error"),
        }
    }

    #[tokio::test]
    async fn test_read_message_invalid_json() {
        // Create a message with invalid JSON
        let invalid_json = b"Content-Length: 9\r\n\r\n{invalid}";
        let cursor = std::io::Cursor::new(invalid_json);
        let mut reader = BufReader::new(cursor);

        let result = read_message(&mut reader, Duration::from_secs(5)).await;

        assert!(result.is_err());
        match result {
            Err(LspError::ParseFailed(msg)) => {
                // The error message should mention JSON parsing failure
                assert!(msg.contains("Invalid JSON") || msg.contains("JSON"));
            }
            _ => panic!("Expected ParseFailed error"),
        }
    }

    #[tokio::test]
    async fn test_read_message_timeout() {
        // Create a pipe that will never have data available
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut buf_reader = BufReader::new(reader);

        // Spawn a task that writes data after the timeout
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = writer.write_all(b"Content-Length: 2\r\n\r\n{}").await;
        });

        // Try to read with a very short timeout
        let result = read_message(&mut buf_reader, Duration::from_millis(50)).await;

        assert!(result.is_err());
        match result {
            Err(LspError::Timeout(duration)) => {
                assert_eq!(duration, Duration::from_millis(50));
            }
            _ => panic!("Expected Timeout error"),
        }
    }

    #[tokio::test]
    async fn test_send_message_simple() {
        // Create a duplex stream to simulate stdin/stdout
        let (writer, mut reader) = tokio::io::duplex(1024);
        let mut stdin = writer;

        // Create a simple message
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        });

        // Send the message
        send_message(&mut stdin, &message).await.unwrap();

        // Read back what was written
        let mut buf_reader = BufReader::new(&mut reader);
        let received = read_message(&mut buf_reader, Duration::from_secs(1))
            .await
            .unwrap();

        // Verify the message was sent correctly
        assert_eq!(received["jsonrpc"], "2.0");
        assert_eq!(received["id"], 1);
        assert_eq!(received["method"], "initialize");
    }

    #[tokio::test]
    async fn test_send_message_with_unicode() {
        // Create a duplex stream
        let (writer, mut reader) = tokio::io::duplex(1024);
        let mut stdin = writer;

        // Create a message with Unicode content
        let message = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "params": {
                "text": "Hello 世界 🚀"
            }
        });

        // Send the message
        send_message(&mut stdin, &message).await.unwrap();

        // Read back what was written
        let mut buf_reader = BufReader::new(&mut reader);
        let received = read_message(&mut buf_reader, Duration::from_secs(1))
            .await
            .unwrap();

        // Verify Unicode was preserved
        assert_eq!(received["params"]["text"], "Hello 世界 🚀");
    }

    #[test]
    fn test_create_initialize_request() {
        use std::env;

        // Create a temporary directory for testing
        let temp_dir = env::temp_dir();
        let workspace_path = temp_dir.to_str().unwrap();

        let request = create_initialize_request(1, workspace_path);

        // Verify JSON-RPC structure
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["id"], 1);
        assert_eq!(request["method"], "initialize");

        // Verify params structure
        assert!(request["params"].is_object());
        assert!(request["params"]["processId"].is_number());
        assert!(request["params"]["capabilities"].is_object());

        // Check workspaceFolders field
        assert!(request["params"]["workspaceFolders"].is_array());
        let workspace_folders = request["params"]["workspaceFolders"].as_array().unwrap();
        assert_eq!(workspace_folders.len(), 1);

        // Verify the workspace folder structure
        let workspace_folder = &workspace_folders[0];
        assert!(workspace_folder["uri"].is_string());
        assert!(workspace_folder["name"].is_string());

        // Verify URI is a file:// URI
        let uri = workspace_folder["uri"].as_str().unwrap();
        assert!(uri.starts_with("file://"));
    }

    #[test]
    fn test_create_did_open_notification() {
        let file_uri = "file:///tmp/test.py";
        let content = "import boto3\ns3 = boto3.client('s3')";

        let notification = create_did_open_notification(file_uri, content);

        // Verify JSON-RPC structure (notifications don't have id)
        assert_eq!(notification["jsonrpc"], "2.0");
        assert!(notification["id"].is_null());
        assert_eq!(notification["method"], "textDocument/didOpen");

        // Verify params structure
        assert!(notification["params"].is_object());
        assert_eq!(notification["params"]["textDocument"]["uri"], file_uri);
        assert_eq!(
            notification["params"]["textDocument"]["languageId"],
            "python"
        );
        assert_eq!(notification["params"]["textDocument"]["version"], 1);
        assert_eq!(notification["params"]["textDocument"]["text"], content);
    }

    #[test]
    fn test_create_hover_request() {
        let file_uri = "file:///tmp/test.py";
        let line = 5;
        let character = 10;

        let request = create_hover_request(42, file_uri, line, character);

        // Verify JSON-RPC structure
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["id"], 42);
        assert_eq!(request["method"], "textDocument/hover");

        // Verify params structure - lsp-types flattens textDocumentPositionParams
        assert!(request["params"].is_object());
        assert_eq!(request["params"]["textDocument"]["uri"], file_uri);
        assert_eq!(request["params"]["position"]["line"], line);
        assert_eq!(request["params"]["position"]["character"], character);
    }

    #[test]
    fn test_create_shutdown_request() {
        let request = create_shutdown_request(99);

        // Verify JSON-RPC structure
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["id"], 99);
        assert_eq!(request["method"], "shutdown");

        // Shutdown request has no params
        assert!(request["params"].is_null());
    }

    #[test]
    fn test_create_exit_notification() {
        let notification = create_exit_notification();

        // Verify JSON-RPC structure (notifications don't have id)
        assert_eq!(notification["jsonrpc"], "2.0");
        assert!(notification["id"].is_null());
        assert_eq!(notification["method"], "exit");

        // Exit notification has no params
        assert!(notification["params"].is_null());
    }
}
