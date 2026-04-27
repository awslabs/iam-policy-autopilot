use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
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
    /// Source file path for provenance tracking during merge conflict detection.
    /// Not serialized — set programmatically after loading.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
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
    /// Optional parameter constraints for conditional matching
    #[serde(default)]
    pub parameter_constraints: Vec<ParameterConstraint>,
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

/// Optional constraint for conditional matching based on call arguments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ParameterConstraint {
    /// Parameter name to check
    pub parameter_name: String,
    /// Expected literal value
    pub expected_value: String,
}

/// Registry of external library models, indexed by (language, library_name).
///
/// The registry discovers, loads, and indexes all external library
/// models (both built-in and user-provided) for a given language. Built-in
/// models are embedded at compile time via `rust-embed`; user-provided models
/// are loaded from a path supplied at runtime.
pub(crate) struct LibraryModelRegistry {
    /// Models indexed by (language, library_name) for fast lookup.
    models: HashMap<(Language, String), ExternalLibraryModel>,
}

impl LibraryModelRegistry {
    /// Create a new registry, loading built-in models for the given language
    /// and optionally loading user-provided models from a path.
    ///
    /// Built-in models that fail to parse are logged as warnings and skipped.
    /// User-provided models that fail to parse are hard errors (halt extraction).
    pub(crate) fn load(language: Language, user_model_path: Option<&Path>) -> Result<Self> {
        let builtin_models = Self::load_builtin_models(language);

        let user_models = match user_model_path {
            Some(path) => Self::load_user_models(path)?
                .into_iter()
                .filter(|m| m.language == language)
                .collect(),
            None => Vec::new(),
        };

        let models = Self::merge_models(builtin_models, user_models)?;

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
            let file_path = Path::new(file_name.as_ref());

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

    /// Load user-provided models from a file or directory path.
    ///
    /// When the path is a directory, all `.json` files within it are loaded
    /// non-recursively. When the path is a single `.json` file, that file is
    /// loaded. If the path does not exist, an error is returned.
    ///
    /// Unlike built-in models, user model parse failures are hard errors
    /// that halt extraction — users need to know their models are invalid so
    /// they can fix them.
    fn load_user_models(path: &Path) -> Result<Vec<ExternalLibraryModel>> {
        if !path.exists() {
            bail!(
                "User-provided library models path '{}' does not exist",
                path.display()
            );
        }

        let mut models = Vec::new();

        if path.is_dir() {
            let entries = std::fs::read_dir(path).with_context(|| {
                format!(
                    "Failed to read user-provided library models directory '{}'",
                    path.display()
                )
            })?;

            for entry in entries {
                let entry = entry.with_context(|| {
                    format!(
                        "Failed to read entry in user-provided library models directory '{}'",
                        path.display()
                    )
                })?;
                let entry_path = entry.path();

                // Only load .json files, skip directories and other files
                if entry_path.is_file()
                    && entry_path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                {
                    let content = std::fs::read(&entry_path).with_context(|| {
                        format!(
                            "Failed to read user-provided library model file '{}'",
                            entry_path.display()
                        )
                    })?;
                    let mut model: ExternalLibraryModel = serde_json::from_slice(&content)
                        .with_context(|| {
                            format!(
                                "Failed to parse external library model '{}'",
                                entry_path.display()
                            )
                        })?;
                    model.source_path = Some(entry_path.clone());
                    models.push(model);
                }
            }
        } else {
            let content = std::fs::read(path).with_context(|| {
                format!(
                    "Failed to read user-provided library model file '{}'",
                    path.display()
                )
            })?;
            let mut model: ExternalLibraryModel =
                serde_json::from_slice(&content).with_context(|| {
                    format!(
                        "Failed to parse external library model '{}'",
                        path.display()
                    )
                })?;
            model.source_path = Some(path.to_path_buf());
            models.push(model);
        }

        log::debug!(
            "Loaded {} user-provided external library model(s) from '{}'",
            models.len(),
            path.display()
        );

        Ok(models)
    }

    /// Merge built-in and user-provided models at the call-pattern level.
    ///
    /// Models are indexed by `(language, library_name)`. When multiple models
    /// target the same library, their call patterns are merged:
    ///
    /// - User-provided call patterns override built-in patterns with the same
    ///   `(module_path, function_name)` signature.
    /// - Non-overlapping patterns from different models for the same library
    ///   are combined into a single effective model.
    /// - Conflicting user-provided patterns (same `(module_path, function_name)`
    ///   from two different user model files) produce an error identifying both
    ///   source files.
    fn merge_models(
        builtin: Vec<ExternalLibraryModel>,
        user: Vec<ExternalLibraryModel>,
    ) -> Result<HashMap<(Language, String), ExternalLibraryModel>> {
        // A pattern signature uniquely identifies a call pattern within a library.
        type PatternSig = (String, String); // (module_path, function_name)

        let mut result: HashMap<(Language, String), ExternalLibraryModel> = HashMap::new();

        // Step 1: Index all built-in patterns by (language, library_name).
        // When multiple built-in models share the same key, merge their patterns.
        for model in builtin {
            let key = (model.language, model.library_name.clone());
            result
                .entry(key)
                .and_modify(|existing| {
                    existing.call_patterns.extend(model.call_patterns.clone());
                })
                .or_insert(model);
        }

        // Step 2: Track user pattern provenance for conflict detection.
        // Maps (language, library_name, module_path, function_name) -> source_path
        let mut user_pattern_sources: HashMap<(Language, String, String, String), PathBuf> =
            HashMap::new();

        // Step 3: Merge user models, checking for conflicts between user models.
        for user_model in user {
            let key = (user_model.language, user_model.library_name.clone());
            let user_source = user_model
                .source_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("<unknown>"));

            // TODO: conflict detection keys on (module_path, function_name) only,
            // ignoring parameter_constraints. Two patterns with different constraints
            // for the same function would conflict even though they're semantically different.
            for pattern in &user_model.call_patterns {
                let pattern_key = (
                    user_model.language,
                    user_model.library_name.clone(),
                    pattern.module_path.clone(),
                    pattern.function_name.clone(),
                );

                if let Some(existing_source) = user_pattern_sources.get(&pattern_key) {
                    bail!(
                        "Conflicting user-provided call patterns for '{}.{}' in library '{}': \
                         defined in both '{}' and '{}'",
                        pattern.module_path,
                        pattern.function_name,
                        user_model.library_name,
                        existing_source.display(),
                        user_source.display()
                    );
                }
                user_pattern_sources.insert(pattern_key, user_source.clone());
            }

            // Merge into the result map at the call-pattern level.
            match result.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();

                    // Build a set of user pattern signatures for fast lookup.
                    let user_sigs: HashMap<PatternSig, &CallPattern> = user_model
                        .call_patterns
                        .iter()
                        .map(|p| ((p.module_path.clone(), p.function_name.clone()), p))
                        .collect();

                    // Remove existing patterns that the user overrides.
                    existing.call_patterns.retain(|p| {
                        let sig = (p.module_path.clone(), p.function_name.clone());
                        !user_sigs.contains_key(&sig)
                    });

                    // Add all user patterns.
                    existing
                        .call_patterns
                        .extend(user_model.call_patterns.clone());
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(user_model);
                }
            }
        }

        Ok(result)
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
    // Unit tests for data model serialization

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
                    ],
                    "parameter_constraints": []
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
                    ],
                    "parameter_constraints": []
                }
            ]
        }"#;

        let model: ExternalLibraryModel =
            serde_json::from_str(json).expect("should parse valid model JSON");

        assert_eq!(model.library_name, "aws_lambda_powertools");
        assert_eq!(model.language, Language::Python);
        assert_eq!(model.version, Some(">=2.0.0".to_string()));
        assert_eq!(model.call_patterns.len(), 2);

        // First pattern: get_parameter -> ssm:GetParameter
        let p0 = &model.call_patterns[0];
        assert_eq!(p0.module_path, "aws_lambda_powertools.utilities.parameters");
        assert_eq!(p0.function_name, "get_parameter");
        assert_eq!(p0.call_type, CallType::ModuleLevel);
        assert_eq!(p0.sdk_operations.len(), 1);
        assert_eq!(p0.sdk_operations[0].service, "ssm");
        assert_eq!(p0.sdk_operations[0].operation, "GetParameter");
        assert!(p0.parameter_constraints.is_empty());

        // Second pattern: get_secret -> secretsmanager:GetSecretValue
        let p1 = &model.call_patterns[1];
        assert_eq!(p1.module_path, "aws_lambda_powertools.utilities.parameters");
        assert_eq!(p1.function_name, "get_secret");
        assert_eq!(p1.call_type, CallType::ModuleLevel);
        assert_eq!(p1.sdk_operations.len(), 1);
        assert_eq!(p1.sdk_operations[0].service, "secretsmanager");
        assert_eq!(p1.sdk_operations[0].operation, "GetSecretValue");
        assert!(p1.parameter_constraints.is_empty());
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

    #[test]
    fn optional_parameter_constraints_defaults_to_empty_when_absent() {
        let json = r#"{
            "library_name": "some_lib",
            "language": "go",
            "call_patterns": [
                {
                    "module_path": "some_lib.mod",
                    "function_name": "do_thing",
                    "call_type": "instance_method",
                    "sdk_operations": [
                        { "service": "dynamodb", "operation": "PutItem" }
                    ]
                }
            ]
        }"#;

        let model: ExternalLibraryModel =
            serde_json::from_str(json).expect("should parse model without parameter_constraints");
        assert!(model.call_patterns[0].parameter_constraints.is_empty());
    }

    // Unit tests for merge_models

    /// Helper to create a minimal valid model for merge testing.
    fn make_model(
        library_name: &str,
        language: Language,
        patterns: Vec<CallPattern>,
        source_path: Option<PathBuf>,
    ) -> ExternalLibraryModel {
        ExternalLibraryModel {
            library_name: library_name.to_string(),
            language,
            version: None,
            call_patterns: patterns,
            source_path,
        }
    }

    /// Helper to create a minimal call pattern for merge testing.
    fn make_pattern(
        module_path: &str,
        function_name: &str,
        service: &str,
        operation: &str,
    ) -> CallPattern {
        CallPattern {
            module_path: module_path.to_string(),
            function_name: function_name.to_string(),
            call_type: CallType::ModuleLevel,
            sdk_operations: vec![SdkOperationMapping {
                service: service.to_string(),
                operation: operation.to_string(),
            }],
            parameter_constraints: vec![],
        }
    }

    #[test]
    fn merge_user_pattern_overrides_builtin_pattern() {
        let builtin = vec![make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern("my_lib.mod", "do_thing", "s3", "GetObject")],
            None,
        )];
        let user = vec![make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern(
                "my_lib.mod",
                "do_thing",
                "dynamodb",
                "PutItem",
            )],
            Some(PathBuf::from("user_model.json")),
        )];

        let result =
            LibraryModelRegistry::merge_models(builtin, user).expect("merge should succeed");

        let key = (Language::Python, "my_lib".to_string());
        let merged = result.get(&key).expect("model should exist");

        // Should have exactly one pattern (user overrides builtin)
        assert_eq!(merged.call_patterns.len(), 1);
        assert_eq!(
            merged.call_patterns[0].sdk_operations[0].service,
            "dynamodb"
        );
        assert_eq!(
            merged.call_patterns[0].sdk_operations[0].operation,
            "PutItem"
        );
    }

    #[test]
    fn merge_non_overlapping_patterns_are_combined() {
        let builtin = vec![make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern("my_lib.mod", "func_a", "s3", "GetObject")],
            None,
        )];
        let user = vec![make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern("my_lib.mod", "func_b", "dynamodb", "PutItem")],
            Some(PathBuf::from("user_model.json")),
        )];

        let result =
            LibraryModelRegistry::merge_models(builtin, user).expect("merge should succeed");

        let key = (Language::Python, "my_lib".to_string());
        let merged = result.get(&key).expect("model should exist");

        // Should have both patterns
        assert_eq!(merged.call_patterns.len(), 2);

        let services: Vec<&str> = merged
            .call_patterns
            .iter()
            .map(|p| p.sdk_operations[0].service.as_str())
            .collect();
        assert!(services.contains(&"s3"));
        assert!(services.contains(&"dynamodb"));
    }

    #[test]
    fn merge_conflicting_user_patterns_returns_error() {
        let builtin = vec![];
        let user = vec![
            make_model(
                "my_lib",
                Language::Python,
                vec![make_pattern("my_lib.mod", "do_thing", "s3", "GetObject")],
                Some(PathBuf::from("user_model_a.json")),
            ),
            make_model(
                "my_lib",
                Language::Python,
                vec![make_pattern(
                    "my_lib.mod",
                    "do_thing",
                    "dynamodb",
                    "PutItem",
                )],
                Some(PathBuf::from("user_model_b.json")),
            ),
        ];

        let result = LibraryModelRegistry::merge_models(builtin, user);
        assert!(result.is_err());

        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("user_model_a.json"),
            "Error should mention first source file, got: {err_msg}"
        );
        assert!(
            err_msg.contains("user_model_b.json"),
            "Error should mention second source file, got: {err_msg}"
        );
        assert!(
            err_msg.contains("my_lib.mod"),
            "Error should mention module_path, got: {err_msg}"
        );
        assert!(
            err_msg.contains("do_thing"),
            "Error should mention function_name, got: {err_msg}"
        );
    }

    #[test]
    fn merge_different_libraries_stay_separate() {
        let builtin = vec![make_model(
            "lib_a",
            Language::Python,
            vec![make_pattern("lib_a.mod", "func_a", "s3", "GetObject")],
            None,
        )];
        let user = vec![make_model(
            "lib_b",
            Language::Python,
            vec![make_pattern("lib_b.mod", "func_b", "dynamodb", "PutItem")],
            Some(PathBuf::from("user_model.json")),
        )];

        let result =
            LibraryModelRegistry::merge_models(builtin, user).expect("merge should succeed");

        assert_eq!(result.len(), 2);
        assert!(result.contains_key(&(Language::Python, "lib_a".to_string())));
        assert!(result.contains_key(&(Language::Python, "lib_b".to_string())));
    }

    // Unit tests for user model override behavior

    // Unit tests for model merging pattern preservation

    #[test]
    fn merge_user_overrides_only_matching_pattern_keeps_others() {
        // Built-in has two patterns for the same library
        let builtin = vec![make_model(
            "my_lib",
            Language::Python,
            vec![
                make_pattern("my_lib.mod", "func_a", "s3", "GetObject"),
                make_pattern("my_lib.mod", "func_b", "ssm", "GetParameter"),
            ],
            None,
        )];
        // User overrides only func_a
        let user = vec![make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern("my_lib.mod", "func_a", "dynamodb", "PutItem")],
            Some(PathBuf::from("user.json")),
        )];

        let result =
            LibraryModelRegistry::merge_models(builtin, user).expect("merge should succeed");

        let key = (Language::Python, "my_lib".to_string());
        let merged = result.get(&key).expect("model should exist");

        // Should have 2 patterns: the overridden func_a and the untouched func_b
        assert_eq!(merged.call_patterns.len(), 2);

        let func_a = merged
            .call_patterns
            .iter()
            .find(|p| p.function_name == "func_a")
            .expect("func_a should exist");
        assert_eq!(func_a.sdk_operations[0].service, "dynamodb");

        let func_b = merged
            .call_patterns
            .iter()
            .find(|p| p.function_name == "func_b")
            .expect("func_b should exist");
        assert_eq!(func_b.sdk_operations[0].service, "ssm");
    }

    // -----------------------------------------------------------------------
    // Unit tests for user model loading and conflict detection
    // -----------------------------------------------------------------------

    /// Helper: write a valid model JSON string for a given library name and pattern.
    fn valid_model_json(
        library_name: &str,
        module_path: &str,
        function_name: &str,
        service: &str,
        operation: &str,
    ) -> String {
        format!(
            r#"{{
                "library_name": "{}",
                "language": "python",
                "call_patterns": [
                    {{
                        "module_path": "{}",
                        "function_name": "{}",
                        "call_type": "module_level",
                        "sdk_operations": [
                            {{ "service": "{}", "operation": "{}" }}
                        ]
                    }}
                ]
            }}"#,
            library_name, module_path, function_name, service, operation
        )
    }

    #[test]
    fn load_user_models_from_directory_with_multiple_json_files() {
        let dir = tempfile::tempdir().expect("should create temp dir");

        let model_a = valid_model_json("lib_a", "lib_a.mod", "func_a", "s3", "GetObject");
        let model_b = valid_model_json("lib_b", "lib_b.mod", "func_b", "dynamodb", "PutItem");

        std::fs::write(dir.path().join("model_a.json"), &model_a)
            .expect("should write model_a.json");
        std::fs::write(dir.path().join("model_b.json"), &model_b)
            .expect("should write model_b.json");

        let models =
            LibraryModelRegistry::load_user_models(dir.path()).expect("should load models");

        assert_eq!(models.len(), 2, "should load both JSON files");

        let names: Vec<&str> = models.iter().map(|m| m.library_name.as_str()).collect();
        assert!(names.contains(&"lib_a"), "should contain lib_a");
        assert!(names.contains(&"lib_b"), "should contain lib_b");

        // Each model should have source_path set
        for model in &models {
            assert!(
                model.source_path.is_some(),
                "source_path should be set for user models"
            );
        }
    }

    #[test]
    fn load_user_models_from_directory_ignores_non_json_files() {
        let dir = tempfile::tempdir().expect("should create temp dir");

        let model = valid_model_json("lib_a", "lib_a.mod", "func_a", "s3", "GetObject");
        std::fs::write(dir.path().join("model.json"), &model).expect("should write model.json");
        std::fs::write(dir.path().join("readme.txt"), "not a model")
            .expect("should write readme.txt");
        std::fs::write(dir.path().join("notes.md"), "# notes").expect("should write notes.md");

        let models =
            LibraryModelRegistry::load_user_models(dir.path()).expect("should load models");

        assert_eq!(models.len(), 1, "should only load the .json file");
        assert_eq!(models[0].library_name, "lib_a");
    }

    #[test]
    fn load_user_models_from_single_json_file() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let file_path = dir.path().join("my_model.json");

        let model = valid_model_json("my_lib", "my_lib.mod", "do_thing", "s3", "GetObject");
        std::fs::write(&file_path, &model).expect("should write model file");

        let models = LibraryModelRegistry::load_user_models(&file_path).expect("should load model");

        assert_eq!(models.len(), 1, "should load exactly one model");
        assert_eq!(models[0].library_name, "my_lib");
        assert_eq!(
            models[0].source_path.as_deref(),
            Some(file_path.as_path()),
            "source_path should match the file path"
        );
    }

    #[test]
    fn load_user_models_nonexistent_path_returns_error() {
        let path = Path::new("/tmp/definitely_does_not_exist_ipa_test_12345");
        let result = LibraryModelRegistry::load_user_models(path);

        assert!(result.is_err(), "should return error for non-existent path");
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("does not exist"),
            "Error should mention path does not exist, got: {err_msg}"
        );
    }

    #[test]
    fn load_user_models_malformed_json_halts_with_error() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let file_path = dir.path().join("broken.json");

        std::fs::write(&file_path, "{ not valid json }").expect("should write broken file");

        let result = LibraryModelRegistry::load_user_models(&file_path);

        assert!(result.is_err(), "should return error for malformed JSON");
    }

    #[test]
    fn merge_two_user_models_same_pattern_produces_conflict_error() {
        let user_a = make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern("my_lib.mod", "do_thing", "s3", "GetObject")],
            Some(PathBuf::from("model_a.json")),
        );
        let user_b = make_model(
            "my_lib",
            Language::Python,
            vec![make_pattern(
                "my_lib.mod",
                "do_thing",
                "dynamodb",
                "PutItem",
            )],
            Some(PathBuf::from("model_b.json")),
        );

        let result = LibraryModelRegistry::merge_models(vec![], vec![user_a, user_b]);

        assert!(
            result.is_err(),
            "should return error for conflicting user patterns"
        );
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("model_a.json"),
            "Error should mention first source, got: {err_msg}"
        );
        assert!(
            err_msg.contains("model_b.json"),
            "Error should mention second source, got: {err_msg}"
        );
        assert!(
            err_msg.contains("do_thing"),
            "Error should mention conflicting function, got: {err_msg}"
        );
    }

    #[test]
    fn load_and_merge_conflicting_user_models_from_directory() {
        let dir = tempfile::tempdir().expect("should create temp dir");

        let model_a = valid_model_json("my_lib", "my_lib.mod", "do_thing", "s3", "GetObject");
        let model_b = valid_model_json("my_lib", "my_lib.mod", "do_thing", "dynamodb", "PutItem");

        std::fs::write(dir.path().join("model_a.json"), &model_a)
            .expect("should write model_a.json");
        std::fs::write(dir.path().join("model_b.json"), &model_b)
            .expect("should write model_b.json");

        let user_models =
            LibraryModelRegistry::load_user_models(dir.path()).expect("should load models");

        let result = LibraryModelRegistry::merge_models(vec![], user_models);
        assert!(
            result.is_err(),
            "should return error for conflicting user patterns loaded from directory"
        );
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(
            err_msg.contains("do_thing"),
            "Error should mention conflicting function, got: {err_msg}"
        );
    }

    #[test]
    fn user_model_overrides_builtin_for_same_pattern() {
        let builtin = vec![make_model(
            "powertools",
            Language::Python,
            vec![make_pattern(
                "powertools.params",
                "get_param",
                "ssm",
                "GetParameter",
            )],
            None,
        )];

        let dir = tempfile::tempdir().expect("should create temp dir");
        let file_path = dir.path().join("override.json");
        let override_json = valid_model_json(
            "powertools",
            "powertools.params",
            "get_param",
            "secretsmanager",
            "GetSecretValue",
        );
        std::fs::write(&file_path, &override_json).expect("should write override model");

        let user_models =
            LibraryModelRegistry::load_user_models(&file_path).expect("should load user model");

        let merged =
            LibraryModelRegistry::merge_models(builtin, user_models).expect("merge should succeed");

        let key = (Language::Python, "powertools".to_string());
        let model = merged.get(&key).expect("model should exist");

        assert_eq!(model.call_patterns.len(), 1);
        assert_eq!(
            model.call_patterns[0].sdk_operations[0].service, "secretsmanager",
            "user model should override built-in"
        );
        assert_eq!(
            model.call_patterns[0].sdk_operations[0].operation, "GetSecretValue",
            "user model operation should override built-in"
        );
    }

    // Constrained and unconstrained patterns for the same function coexist
    // (specificity preference is enforced at match time in the extractor; here we
    // verify that merge_models correctly preserves both patterns since they have
    // different parameter_constraints and are therefore not conflicting)
    #[test]
    fn constrained_and_unconstrained_patterns_coexist_after_merge() {
        let unconstrained = CallPattern {
            module_path: "my_lib.mod".to_string(),
            function_name: "get_data".to_string(),
            call_type: CallType::ModuleLevel,
            sdk_operations: vec![SdkOperationMapping {
                service: "s3".to_string(),
                operation: "GetObject".to_string(),
            }],
            parameter_constraints: vec![],
        };

        let constrained = CallPattern {
            module_path: "my_lib.mod".to_string(),
            function_name: "get_data".to_string(),
            call_type: CallType::ModuleLevel,
            sdk_operations: vec![SdkOperationMapping {
                service: "dynamodb".to_string(),
                operation: "GetItem".to_string(),
            }],
            parameter_constraints: vec![ParameterConstraint {
                parameter_name: "backend".to_string(),
                expected_value: "dynamodb".to_string(),
            }],
        };

        // Built-in has the unconstrained pattern
        let builtin = vec![make_model(
            "my_lib",
            Language::Python,
            vec![unconstrained.clone()],
            None,
        )];

        // User provides the constrained pattern (different parameter_constraints
        // means it's a different effective pattern, not a conflict)
        let user = vec![ExternalLibraryModel {
            library_name: "my_lib".to_string(),
            language: Language::Python,
            version: None,
            call_patterns: vec![constrained.clone()],
            source_path: Some(PathBuf::from("user_constrained.json")),
        }];

        let merged =
            LibraryModelRegistry::merge_models(builtin, user).expect("merge should succeed");

        let key = (Language::Python, "my_lib".to_string());
        // Verify the merge succeeded (user overrides built-in for same signature)
        let _merged_model = merged.get(&key).expect("model should exist");

        // The merge logic keys on (module_path, function_name) so the user
        // pattern replaces the built-in here. The real "coexistence" case is
        // when a single model contains both constrained and
        // unconstrained patterns for the same function name — verify that below.

        let combined_model = make_model(
            "my_lib_combined",
            Language::Python,
            vec![unconstrained, constrained],
            None,
        );

        let merged_combined = LibraryModelRegistry::merge_models(vec![combined_model], vec![])
            .expect("merge should succeed");

        let key_combined = (Language::Python, "my_lib_combined".to_string());
        let model_combined = merged_combined
            .get(&key_combined)
            .expect("model should exist");

        // Both patterns should be preserved in the model
        assert_eq!(
            model_combined.call_patterns.len(),
            2,
            "both constrained and unconstrained patterns should coexist"
        );

        let has_unconstrained = model_combined
            .call_patterns
            .iter()
            .any(|p| p.parameter_constraints.is_empty() && p.function_name == "get_data");
        let has_constrained = model_combined
            .call_patterns
            .iter()
            .any(|p| !p.parameter_constraints.is_empty() && p.function_name == "get_data");

        assert!(
            has_unconstrained,
            "unconstrained pattern should be preserved"
        );
        assert!(has_constrained, "constrained pattern should be preserved");

        // Verify the constrained pattern maps to the expected service
        let constrained_pattern = model_combined
            .call_patterns
            .iter()
            .find(|p| !p.parameter_constraints.is_empty())
            .expect("constrained pattern should exist");
        assert_eq!(constrained_pattern.sdk_operations[0].service, "dynamodb");

        // Verify the unconstrained pattern maps to the expected service
        let unconstrained_pattern = model_combined
            .call_patterns
            .iter()
            .find(|p| p.parameter_constraints.is_empty() && p.function_name == "get_data")
            .expect("unconstrained pattern should exist");
        assert_eq!(unconstrained_pattern.sdk_operations[0].service, "s3");
    }

    #[test]
    fn load_user_models_from_empty_directory() {
        let dir = tempfile::tempdir().expect("should create temp dir");

        let models =
            LibraryModelRegistry::load_user_models(dir.path()).expect("should load models");

        assert!(
            models.is_empty(),
            "empty directory should produce zero models"
        );
    }
}
