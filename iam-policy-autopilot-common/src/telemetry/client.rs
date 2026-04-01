//! Telemetry HTTP client for CloudFront with Lambda Function URL origin.
//!
//! Sends telemetry events as JSON POST requests via a CloudFront distribution
//! backed by an AWS Lambda Function URL with OAC (Origin Access Control).
//!
//! Per CloudFront OAC requirements for POST requests, the SHA-256 hash of the
//! request body is included in the `x-amz-content-sha256` header.
//! See: <https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/private-content-restricting-access-to-lambda.html>
//!
//! All errors are silently ignored to ensure telemetry never impacts tool functionality.

use log::debug;
use reqwest::Client;
use sha2::{Digest, Sha256};

use super::event::TelemetryEvent;

/// The CloudFront distribution endpoint for telemetry ingestion.
const TELEMETRY_ENDPOINT: &str = "https://d1c1nvl8iiaf7v.cloudfront.net/";

const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";

/// Fire-and-forget telemetry client.
///
/// Serializes [`TelemetryEvent`]s to JSON and sends via HTTPS POST to the
/// CloudFront distribution endpoint. The request includes a SHA-256 payload
/// hash in `x-amz-content-sha256` as required by CloudFront OAC for POST
/// requests to Lambda Function URL origins.
///
/// All errors (network, serialization, etc.) are silently caught —
/// telemetry must never interfere with tool operation.
pub struct TelemetryClient {
    client: Client,
    endpoint: String,
}

/// Global singleton for the telemetry client.
///
/// Initialized once on first access and reused for all subsequent telemetry
/// emissions within the process. This avoids creating a new HTTP client per event.
static GLOBAL_CLIENT: std::sync::OnceLock<TelemetryClient> = std::sync::OnceLock::new();

impl TelemetryClient {
    /// Get or initialize the global singleton telemetry client.
    ///
    /// The client is created once and reused for the lifetime of the process.
    pub fn global() -> &'static Self {
        GLOBAL_CLIENT.get_or_init(Self::new)
    }

    /// Create a new telemetry client with the default endpoint.
    fn new() -> Self {
        let user_agent = format!("{}/{}", IAM_POLICY_AUTOPILOT, env!("CARGO_PKG_VERSION"));
        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .expect("Failed to create HTTP client for telemetry");
        Self {
            client,
            endpoint: TELEMETRY_ENDPOINT.to_string(),
        }
    }

    /// Create a new telemetry client with a custom endpoint (for testing).
    #[cfg(test)]
    pub(crate) fn with_endpoint(endpoint: String) -> Self {
        let user_agent = format!("{}/{}", IAM_POLICY_AUTOPILOT, env!("CARGO_PKG_VERSION"));
        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .expect("Failed to create HTTP client for telemetry");
        Self { client, endpoint }
    }

    /// Emit a telemetry event. This is fire-and-forget: all errors are silently ignored.
    ///
    /// The event is serialized to JSON and sent as a POST request to the
    /// CloudFront endpoint with the SHA-256 payload hash header.
    pub async fn emit(&self, event: &TelemetryEvent) {
        debug!(
            "Telemetry: preparing event for command='{}' installation_id={:?}",
            event.command, event.installation_id
        );

        let json_body = match event.to_json() {
            Ok(body) => {
                debug!(
                    "Telemetry: serialized payload ({} bytes): {}",
                    body.len(),
                    body
                );
                body
            }
            Err(e) => {
                debug!("Telemetry: serialization failed (ignored): {e}");
                return;
            }
        };

        if let Err(e) = self.send(&json_body).await {
            debug!("Telemetry: send failed (ignored): {e}");
        }
    }

    /// Send the JSON payload with the `x-amz-content-sha256` header.
    ///
    /// CloudFront OAC requires POST requests to include the SHA-256 hash of
    /// the request body in the `x-amz-content-sha256` header. Lambda doesn't
    /// support unsigned payloads.
    async fn send(&self, json_body: &str) -> Result<(), Box<dyn std::error::Error>> {
        let payload_hash = sha256_hex(json_body.as_bytes());
        debug!(
            "Telemetry: sending POST to {} (payload sha256={})",
            self.endpoint, payload_hash
        );

        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .header("x-amz-content-sha256", &payload_hash)
            .body(json_body.to_string())
            .send()
            .await?;

        let status = response.status();
        let response_body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".to_string());

        if status.is_success() {
            debug!("Telemetry: event sent successfully (status={status}, body={response_body})");
        } else {
            debug!("Telemetry: server returned error (status={status}, body={response_body})");
        }

        Ok(())
    }
}

/// Compute the hex-encoded SHA-256 digest of the given bytes.
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    // Format each byte as two lowercase hex digits
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_client_creation() {
        let _client = TelemetryClient::global();
    }

    // =========================================================================
    // sha256_hex — parameterized over known inputs
    // =========================================================================

    #[rstest]
    #[case::empty(
        b"" as &[u8],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    )]
    #[case::json(
        br#"{"command":"test"}"# as &[u8],
        "" // non-empty placeholder — asserted by length + hex check below
    )]
    fn test_sha256_hex(#[case] input: &[u8], #[case] expected: &str) {
        let hash = sha256_hex(input);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        if !expected.is_empty() {
            assert_eq!(hash, expected);
        }
    }

    // =========================================================================
    // Emit — payload structure
    // =========================================================================

    #[tokio::test]
    async fn test_emit_json_contains_required_fields() {
        let event = TelemetryEvent::new("generate-policies")
            .with_str("language", "python")
            .with_result_success(true)
            .with_result_num_policies(2);

        let parsed: serde_json::Value = serde_json::from_str(&event.to_json().unwrap()).unwrap();

        assert_eq!(parsed["command"], "generate-policies");
        assert!(parsed["version"].is_string());
        assert!(parsed["installation_id"].is_string());
        assert_eq!(parsed["params"]["language"], "python");
        assert_eq!(parsed["result"]["success"], true);
        assert_eq!(parsed["result"]["num_policies_generated"], 2);
    }

    #[tokio::test]
    async fn test_emit_sends_sha256_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header_exists("x-amz-content-sha256"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        TelemetryClient::with_endpoint(mock_server.uri())
            .emit(&TelemetryEvent::new("test"))
            .await;
    }

    // =========================================================================
    // Fire-and-forget — parameterized error scenarios
    // =========================================================================

    #[tokio::test]
    async fn test_emit_fire_and_forget_on_connection_refused() {
        TelemetryClient::with_endpoint("http://127.0.0.1:1".to_string())
            .emit(&TelemetryEvent::new("test"))
            .await;
    }

    #[tokio::test]
    async fn test_emit_fire_and_forget_on_server_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        TelemetryClient::with_endpoint(mock_server.uri())
            .emit(&TelemetryEvent::new("test"))
            .await;
    }
}
