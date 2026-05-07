use std::collections::HashMap;

use anyhow::{Context, Result};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};

use crate::Language;

/// Embedded external library model files.
///
/// These JSON files describe how third-party library function calls map to
/// underlying AWS SDK operations. They are embedded at compile time alongside
/// existing SDK metadata, following the same `rust-embed` pattern used for
/// botocore data, Go SDK features, and JS v3 libraries.
#[derive(RustEmbed)]
#[folder = "resources/config/external-library-models"]
#[include = "*.json"]
struct ExternalLibraryModelsAsset;

/// Represents a deserialized external library model file.
///
/// Each model describes one library for one language, mapping its function calls
/// to underlying AWS SDK operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ExternalLibraryModel {
    /// Library name (e.g., "aws_lambda_powertools")
    pub library_name: String,
    /// Target programming language
    pub language: Language,
    /// Semver version constraint (informational, not enforced at runtime)
    #[serde(default)]
    pub version: Option<String>,
    /// List of call patterns that map library calls to SDK operations
    pub call_patterns: Vec<CallPattern>,
}

/// Describes how to match a specific library function call and what SDK operations it maps to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CallPattern {
    /// Module path (e.g., "aws_lambda_powertools.utilities.parameters")
    pub module_path: String,
    /// Function or method name (e.g., "get_parameter")
    pub function_name: String,
    /// Whether this is a module-level call or instance method call
    pub call_type: CallType,
    /// AWS SDK operations this call maps to
    pub sdk_operations: Vec<SdkOperationMapping>,
}

/// Discriminator for how the function is invoked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallType {
    /// Called on an imported module: `parameters.get_parameter(...)`
    ModuleLevel,
    /// Called on an instantiated object: `provider.get(...)`
    InstanceMethod,
}

/// Maps a library call to a specific AWS SDK operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SdkOperationMapping {
    /// AWS service name (e.g., "ssm", "secretsmanager")
    pub service: String,
    /// AWS operation name in PascalCase (e.g., "GetParameter")
    pub operation: String,
}

/// Registry of external library models, indexed by (language, library_name).
///
/// The registry discovers, loads, and indexes all built-in external library
/// models for a given language. Built-in models are embedded at compile time
/// via `rust-embed`.
pub(crate) struct LibraryModelRegistry {
    /// Models indexed by (language, library_name) for fast lookup.
    models: HashMap<(Language, String), ExternalLibraryModel>,
}

impl LibraryModelRegistry {
    /// Create a new registry, loading built-in models for the given language.
    ///
    /// Built-in models that fail to parse are logged as warnings and skipped.
    pub(crate) fn load(language: Language) -> Result<Self> {
        let builtin_models = Self::load_builtin_models(language);

        let mut models = HashMap::new();
        for model in builtin_models {
            let key = (model.language, model.library_name.clone());
            models
                .entry(key)
                .and_modify(|existing: &mut ExternalLibraryModel| {
                    existing.call_patterns.extend(model.call_patterns.clone());
                })
                .or_insert(model);
        }

        Ok(Self { models })
    }

    /// Get all models for a given language.
    pub(crate) fn models_for_language(&self, language: Language) -> Vec<&ExternalLibraryModel> {
        self.models
            .values()
            .filter(|model| model.language == language)
            .collect()
    }

    /// Load built-in models from embedded resources, filtering by language.
    ///
    /// Iterates over all `.json` files in the embedded `external-library-models/`
    /// directory, parses each one, and collects models that match
    /// the requested language. If a built-in model fails to parse, a warning
    /// is logged and the model is skipped (extraction is not halted).
    fn load_builtin_models(language: Language) -> Vec<ExternalLibraryModel> {
        let mut models = Vec::new();

        for file_name in ExternalLibraryModelsAsset::iter() {
            let file_path = std::path::Path::new(file_name.as_ref());

            let embedded_file = if let Some(f) = ExternalLibraryModelsAsset::get(file_name.as_ref())
            {
                f
            } else {
                log::warn!(
                    "Failed to read embedded external library model '{}'",
                    file_path.display()
                );
                continue;
            };

            match serde_json::from_slice::<ExternalLibraryModel>(&embedded_file.data).with_context(
                || {
                    format!(
                        "Failed to parse external library model '{}'",
                        file_path.display()
                    )
                },
            ) {
                Ok(model) => {
                    if model.language == language {
                        log::debug!(
                            "Loaded built-in external library model '{}' for {:?}",
                            model.library_name,
                            language
                        );
                        models.push(model);
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Skipping invalid built-in external library model '{}': {:#}",
                        file_path.display(),
                        e
                    );
                }
            }
        }

        log::debug!(
            "Loaded {} built-in external library model(s) for {:?}",
            models.len(),
            language
        );

        models
    }

    /// Create a registry from a pre-built list of models.
    ///
    /// This is useful for testing where models are constructed programmatically
    /// rather than loaded from files.
    #[cfg(test)]
    pub(crate) fn from_models(models: Vec<ExternalLibraryModel>) -> Self {
        let mut map = HashMap::new();
        for model in models {
            let key = (model.language, model.library_name.clone());
            map.insert(key, model);
        }
        Self { models: map }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aws_lambda_powertools_model_with_all_fields() {
        let json = r#"{
            "library_name": "aws_lambda_powertools",
            "language": "python",
            "version": ">=2.0.0",
            "call_patterns": [
                {
                    "module_path": "aws_lambda_powertools.utilities.parameters",
                    "function_name": "get_parameter",
                    "call_type": "module_level",
                    "sdk_operations": [
                        {
                            "service": "ssm",
                            "operation": "GetParameter"
                        }
                    ]
                },
                {
                    "module_path": "aws_lambda_powertools.utilities.parameters",
                    "function_name": "get_secret",
                    "call_type": "module_level",
                    "sdk_operations": [
                        {
                            "service": "secretsmanager",
                            "operation": "GetSecretValue"
                        }
                    ]
                }
            ]
        }"#;

        let model: ExternalLibraryModel =
            serde_json::from_str(json).expect("should parse valid model JSON");

        assert_eq!(model.library_name, "aws_lambda_powertools");
        assert_eq!(model.language, Language::Python);
        assert_eq!(model.version, Some(">=2.0.0".to_string()));
        assert_eq!(model.call_patterns.len(), 2);

        let p0 = &model.call_patterns[0];
        assert_eq!(p0.module_path, "aws_lambda_powertools.utilities.parameters");
        assert_eq!(p0.function_name, "get_parameter");
        assert_eq!(p0.call_type, CallType::ModuleLevel);
        assert_eq!(p0.sdk_operations.len(), 1);
        assert_eq!(p0.sdk_operations[0].service, "ssm");
        assert_eq!(p0.sdk_operations[0].operation, "GetParameter");

        let p1 = &model.call_patterns[1];
        assert_eq!(p1.module_path, "aws_lambda_powertools.utilities.parameters");
        assert_eq!(p1.function_name, "get_secret");
        assert_eq!(p1.call_type, CallType::ModuleLevel);
        assert_eq!(p1.sdk_operations.len(), 1);
        assert_eq!(p1.sdk_operations[0].service, "secretsmanager");
        assert_eq!(p1.sdk_operations[0].operation, "GetSecretValue");
    }

    #[test]
    fn call_type_module_level_serializes_to_snake_case() {
        let json =
            serde_json::to_string(&CallType::ModuleLevel).expect("serialization should succeed");
        assert_eq!(json, r#""module_level""#);
    }

    #[test]
    fn call_type_instance_method_serializes_to_snake_case() {
        let json =
            serde_json::to_string(&CallType::InstanceMethod).expect("serialization should succeed");
        assert_eq!(json, r#""instance_method""#);
    }

    #[test]
    fn optional_version_defaults_to_none_when_absent() {
        let json = r#"{
            "library_name": "some_lib",
            "language": "python",
            "call_patterns": [
                {
                    "module_path": "some_lib.mod",
                    "function_name": "do_thing",
                    "call_type": "module_level",
                    "sdk_operations": [
                        { "service": "s3", "operation": "GetObject" }
                    ]
                }
            ]
        }"#;

        let model: ExternalLibraryModel =
            serde_json::from_str(json).expect("should parse model without version");
        assert_eq!(model.version, None);
    }
}
