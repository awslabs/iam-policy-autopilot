use std::path::{Path, PathBuf};
use std::sync::Arc;

use log::{info, trace, warn};

use crate::api::model::ServiceHints;
use crate::extraction::sdk_model::ServiceDiscovery;
use crate::extraction::ServiceHintsProcessor;
use crate::service_configuration::load_service_configuration;
use crate::{ExtractedMethods, ExtractionEngine, Language, SourceFile};

use anyhow::{Context, Result};

/// Process source files and extract SDK method calls
pub(crate) async fn process_source_files(
    extractor: &ExtractionEngine,
    source_files: &[PathBuf],
    language_override: Option<&str>,
    service_hints: Option<ServiceHints>,
) -> Result<ExtractedMethods> {
    trace!("Processing {} source files", source_files.len());

    // Log the files being processed
    for (i, file) in source_files.iter().enumerate() {
        trace!("Source file {}: {}", i + 1, file.display());
    }

    // Convert PathBuf to &Path for language detection
    let source_file_paths: Vec<&Path> = source_files
        .iter()
        .map(std::path::PathBuf::as_path)
        .collect();

    // Determine the programming language to use
    let language = if let Some(override_lang) = language_override {
        info!("Using language override: {override_lang}");
        override_lang.to_string()
    } else {
        // Detect and validate language consistency across all files
        let detected_language = extractor
            .detect_and_validate_language(&source_file_paths)
            .context("Failed to detect or validate programming language consistency")?;

        info!("Detected programming language: {detected_language}");
        detected_language.to_string()
    };

    let language = Language::try_from_str(&language)?;

    // Load all source files into SourceFile objects
    let mut loaded_source_files = Vec::new();
    for file_path in source_files {
        let content = std::fs::read_to_string(file_path).context(format!(
            "Failed to read source file: {}",
            file_path.display()
        ))?;

        let source_file = SourceFile::with_language(file_path.clone(), content, language);
        loaded_source_files.push(source_file);
    }

    // Extract SDK method calls from the loaded source files
    let mut results = extractor
        .extract_sdk_method_calls(language, loaded_source_files)
        .await
        .context("Failed to extract SDK method calls from source files")?;

    // If service hints are provided, validate and filter the results
    if let Some(hints) = service_hints {
        // Load service index and configuration for validation
        let service_index = ServiceDiscovery::load_service_index(language).await?;
        let service_config = load_service_configuration()?;

        // Create processor and validate
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));
        processor.validate()?;

        // Filter the results
        processor.filter(&mut results);
    }

    info!(
        "Extraction completed: {} SDK method calls found from {} source files",
        results.methods.len(),
        results.metadata.source_files.len()
    );

    // Log warnings if any
    for warning in &results.metadata.warnings {
        warn!("{warning}");
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::ServiceHints;
    use crate::extraction::{ExtractionMetadata, SdkMethodCall};
    use crate::ExtractedMethods;

    #[tokio::test]
    async fn test_filter_by_service_hints_with_smithy_mapping() {
        // Load service index and configuration
        let service_index = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index");
        let service_config = load_service_configuration().expect("Failed to load config");

        // Create test data with methods for different services
        let results = ExtractedMethods {
            methods: vec![
                SdkMethodCall {
                    name: "put_log_events".to_string(),
                    possible_services: vec!["logs".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "get_item".to_string(),
                    possible_services: vec!["dynamodb".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "post_text".to_string(),
                    possible_services: vec!["lex-runtime".to_string()],
                    metadata: None,
                },
            ],
            metadata: ExtractionMetadata::new(vec![], vec![]),
        };

        // Test 1: Filter using smithy name "cloudwatch-logs" should match "logs"
        // According to service-configuration.json: "cloudwatch-logs" -> "logs"
        let hints = ServiceHints {
            service_names: vec!["cloudwatch-logs".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "cloudwatch-logs hint should match logs service"
        );
        assert_eq!(test_results.methods[0].possible_services[0], "logs");

        // Test 2: Filter using dashless variant "cloudwatchlogs" should also match "logs"
        let hints = ServiceHints {
            service_names: vec!["cloudwatchlogs".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "cloudwatchlogs (dashless) hint should match logs service"
        );
        assert_eq!(test_results.methods[0].possible_services[0], "logs");

        // Test 3: Filter using "lex" should match "lex-runtime"
        // According to service-configuration.json: "lex-runtime-service" -> "lex-runtime"
        let hints = ServiceHints {
            service_names: vec!["lex-runtime-service".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "lex-runtime-service hint should match lex-runtime service"
        );
        assert_eq!(test_results.methods[0].possible_services[0], "lex-runtime");

        // Test 4: Multiple hints should match multiple services
        let hints = ServiceHints {
            service_names: vec!["cloudwatch-logs".to_string(), "dynamodb".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(test_results.methods.len(), 2);

        // Test 5: Direct service name should still work
        let hints = ServiceHints {
            service_names: vec!["logs".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "Direct logs hint should match logs service"
        );
    }

    #[tokio::test]
    async fn test_filter_by_service_hints_with_rename_services() {
        // Load service index and configuration
        let service_index = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index");
        let service_config = load_service_configuration().expect("Failed to load config");

        // Test RenameServicesServiceReference mappings
        // According to service-configuration.json, multiple services map to "bedrock"
        let results = ExtractedMethods {
            methods: vec![
                SdkMethodCall {
                    name: "invoke_model".to_string(),
                    possible_services: vec!["bedrock-runtime".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "create_agent".to_string(),
                    possible_services: vec!["bedrock-agent".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "send_message".to_string(),
                    possible_services: vec!["chime-sdk-messaging".to_string()],
                    metadata: None,
                },
                SdkMethodCall {
                    name: "get_item".to_string(),
                    possible_services: vec!["dynamodb".to_string()],
                    metadata: None,
                },
            ],
            metadata: ExtractionMetadata::new(vec![], vec![]),
        };

        // Test 1: Providing "bedrock" should match both bedrock-runtime and bedrock-agent
        let hints = ServiceHints {
            service_names: vec!["bedrock".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            2,
            "bedrock hint should match both bedrock-runtime and bedrock-agent"
        );

        // Test 2: Providing "chime" should match chime-sdk-messaging
        let hints = ServiceHints {
            service_names: vec!["chime".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "chime hint should match chime-sdk-messaging"
        );
        assert_eq!(
            test_results.methods[0].possible_services[0],
            "chime-sdk-messaging"
        );

        // Test 3: Providing specific service should still work
        let hints = ServiceHints {
            service_names: vec!["bedrock-agent".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            1,
            "bedrock-agent hint should match bedrock-agent service"
        );

        // Test 4: Combined hints with both renamed and direct services
        let hints = ServiceHints {
            service_names: vec!["bedrock".to_string(), "dynamodb".to_string()],
        };
        let mut test_results = results.clone();
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));
        processor.filter(&mut test_results);
        assert_eq!(
            test_results.methods.len(),
            3,
            "bedrock and dynamodb hints should match 3 services total"
        );
    }

    #[tokio::test]
    async fn test_validate_service_hints_with_renamed_services() {
        // Load service index and configuration for validation
        let service_index = ServiceDiscovery::load_service_index(Language::Python)
            .await
            .expect("Failed to load service index");
        let service_config = load_service_configuration().expect("Failed to load config");

        // Test 1: Validate that "chime" is accepted (maps to chime-sdk-* services)
        let hints = ServiceHints {
            service_names: vec!["chime".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "chime should be accepted as it maps to chime-sdk-* services: {:?}",
            result
        );

        // Test 2: Validate that "bedrock" is accepted (maps to bedrock-* services)
        let hints = ServiceHints {
            service_names: vec!["bedrock".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "bedrock should be accepted as it maps to bedrock-* services: {:?}",
            result
        );

        // Test 3: Validate that "cloudwatch-logs" is accepted (Smithy name for "logs")
        let hints = ServiceHints {
            service_names: vec!["cloudwatch-logs".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "cloudwatch-logs should be accepted as it maps to logs: {:?}",
            result
        );

        // Test 4: Validate that dash-less variant "cloudwatchlogs" is accepted
        let hints = ServiceHints {
            service_names: vec!["cloudwatchlogs".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "cloudwatchlogs should be accepted as dash-less variant: {:?}",
            result
        );

        // Test 5: Validate multiple renamed services together
        let hints = ServiceHints {
            service_names: vec![
                "chime".to_string(),
                "bedrock".to_string(),
                "cloudwatch-logs".to_string(),
                "dynamodb".to_string(),
            ],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "Multiple renamed services should all be accepted: {:?}",
            result
        );

        // Test 6: Validate that invalid service still fails
        let hints = ServiceHints {
            service_names: vec!["totally-invalid-service-name-xyz".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config.clone(), Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_err(),
            "Invalid service should still fail validation"
        );

        // Test 7: Validate that specific service names still work
        let hints = ServiceHints {
            service_names: vec!["chime-sdk-messaging".to_string()],
        };
        let processor =
            ServiceHintsProcessor::new(hints, service_config, Arc::clone(&service_index));
        let result = processor.validate();
        assert!(
            result.is_ok(),
            "Specific service name chime-sdk-messaging should be accepted: {:?}",
            result
        );
    }
}
