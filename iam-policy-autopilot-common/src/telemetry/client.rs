//! Telemetry HTTP client with AWS SigV4 signing.
//!
//! Sends telemetry events as signed JSON POST requests to an AWS Lambda Function URL.
//! Uses AWS SigV4 authentication so the Lambda can use AWS_IAM auth mode.
//! All errors are silently ignored to ensure telemetry never impacts tool functionality.

use std::time::SystemTime;

use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use log::{debug, trace};
use reqwest::Client;

use super::event::TelemetryEvent;

/// The Lambda Function URL endpoint for telemetry ingestion.
const TELEMETRY_ENDPOINT: &str =
    "https://ou7gxtzouk3koqzt5wcpyko4e40qyroi.lambda-url.us-east-1.on.aws/";

const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";

/// AWS region for SigV4 signing (Lambda is deployed here).
const SIGNING_REGION: &str = "us-east-1";

/// AWS service name for Lambda Function URL SigV4 signing.
const SIGNING_SERVICE: &str = "lambda";

/// Fire-and-forget telemetry client with SigV4 signing.
///
/// Serializes [`TelemetryEvent`]s to JSON, signs the request with AWS SigV4,
/// and sends via HTTPS POST to the telemetry Lambda Function URL.
///
/// All errors (network, credentials, serialization, etc.) are silently caught —
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
    /// The event is serialized to JSON, signed with SigV4 using the caller's
    /// AWS credentials, and sent as a POST request to the Lambda Function URL.
    pub async fn emit(&self, event: &TelemetryEvent) {
        debug!(
            "Telemetry: preparing event for command='{}' anonymous_id={:?}",
            event.command, event.anonymous_id
        );

        let json_body = match event.to_json() {
            Ok(body) => {
                debug!("Telemetry: serialized payload ({} bytes): {}", body.len(), body);
                body
            }
            Err(e) => {
                debug!("Telemetry: serialization failed (ignored): {e}");
                return;
            }
        };

        // Attempt to sign and send with SigV4
        if let Err(e) = self.sign_and_send(&json_body).await {
            debug!("Telemetry: send failed (ignored): {e}");
        }
    }

    /// Sign the request with SigV4 and send it.
    async fn sign_and_send(&self, json_body: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Load AWS credentials from the default provider chain
        debug!("Telemetry: loading AWS credentials from default provider chain");
        let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
        let credentials_provider = config
            .credentials_provider()
            .ok_or("No AWS credentials provider available")?;
        let credentials = credentials_provider.provide_credentials().await?;
        debug!("Telemetry: AWS credentials loaded successfully");

        let identity = credentials.into();

        // Create the signing params
        let mut signing_settings = SigningSettings::default();
        signing_settings.payload_checksum_kind =
            aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;

        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(SIGNING_REGION)
            .name(SIGNING_SERVICE)
            .time(SystemTime::now())
            .settings(signing_settings)
            .build()?;

        // Build signable request with the content-type header
        let headers = [("content-type", "application/json")];
        let signable_request = SignableRequest::new(
            "POST",
            &self.endpoint,
            headers.iter().copied(),
            SignableBody::Bytes(json_body.as_bytes()),
        )?;

        // Sign the request
        debug!(
            "Telemetry: signing request with SigV4 (service={}, region={})",
            SIGNING_SERVICE, SIGNING_REGION
        );
        let (signing_instructions, _signature) =
            sign(signable_request, &signing_params.into())?.into_parts();

        // Build an http::Request so we can apply signing instructions
        let mut http_request = http::Request::builder()
            .method("POST")
            .uri(&self.endpoint)
            .header("content-type", "application/json")
            .body(json_body.to_string())?;

        // Apply the SigV4 signing headers to the http::Request
        signing_instructions.apply_to_request_http1x(&mut http_request);

        // Log the signed headers for debugging
        debug!("Telemetry: sending POST to {}", self.endpoint);
        for (name, value) in http_request.headers() {
            trace!("Telemetry:   header {}: {:?}", name, value);
        }

        // Transfer all headers (including SigV4 signing headers) to reqwest
        let mut request_builder = self
            .client
            .post(&self.endpoint)
            .body(json_body.to_string());

        for (name, value) in http_request.headers() {
            request_builder = request_builder.header(name.clone(), value.clone());
        }

        let response = request_builder.send().await?;
        let status = response.status();
        let response_body = response.text().await.unwrap_or_else(|_| "<unreadable>".to_string());

        if status.is_success() {
            debug!(
                "Telemetry: event sent successfully (status={}, body={})",
                status, response_body
            );
        } else {
            debug!(
                "Telemetry: server returned error (status={}, body={})",
                status, response_body
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_client_creation() {
        // global() is infallible — just verify it doesn't panic
        let _client = TelemetryClient::global();
    }

    #[tokio::test]
    async fn test_emit_json_contains_required_fields() {
        let event = TelemetryEvent::new("generate-policies")
            .with_str("language", "python")
            .with_result_success(true)
            .with_result_num_policies(2);

        // Verify the JSON payload serializes correctly
        let json = event.to_json().expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse");
        assert_eq!(parsed["command"], "generate-policies");
        assert!(parsed["version"].is_string());
        // anonymous_id is auto-loaded from config
        assert!(parsed["anonymous_id"].is_string());
        assert_eq!(parsed["params"]["language"], "python");
        assert_eq!(parsed["result"]["success"], true);
        assert_eq!(parsed["result"]["num_policies_generated"], 2);
    }

    #[tokio::test]
    async fn test_emit_fire_and_forget_on_connection_refused() {
        // Use a port that is not listening
        let client = TelemetryClient::with_endpoint("http://127.0.0.1:1".to_string());
        let event = TelemetryEvent::new("test-command");

        // Should not panic or propagate error (fire-and-forget)
        client.emit(&event).await;
    }

    #[tokio::test]
    async fn test_emit_fire_and_forget_without_credentials() {
        // Even without valid AWS credentials, emit should not panic
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let client = TelemetryClient::with_endpoint(mock_server.uri());
        let event = TelemetryEvent::new("test-command");

        // Should not panic — credentials failure is silently caught
        client.emit(&event).await;
    }
}
