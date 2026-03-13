//! Telemetry HTTP client.
//!
//! Sends telemetry events as custom HTTP headers on a lightweight GET request
//! to the service reference endpoint. All errors are silently ignored to ensure
//! telemetry never impacts tool functionality.

use log::{debug, trace};
use reqwest::Client;

use super::event::TelemetryEvent;

/// The service reference endpoint used for telemetry transmission.
/// This is the same endpoint already used by `RemoteServiceReferenceLoader`.
const TELEMETRY_ENDPOINT: &str = "https://servicereference.us-east-1.amazonaws.com";

const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";

/// Fire-and-forget telemetry client.
///
/// Encodes [`TelemetryEvent`]s as custom HTTP headers and sends them via
/// a lightweight GET request to the service reference endpoint. Uses the same
/// User-Agent as `RemoteServiceReferenceLoader`.
///
/// All errors (network, header encoding, etc.) are silently caught — telemetry
/// must never interfere with tool operation.
pub struct TelemetryClient {
    client: Client,
    endpoint: String,
}

impl TelemetryClient {
    /// Create a new telemetry client with the default endpoint.
    ///
    /// Returns `None` if the HTTP client cannot be created (should be extremely rare).
    #[must_use]
    pub fn new() -> Option<Self> {
        Self::create_client().map(|client| Self {
            client,
            endpoint: TELEMETRY_ENDPOINT.to_string(),
        })
    }

    /// Create a new telemetry client with a custom endpoint (for testing).
    #[cfg(test)]
    pub(crate) fn with_endpoint(endpoint: String) -> Option<Self> {
        Self::create_client().map(|client| Self { client, endpoint })
    }

    /// Build the reqwest client with the same User-Agent as `RemoteServiceReferenceLoader`.
    fn create_client() -> Option<Client> {
        let user_agent = format!(
            "{}/{}",
            IAM_POLICY_AUTOPILOT,
            env!("CARGO_PKG_VERSION")
        );

        Client::builder().user_agent(user_agent).build().ok()
    }

    /// Emit a telemetry event. This is fire-and-forget: all errors are silently ignored.
    ///
    /// The event is encoded as custom HTTP headers on a GET request to the
    /// service reference endpoint.
    pub async fn emit(&self, event: &TelemetryEvent) {
        trace!("Emitting telemetry event: command={}", event.command);

        let headers = event.to_headers();

        let mut request = self.client.get(&self.endpoint);
        for (name, value) in &headers {
            // reqwest silently handles invalid header names/values,
            // but we filter defensively just in case
            if let (Ok(header_name), Ok(header_value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                request = request.header(header_name, header_value);
            }
        }

        match request.send().await {
            Ok(_) => {
                debug!("Telemetry event sent successfully: command={}", event.command);
            }
            Err(e) => {
                // Fire-and-forget: silently ignore all errors
                debug!("Telemetry send failed (ignored): {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_client_creation() {
        let client = TelemetryClient::new();
        assert!(client.is_some());
    }

    #[tokio::test]
    async fn test_emit_sends_headers() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(header_exists("x-ipa-command"))
            .and(header_exists("x-ipa-version"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client =
            TelemetryClient::with_endpoint(mock_server.uri()).expect("client should be created");
        let event = TelemetryEvent::new("test-command").with_bool("pretty", true);

        client.emit(&event).await;

        // Verify mock expectations were met (1 request received)
    }

    #[tokio::test]
    async fn test_emit_sends_parameter_headers() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(header_exists("x-ipa-p-pretty"))
            .and(header_exists("x-ipa-p-language"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client =
            TelemetryClient::with_endpoint(mock_server.uri()).expect("client should be created");
        let event = TelemetryEvent::new("generate-policies")
            .with_bool("pretty", true)
            .with_str("language", "python");

        client.emit(&event).await;
    }

    #[tokio::test]
    async fn test_emit_fire_and_forget_on_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client =
            TelemetryClient::with_endpoint(mock_server.uri()).expect("client should be created");
        let event = TelemetryEvent::new("test-command");

        // Should not panic or propagate error
        client.emit(&event).await;
    }

    #[tokio::test]
    async fn test_emit_fire_and_forget_on_connection_refused() {
        // Use a port that is not listening
        let client = TelemetryClient::with_endpoint("http://127.0.0.1:1".to_string())
            .expect("client should be created");
        let event = TelemetryEvent::new("test-command");

        // Should not panic or propagate error
        client.emit(&event).await;
    }

    #[tokio::test]
    async fn test_emit_sends_request_with_user_agent() {
        let mock_server = MockServer::start().await;

        // The reqwest client is built with a User-Agent, so any GET request
        // reaching the mock server will include it.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client =
            TelemetryClient::with_endpoint(mock_server.uri()).expect("client should be created");
        let event = TelemetryEvent::new("test-command");

        client.emit(&event).await;

        // Verify that the request was received (mock expects exactly 1)
        // The User-Agent is set by the reqwest Client builder, verified by
        // the create_client() function which always sets it.
    }
}
