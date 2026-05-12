//! Shared utilities for JavaScript and TypeScript AWS SDK extraction
//!
//! This module contains common functionality shared between JavaScript and TypeScript
//! extractors.

use std::collections::HashSet;

use crate::extraction::javascript::types::{ImportInfo, JavaScriptScanResults};
use crate::extraction::{Parameter, ParameterValue, SdkMethodCall, SdkMethodCallMetadata};
use crate::Location;
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;

/// Embedded JavaScript SDK v3 libraries mapping
///
/// This struct provides access to the JavaScript SDK v3 libraries mapping configuration
/// that defines how lib-* submodule commands map to client-* commands.
#[derive(RustEmbed)]
#[folder = "resources/config/sdks"]
#[include = "js_v3_libraries.json"]
struct JsV3Libraries;

impl JsV3Libraries {
    /// Get the JavaScript SDK v3 libraries mapping configuration
    fn get_libraries_mapping() -> Option<std::borrow::Cow<'static, [u8]>> {
        Self::get("js_v3_libraries.json").map(|file| file.data)
    }
}

/// JSON structure for JS v3 libraries mapping
///
/// The AWS SDK fro Javascript defines in aws-sdk-js-v3/lib
/// some auxiliary libraries that export common utility operations
/// such as Upload. Behind the scenes, these utilities may call different
/// SDK methods. This map associate to each library name a map from
/// a utility operation to a list of SDK methods the utility operation
/// may invoke.
#[derive(Debug, Deserialize)]
struct JsV3LibrariesMapping {
    #[serde(flatten)]
    library_operation_expansions: HashMap<String, HashMap<String, Vec<String>>>,
}

/// Load JS v3 libraries mapping from embedded data
fn load_libraries_mapping() -> Option<JsV3LibrariesMapping> {
    let content_bytes = JsV3Libraries::get_libraries_mapping()?;

    let content = std::str::from_utf8(&content_bytes).ok()?;

    serde_json::from_str(content).ok()
}

/// Result of finding a command/function instantiation with its arguments
#[derive(Debug, Clone)]
pub(crate) struct CommandUsage<'a> {
    /// The matched text from the AST
    pub(crate) text: Cow<'a, str>,
    /// Location where the command usage was found
    pub(crate) location: Location,
    /// Extracted parameters from the command/function arguments
    pub(crate) parameters: Vec<crate::extraction::Parameter>,
}

impl<'a> CommandUsage<'a> {
    /// Create a new CommandInstantiationResult
    pub(crate) fn new(
        text: Cow<'a, str>,
        location: Location,
        parameters: Vec<crate::extraction::Parameter>,
    ) -> Self {
        Self {
            text,
            location,
            parameters,
        }
    }
}

// Used when we cannot find a method call, and fall back to adding an operation purely based on an import statement
impl From<&ImportInfo> for CommandUsage<'_> {
    fn from(value: &ImportInfo) -> Self {
        Self {
            text: Cow::Owned(value.statement.clone()),
            location: value.location.clone(),
            // TODO: parameters should be an Option, so we can distinguish
            // the case where we fall back to an import statement
            parameters: vec![],
        }
    }
}

/// Shared extraction utilities for JavaScript/TypeScript AWS SDK method calls
pub(crate) struct ExtractionUtils;

impl ExtractionUtils {
    /// Extract operations from imported types and their usage patterns
    pub(crate) fn extract_operations_from_imports<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut method_calls = Vec::new();
        let mut handled_names = HashSet::new();

        // Load library mappings once for reuse across all extraction functions
        let lib_mappings = load_libraries_mapping();

        // Extract operations from Command imports (e.g., PutObjectCommand -> PutObject operation)
        method_calls.extend(Self::extract_command_operations(
            scan_results,
            scanner,
            lib_mappings.as_ref(),
            &mut handled_names,
        ));

        // Extract operations from paginate function imports (e.g., paginateQuery -> Query operation)
        method_calls.extend(Self::extract_paginate_operations(
            scan_results,
            scanner,
            lib_mappings.as_ref(),
            &mut handled_names,
        ));

        // Extract operations from waiter function imports (e.g., waitUntilBucketExists -> BucketExists waiter)
        method_calls.extend(Self::extract_waiter_operations(
            scan_results,
            scanner,
            &mut handled_names,
        ));

        // Extract operations from CommandInput imports (e.g., QueryCommandInput -> Query operation)
        method_calls.extend(Self::extract_command_input_operations(
            scan_results,
            scanner,
            &mut handled_names,
        ));

        // Extract operations from generic lib-* class imports (e.g., Upload -> multiple S3 commands)
        // Uses handled_names to skip names already processed by the extractors above
        method_calls.extend(Self::extract_library_class_operations(
            scan_results,
            scanner,
            lib_mappings.as_ref(),
            &handled_names,
        ));

        method_calls
    }

    /// Extract operations from Command type imports and their constructor usage
    fn extract_command_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
        lib_mappings: Option<&JsV3LibrariesMapping>,
        handled_names: &mut HashSet<String>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find Command types
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) =
                    Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary)
                else {
                    continue;
                };

                let is_lib = sublibrary_info.sublibrary.starts_with("lib-");

                // Named imports (e.g., import { CreateBucketCommand } from '@aws-sdk/client-s3')
                for import_info in &sublibrary_info.imports {
                    // Check if this is a Command type (ends with "Command")
                    if import_info.original_name.ends_with("Command") {
                        handled_names.insert(import_info.original_name.clone());
                        // Try to find the actual constructor instantiation with arguments
                        // Use the local name for the search (handles renames)
                        let result = scanner
                            .find_command_instantiation_with_args(&import_info.local_name)
                            .unwrap_or_else(|| import_info.into()); // Fallback to import position with no params

                        // Check if this needs library expansion (lib-* sublibraries)
                        let expanded = Self::expand_lib_names(
                            is_lib,
                            &service,
                            &import_info.original_name,
                            lib_mappings,
                        );

                        // Create operations for each expanded command name
                        for command_name in expanded {
                            // Extract operation name by removing "Command" suffix
                            if let Some(operation_name) = command_name.strip_suffix("Command") {
                                operations.push(Self::build_sdk_method_call(
                                    operation_name,
                                    &service,
                                    &result,
                                ));
                            }
                        }
                    }
                }

                // Wildcard imports (e.g., import * as S3 from '@aws-sdk/client-s3')
                if let Some(namespace) = &sublibrary_info.wildcard_namespace {
                    for (command_name, usage) in
                        scanner.find_namespace_command_with_args(namespace, "Command")
                    {
                        handled_names.insert(command_name.clone());
                        let expanded =
                            Self::expand_lib_names(is_lib, &service, &command_name, lib_mappings);
                        for name in expanded {
                            if let Some(op) = name.strip_suffix("Command") {
                                operations.push(Self::build_sdk_method_call(op, &service, &usage));
                            }
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from paginate function imports
    fn extract_paginate_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
        lib_mappings: Option<&JsV3LibrariesMapping>,
        handled_names: &mut HashSet<String>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find paginate functions
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) =
                    Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary)
                else {
                    continue;
                };

                let is_lib = sublibrary_info.sublibrary.starts_with("lib-");

                // Named imports (e.g., import { paginateListObjectsV2 } from '@aws-sdk/client-s3')
                for import_info in &sublibrary_info.imports {
                    // Check if this is a paginate function (starts with "paginate")
                    if import_info.original_name.starts_with("paginate") {
                        handled_names.insert(import_info.original_name.clone());
                        // Try to find the actual paginate function call with arguments
                        // Use the local name for the search (handles renames)
                        let result = scanner
                            .find_paginate_function_with_args(&import_info.local_name)
                            .unwrap_or_else(|| import_info.into()); // Fallback to import position with no params

                        // Check if this needs library expansion (lib-* sublibraries)
                        let expanded = Self::expand_lib_names(
                            is_lib,
                            &service,
                            &import_info.original_name,
                            lib_mappings,
                        );

                        for name in expanded {
                            let operation_name = name
                                .strip_suffix("Command")
                                .or_else(|| name.strip_prefix("paginate"))
                                .unwrap_or(&name);
                            operations.push(Self::build_sdk_method_call(
                                operation_name,
                                &service,
                                &result,
                            ));
                        }
                    }
                }

                // Wildcard imports (e.g., import * as S3 from '@aws-sdk/client-s3')
                if let Some(namespace) = &sublibrary_info.wildcard_namespace {
                    for (paginator_name, usage) in
                        scanner.find_namespace_paginate_with_args(namespace)
                    {
                        handled_names.insert(paginator_name.clone());
                        let expanded =
                            Self::expand_lib_names(is_lib, &service, &paginator_name, lib_mappings);
                        for name in expanded {
                            let op = name
                                .strip_suffix("Command")
                                .or_else(|| name.strip_prefix("paginate"))
                                .unwrap_or(&name);
                            operations.push(Self::build_sdk_method_call(op, &service, &usage));
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from waiter function imports
    /// Waiters like `waitUntilBucketExists` map to underlying operations like `HeadBucket`
    /// The waiter name is extracted here; actual operation resolution happens in filter_map
    pub(crate) fn extract_waiter_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
        handled_names: &mut HashSet<String>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find waiter functions
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) =
                    Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary)
                else {
                    continue;
                };

                // Named imports (e.g., import { waitUntilBucketExists } from '@aws-sdk/client-s3')
                for import_info in &sublibrary_info.imports {
                    // Check if this is a waiter function (starts with "waitUntil")
                    if let Some(waiter_name) = import_info.original_name.strip_prefix("waitUntil") {
                        handled_names.insert(import_info.original_name.clone());
                        // Try to find the actual waiter function call with arguments
                        // Use the local name for the search (handles renames)
                        let result = scanner
                            .find_waiter_function_with_args(&import_info.local_name)
                            .unwrap_or_else(|| import_info.into()); // Fallback to import position with no params

                        // Keep PascalCase waiter name
                        // e.g., "BucketExists" from "waitUntilBucketExists"
                        // This will be resolved to the actual operation (e.g., "HeadBucket") in filter_map
                        operations.push(Self::build_sdk_method_call(
                            waiter_name,
                            &service,
                            &result,
                        ));
                    }
                }

                // Wildcard imports (e.g., import * as S3 from '@aws-sdk/client-s3')
                if let Some(namespace) = &sublibrary_info.wildcard_namespace {
                    for (waiter_name, usage) in scanner.find_namespace_waiter_with_args(namespace) {
                        handled_names.insert(waiter_name.clone());
                        if let Some(waiter_state) = waiter_name.strip_prefix("waitUntil") {
                            operations.push(Self::build_sdk_method_call(
                                waiter_state,
                                &service,
                                &usage,
                            ));
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from CommandInput type imports
    pub(crate) fn extract_command_input_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
        handled_names: &mut HashSet<String>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut operations = Vec::new();

        // Process both imports and requires to find CommandInput types
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Skip sublibraries that don't match known patterns
                let Some(service) =
                    Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary)
                else {
                    continue;
                };

                for import_info in &sublibrary_info.imports {
                    // Check if this is a CommandInput or Input type
                    let operation_name = if import_info.original_name.ends_with("CommandInput") {
                        handled_names.insert(import_info.original_name.clone());
                        // Extract operation name by removing "CommandInput" suffix
                        import_info.original_name.strip_suffix("CommandInput")
                    } else {
                        None
                    };

                    if let Some(operation_name) = operation_name {
                        // Try to find the actual CommandInput type usage position (TypeScript-specific)
                        // Use the local name for the search (handles renames)
                        let result = scanner
                            .find_command_input_usage_position(&import_info.local_name)
                            .unwrap_or_else(|| import_info.into()); // Fallback to import position with no params

                        // Keep PascalCase operation name to match service index
                        // e.g., "Query" stays "Query"
                        let metadata = SdkMethodCallMetadata::new(
                            result.text.to_string(),
                            result.location.clone(),
                        );

                        let method_call = SdkMethodCall {
                            name: operation_name.to_string(),
                            possible_services: vec![service.clone()],
                            metadata: Some(metadata),
                        };
                        operations.push(method_call);
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from generic lib-* class imports (e.g., Upload -> multiple S3 commands)
    /// This handles library classes that don't match Command/paginate/waitUntil/CommandInput patterns
    fn extract_library_class_operations<T>(
        scan_results: &JavaScriptScanResults,
        scanner: &mut crate::extraction::javascript::scanner::ASTScanner<T>,
        lib_mappings: Option<&JsV3LibrariesMapping>,
        handled_names: &HashSet<String>,
    ) -> Vec<SdkMethodCall>
    where
        T: ast_grep_language::LanguageExt,
    {
        let mut operations = Vec::new();

        // Early return if no library mappings available
        let Some(lib_mappings) = lib_mappings else {
            return operations;
        };

        // Process both imports and requires to find generic lib-* classes
        for import_source in [&scan_results.imports, &scan_results.requires] {
            for sublibrary_info in import_source {
                // Only process lib-* sublibraries
                if !sublibrary_info.sublibrary.starts_with("lib-") {
                    continue;
                }

                let Some(service) =
                    Self::extract_service_from_sublibrary(&sublibrary_info.sublibrary)
                else {
                    continue;
                };

                // Named imports (e.g., import { Upload } from '@aws-sdk/lib-storage')
                for import_info in &sublibrary_info.imports {
                    if handled_names.contains(&import_info.original_name) {
                        continue;
                    }

                    // Check if this class has a mapping
                    if let Some(expanded_commands) = lib_mappings
                        .library_operation_expansions
                        .get(&service)
                        .and_then(|lib| lib.get(&import_info.original_name))
                    {
                        // Try to find class instantiation, fallback to import position
                        let result = scanner
                            .find_command_instantiation_with_args(&import_info.local_name)
                            .unwrap_or_else(|| import_info.into()); // Fallback to import position with no params

                        // Create operations for each expanded command
                        for command_name in expanded_commands {
                            // Extract operation name by removing "Command" suffix
                            if let Some(operation_name) = command_name.strip_suffix("Command") {
                                operations.push(Self::build_sdk_method_call(
                                    operation_name,
                                    &service,
                                    &result,
                                ));
                            }
                        }
                    }
                }

                // Wildcard imports (e.g., import * as S3Storage from '@aws-sdk/lib-storage')
                // Handles namespace-prefixed classes like S3Storage.Upload(...)
                if let Some(namespace) = &sublibrary_info.wildcard_namespace {
                    let service_mappings = lib_mappings.library_operation_expansions.get(&service);

                    if let Some(service_mappings) = service_mappings {
                        for (class_name, expanded_commands) in service_mappings {
                            if handled_names.contains(class_name) {
                                continue;
                            }

                            // Scan for `new NS.ClassName($$$ARGS)`
                            let usages =
                                scanner.find_namespace_command_with_args(namespace, class_name);
                            for (_matched_name, usage) in usages {
                                for command_name in expanded_commands {
                                    if let Some(op) = command_name.strip_suffix("Command") {
                                        operations.push(Self::build_sdk_method_call(
                                            op, &service, &usage,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        operations
    }

    /// Extract operations from direct client method calls (e.g., client.getObject())
    pub(crate) fn extract_operations_from_method_calls(
        scan_results: &JavaScriptScanResults,
    ) -> Vec<SdkMethodCall> {
        let mut operations = Vec::new();

        // Process method calls to find direct operations on clients
        for method_call in &scan_results.method_calls {
            // Skip send method calls (handled separately)
            if method_call.method_name == "send" {
                continue;
            }

            // Skip method calls from sublibraries that don't match known patterns
            let Some(service) =
                Self::extract_service_from_sublibrary(&method_call.client_sublibrary)
            else {
                continue;
            };

            // Convert camelCase to PascalCase to match service index
            // e.g., "getObject" -> "GetObject"
            let operation_name = Self::camel_case_to_pascal_case(&method_call.method_name);

            // Convert method arguments to parameters
            let parameters = Self::convert_arguments_to_parameters(&method_call.arguments);

            let metadata =
                SdkMethodCallMetadata::new(method_call.expr.clone(), method_call.location.clone())
                    .with_parameters(parameters)
                    .with_receiver(method_call.client_variable.clone());

            let sdk_method_call = SdkMethodCall {
                name: operation_name,
                possible_services: vec![service],
                metadata: Some(metadata),
            };

            operations.push(sdk_method_call);
        }

        operations
    }

    /// Convert camelCase to PascalCase for method names
    /// e.g., "getObject" -> "GetObject", "listTables" -> "ListTables"
    pub(crate) fn camel_case_to_pascal_case(input: &str) -> String {
        if input.is_empty() {
            return input.to_string();
        }

        let mut chars = input.chars();
        if let Some(first_char) = chars.next() {
            first_char.to_uppercase().collect::<String>() + chars.as_str()
        } else {
            input.to_string()
        }
    }

    /// Extract service name from sublibrary name
    /// Returns Some(service) if the sublibrary matches a known pattern, None otherwise
    pub(crate) fn extract_service_from_sublibrary(sublibrary: &str) -> Option<String> {
        // Handle common patterns:
        // "client-s3" -> Some("s3")
        // "lib-dynamodb" -> Some("dynamodb")
        // "client-lambda" -> Some("lambda")
        if let Some(service) = sublibrary.strip_prefix("client-") {
            Some(service.to_string())
        } else {
            sublibrary
                .strip_prefix("lib-")
                .map(std::string::ToString::to_string)
        }
    }

    /// Convert argument HashMap to Parameter vector
    pub(crate) fn convert_arguments_to_parameters(
        arguments: &HashMap<String, String>,
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();

        // Convert each argument to a keyword parameter
        for (position, (name, value)) in arguments.iter().enumerate() {
            parameters.push(Parameter::Keyword {
                name: name.clone(),
                value: ParameterValue::Unresolved(value.clone()), // JavaScript values are typically unresolved
                position,
                type_annotation: None, // We don't infer types for JavaScript/TypeScript parameters for now
            });
        }

        parameters
    }

    /// Check if a name matches any of the known AWS SDK Command/paginate/waiter/Input patterns
    /// that are handled by other extractors
    /// Expand a name through lib-* mappings if applicable, or return it as-is for client-* packages.
    fn expand_lib_names(
        is_lib: bool,
        service: &str,
        name: &str,
        lib_mappings: Option<&JsV3LibrariesMapping>,
    ) -> Vec<String> {
        if is_lib {
            lib_mappings
                .and_then(|m| m.library_operation_expansions.get(service))
                .and_then(|lib| lib.get(name))
                .cloned()
                .unwrap_or_else(|| {
                    log::debug!("No mapping found for {service}/{name}, using original name");
                    vec![name.to_string()]
                })
        } else {
            vec![name.to_string()]
        }
    }

    /// Build an `SdkMethodCall` from a matched operation name, service, and usage site.
    fn build_sdk_method_call(
        operation_name: &str,
        service: &str,
        usage: &CommandUsage<'_>,
    ) -> SdkMethodCall {
        let metadata = SdkMethodCallMetadata::new(usage.text.to_string(), usage.location.clone())
            .with_parameters(usage.parameters.clone());

        SdkMethodCall {
            name: operation_name.to_string(),
            possible_services: vec![service.to_string()],
            metadata: Some(metadata),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_service_from_sublibrary() {
        // Test successful pattern matching (Some cases)
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("client-s3"),
            Some("s3".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("lib-dynamodb"),
            Some("dynamodb".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("client-lambda"),
            Some("lambda".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("client-ec2"),
            Some("ec2".to_string())
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("lib-storage"),
            Some("storage".to_string())
        );

        // Test unsuccessful pattern matching (None cases)
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("other"),
            None
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("unknown-prefix-service"),
            None
        );
        assert_eq!(
            ExtractionUtils::extract_service_from_sublibrary("service-s3"),
            None
        );
        assert_eq!(ExtractionUtils::extract_service_from_sublibrary(""), None);
    }

    #[test]
    fn test_convert_arguments_to_parameters() {
        let mut arguments = HashMap::new();
        arguments.insert("Bucket".to_string(), "my-bucket".to_string());
        arguments.insert("Key".to_string(), "my-key".to_string());

        let parameters = ExtractionUtils::convert_arguments_to_parameters(&arguments);
        assert_eq!(parameters.len(), 2);

        // Check that both parameters are keyword parameters
        for param in &parameters {
            match param {
                Parameter::Keyword {
                    name,
                    value,
                    position,
                    ..
                } => {
                    assert!(name == "Bucket" || name == "Key");
                    assert!(value.as_string() == "my-bucket" || value.as_string() == "my-key");
                    assert!(*position < 2);
                }
                _ => panic!("Expected keyword parameter"),
            }
        }
    }

    #[test]
    fn test_camel_case_to_pascal_case() {
        assert_eq!(
            ExtractionUtils::camel_case_to_pascal_case("getObject"),
            "GetObject"
        );
        assert_eq!(
            ExtractionUtils::camel_case_to_pascal_case("listTables"),
            "ListTables"
        );
        assert_eq!(
            ExtractionUtils::camel_case_to_pascal_case("createBucket"),
            "CreateBucket"
        );
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case("query"), "Query");
        assert_eq!(ExtractionUtils::camel_case_to_pascal_case(""), "");
    }
}
