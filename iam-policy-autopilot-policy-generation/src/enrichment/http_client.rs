//! Platform-abstracted HTTP client for service reference loading.
//!
//! - **Native**: Uses `reqwest::Client` (async, runs on tokio).
//! - **WASM/Emscripten**: Uses browser `fetch()` via Emscripten JS interop.

use crate::errors::ExtractorError;

/// Minimal async HTTP GET interface used by the service reference loader.
#[async_trait::async_trait]
pub(crate) trait HttpGet: Send + Sync + std::fmt::Debug {
    /// Perform a GET request and return the response body as text.
    async fn get_text(&self, url: &str) -> crate::errors::Result<String>;
}

// ---------------------------------------------------------------------------
// Native implementation (reqwest)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{ExtractorError, HttpGet};
    use reqwest::Client;

    const IAM_POLICY_AUTOPILOT: &str = "IAMPolicyAutopilot";

    #[derive(Debug, Clone)]
    pub(crate) struct ReqwestHttpClient {
        client: Client,
    }

    impl ReqwestHttpClient {
        pub(crate) fn new() -> crate::errors::Result<Self> {
            let user_agent_suffix = if cfg!(feature = "integ-test") {
                "-integration-test"
            } else {
                ""
            };

            let user_agent = format!(
                "{}{}/{}",
                IAM_POLICY_AUTOPILOT,
                user_agent_suffix,
                env!("CARGO_PKG_VERSION")
            );

            let client = Client::builder()
                .user_agent(user_agent)
                .build()
                .map_err(|e| ExtractorError::Configuration {
                    message:
                        "Failed to initialize the HTTP client for the service reference endpoint"
                            .to_string(),
                    source: Some(Box::new(e)),
                })?;

            Ok(Self { client })
        }
    }

    #[async_trait::async_trait]
    impl HttpGet for ReqwestHttpClient {
        async fn get_text(&self, url: &str) -> crate::errors::Result<String> {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| ExtractorError::Network {
                    message: format!(
                        "Failed to connect to '{url}'. \
                         Verify that this URL is reachable from your network \
                         (e.g. not blocked by a firewall or VPN). \
                         If you need to route through a proxy, \
                         set the HTTPS_PROXY environment variable \
                         (see https://docs.rs/reqwest/latest/reqwest/#proxies)"
                    ),
                    source: Some(Box::new(e)),
                })?
                .error_for_status()
                .map_err(|e| ExtractorError::Network {
                    message: format!("HTTP error from '{url}'"),
                    source: Some(Box::new(e)),
                })?
                .text()
                .await
                .map_err(|e| ExtractorError::Network {
                    message: format!("Failed to read response body from '{url}'"),
                    source: Some(Box::new(e)),
                })?;

            Ok(response)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::ReqwestHttpClient;

// ---------------------------------------------------------------------------
// Emscripten/WASM implementation (browser fetch via JS FFI)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)]
mod emscripten {
    use super::*;

    extern "C" {
        /// Call browser fetch(url) and return the response text.
        /// Returns a pointer to a null-terminated UTF-8 string allocated with malloc.
        /// Caller must free with `em_fetch_free`. Returns null on error.
        fn em_fetch_get_sync(url_ptr: *const u8, url_len: u32) -> *mut u8;
        /// Free a string returned by `em_fetch_get_sync`.
        fn em_fetch_free(ptr: *mut u8);
    }

    #[derive(Debug, Clone)]
    pub(crate) struct EmscriptenHttpClient;

    impl EmscriptenHttpClient {
        pub(crate) fn new() -> crate::errors::Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait::async_trait]
    impl HttpGet for EmscriptenHttpClient {
        async fn get_text(&self, url: &str) -> crate::errors::Result<String> {
            let url_bytes = url.as_bytes();
            let result_ptr =
                unsafe { em_fetch_get_sync(url_bytes.as_ptr(), url_bytes.len() as u32) };

            if result_ptr.is_null() {
                return Err(ExtractorError::Network {
                    message: format!("Fetch failed for '{url}'"),
                    source: None,
                });
            }

            // Read the C string, then free immediately to avoid leaks on error paths.
            let c_str = unsafe { std::ffi::CStr::from_ptr(result_ptr as *const i8) };
            let result = c_str.to_str().map(|s| s.to_owned());
            unsafe { em_fetch_free(result_ptr) };

            let text = result.map_err(|e| ExtractorError::Network {
                message: format!("Invalid UTF-8 in response from '{url}': {e}"),
                source: None,
            })?;

            Ok(text)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use emscripten::EmscriptenHttpClient;

// ---------------------------------------------------------------------------
// Factory function
// ---------------------------------------------------------------------------

/// Create the platform-appropriate HTTP client.
pub(crate) fn create_http_client() -> crate::errors::Result<Box<dyn HttpGet>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Box::new(ReqwestHttpClient::new()?))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Box::new(EmscriptenHttpClient::new()?))
    }
}
