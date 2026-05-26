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
    /// Library version this model was built against (informational, not enforced at runtime)
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
    /// Class or type name for `StaticMethod`/`InstanceMethod` patterns (e.g., "SSMProvider")
    #[serde(default)]
    pub class_name: Option<String>,
    /// Function or method name (e.g., "get_parameter")
    pub function_name: String,
    /// The kind of callable this pattern represents
    pub call_type: CallType,
    /// AWS SDK operations this call maps to
    pub sdk_operations: Vec<SdkOperationMapping>,
}

/// Describes the kind of callable in the source library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallType {
    /// A function defined in a module/namespace: `mod.func()`, `pkg.Func()`, `func()`
    Function,
    /// A method called on a class/type: `Class.method()`, `Type::method()`
    StaticMethod,
    /// A method called on an instance: `obj.method()`, `obj->method()`
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

/// Registry of external library models, indexed by language.
///
/// The registry discovers, loads, and indexes all built-in external library
/// models for a given language. Built-in models are embedded at compile time
/// via `rust-embed`.
pub(crate) struct LibraryModelRegistry {
    models: HashMap<Language, Vec<ExternalLibraryModel>>,
}

impl LibraryModelRegistry {
    /// Create a new registry, loading built-in models for the given language.
    ///
    /// Panics if an embedded file cannot be read (indicates a build-time bug).
    /// Built-in models that fail to parse are logged as warnings and skipped.
    pub(crate) fn load(language: Language) -> Result<Self> {
        let builtin_models = Self::load_builtin_models(language);

        let mut models: HashMap<Language, Vec<ExternalLibraryModel>> = HashMap::new();
        for model in builtin_models {
            let lang = model.language;
            let entry = models.entry(lang).or_default();
            if let Some(existing) = entry
                .iter_mut()
                .find(|m| m.library_name == model.library_name)
            {
                existing.call_patterns.extend(model.call_patterns);
            } else {
                entry.push(model);
            }
        }

        Ok(Self { models })
    }

    /// Get all models for a given language.
    pub(crate) fn models_for_language(&self, language: Language) -> &[ExternalLibraryModel] {
        self.models
            .get(&language)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Load built-in models from embedded resources, filtering by language.
    ///
    /// Iterates over all `.json` files in the embedded `external-library-models/`
    /// directory, parses each one, and collects models that match the requested
    /// language. Panics if an embedded file listed by `iter()` cannot be read
    /// (this should never happen). If a file fails to parse, a warning is logged
    /// and the model is skipped (extraction is not halted).
    fn load_builtin_models(language: Language) -> Vec<ExternalLibraryModel> {
        let mut models = Vec::new();

        for file_name in ExternalLibraryModelsAsset::iter() {
            let embedded_file =
                ExternalLibraryModelsAsset::get(file_name.as_ref()).unwrap_or_else(|| {
                    panic!(
                        "Failed to read embedded external library model '{}'",
                        file_name.as_ref()
                    )
                });

            match serde_json::from_slice::<ExternalLibraryModel>(&embedded_file.data).with_context(
                || {
                    format!(
                        "Failed to parse external library model '{}'",
                        file_name.as_ref()
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
                        file_name.as_ref(),
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
        let mut map: HashMap<Language, Vec<ExternalLibraryModel>> = HashMap::new();
        for model in models {
            map.entry(model.language).or_default().push(model);
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
                    "call_type": "function",
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
                    "call_type": "function",
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
        assert_eq!(p0.call_type, CallType::Function);
        assert_eq!(p0.sdk_operations.len(), 1);
        assert_eq!(p0.sdk_operations[0].service, "ssm");
        assert_eq!(p0.sdk_operations[0].operation, "GetParameter");

        let p1 = &model.call_patterns[1];
        assert_eq!(p1.module_path, "aws_lambda_powertools.utilities.parameters");
        assert_eq!(p1.function_name, "get_secret");
        assert_eq!(p1.call_type, CallType::Function);
        assert_eq!(p1.sdk_operations.len(), 1);
        assert_eq!(p1.sdk_operations[0].service, "secretsmanager");
        assert_eq!(p1.sdk_operations[0].operation, "GetSecretValue");
    }

    #[test]
    fn call_type_function_serializes_to_snake_case() {
        let json =
            serde_json::to_string(&CallType::Function).expect("serialization should succeed");
        assert_eq!(json, r#""function""#);
    }

    #[test]
    fn call_type_static_method_serializes_to_snake_case() {
        let json =
            serde_json::to_string(&CallType::StaticMethod).expect("serialization should succeed");
        assert_eq!(json, r#""static_method""#);
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
                    "call_type": "function",
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
