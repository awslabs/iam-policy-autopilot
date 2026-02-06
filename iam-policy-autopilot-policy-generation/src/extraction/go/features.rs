//! Go SDK v2 feature method extraction
//!
//! This module handles extraction of Go AWS SDK v2 feature methods like S3 Upload/Download,
//! and other specialized SDK features that aren't regular service operations.

use rust_embed::RustEmbed;
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};

/// Embedded Go SDK v2 features configuration
#[derive(RustEmbed)]
#[folder = "resources/config/sdks/"]
#[include = "go-sdk-v2-features.json"]
struct GoSdkV2FeaturesAsset;

/// Root structure for Go SDK v2 features configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct GoSdkV2Features {
    /// Map of service name to their feature methods
    pub(crate) services: HashMap<String, HashMap<String, FeatureMethod>>,
}

/// Information about a receiver type (struct that has methods)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
// TODO: Use to track receiver of call
#[allow(dead_code)]
pub(crate) struct ReceiverInfo {
    /// Name of the receiver type (e.g., "Uploader", "Downloader")
    pub(crate) name: String,

    /// Constructor function signature
    pub(crate) constructor: String,
}

/// A feature method in the Go SDK v2
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct FeatureMethod {
    /// Method name (e.g., "Upload", "Download")
    pub(crate) method_name: String,

    /// Import path for this feature (e.g., "github.com/aws/aws-sdk-go-v2/feature/s3/manager")
    pub(crate) import: String,

    /// Receiver information (None for package-level functions)
    // TODO: use in def-use analysis during extraction
    #[allow(dead_code)]
    pub(crate) receiver: Option<ReceiverInfo>,

    /// List of IAM operations this feature requires
    pub(crate) operations: Vec<String>,

    /// Function signature
    // For documentation
    #[allow(dead_code)]
    pub(crate) signature: String,

    /// Minimum number of arguments required
    pub(crate) min_arguments: usize,

    /// Maximum number of arguments allowed
    // TODO: use in argument-based disambiguation
    #[allow(dead_code)]
    pub(crate) max_arguments: usize,

    /// Required named parameters (struct fields that must be present)
    // TODO: use in argument-based disambiguation
    #[allow(dead_code)]
    pub(crate) required_named_parameters: Vec<String>,
}

impl GoSdkV2Features {
    /// Load the embedded Go SDK v2 features configuration
    /// Uses a static cache to avoid re-parsing the JSON on subsequent calls
    pub(crate) fn load() -> Result<&'static Self, Box<dyn std::error::Error>> {
        static FEATURES_CACHE: OnceLock<Result<GoSdkV2Features, String>> = OnceLock::new();

        let cached = FEATURES_CACHE.get_or_init(|| {
            let file = match GoSdkV2FeaturesAsset::get("go-sdk-v2-features.json") {
                Some(f) => f,
                None => return Err("Failed to load embedded go-sdk-v2-features.json".to_string()),
            };

            serde_json::from_slice::<Self>(&file.data)
                .map_err(|e| format!("Failed to parse go-sdk-v2-features.json: {e}"))
        });

        cached.as_ref().map_err(|e| e.clone().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_features() {
        let features = GoSdkV2Features::load().expect("Failed to load features");
        let features2 = GoSdkV2Features::load().expect("Failed to load features");

        // Check that we have services
        assert!(
            !features.services.is_empty(),
            "Should have at least one service"
        );

        // Verify both calls return the same cached instance (pointer equality)
        assert!(
            std::ptr::eq(features, features2),
            "Both load() calls should return the same cached instance"
        );

        // Verify the content matches
        assert_eq!(
            features.services.len(),
            features2.services.len(),
            "Both instances should have the same number of services"
        );
    }
}
