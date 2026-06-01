//! Shared test macros for Java extraction unit tests.

/// Generate a file-driven `#[rstest]` test for Java extraction.
///
/// # Usage
///
/// ```ignore
/// java_extraction_test!(
///     "tests/java/extractors/imports/*.java",  // glob for rstest #[files(...)]
///     Import,                                  // element type (must impl Deserialize + PartialEq + Debug)
///     imports,                                 // field on ExtractionResult to compare
/// );
/// ```
///
/// The macro derives:
/// - The test function name as `test_java_<result_field>_extraction`
/// - The JSON wrapper field name as `expected_<result_field>`
///
/// The macro generates:
/// - A local `ExpectedOutput` struct with `#[serde(rename_all = "PascalCase")]`
/// - A `#[rstest]` test function that reads `.java` + `.json` pairs, runs extraction,
///   and asserts `result.<result_field> == expected.expected_<result_field>`.
///
/// Extraction order is deterministic (tree-sitter visits nodes in source order), so a
/// plain `Vec` equality check is used — no sorting or set conversion.
#[macro_export]
macro_rules! java_extractor_test {
    (
        $glob:literal,
        $item_ty:ty,
        $result_field:ident
    ) => {
        ::paste::paste! {
            #[derive(Debug, serde::Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct ExpectedOutput {
                [< expected_ $result_field >]: Vec<$item_ty>,
            }

            #[rstest::rstest]
            fn [< test_java_ $result_field _extraction >](
                #[files($glob)] java_file: std::path::PathBuf,
            ) {
                use std::fs;
                use $crate::{Language, SourceFile};
                use $crate::extraction::java::extractor::JavaLanguageExtractorSet;

                let source_code = fs::read_to_string(&java_file)
                    .unwrap_or_else(|e| panic!("Failed to read Java test file {java_file:?}: {e}"));

                let json_file = java_file.with_extension("json");
                let expected_json = fs::read_to_string(&json_file)
                    .unwrap_or_else(|e| panic!("Failed to read expected output file {json_file:?}: {e}"));

                let expected: ExpectedOutput = serde_json::from_str(&expected_json)
                    .unwrap_or_else(|e| panic!("Failed to parse expected output JSON {json_file:?}: {e}"));

                let file_name = java_file.file_name().expect("java_file should have a file name");
                let source = SourceFile::with_language(file_name.into(), source_code, Language::Java);
                let result = JavaLanguageExtractorSet::default_aws_v2()
                    .extract_from_file(&source)
                    .expect("extraction should succeed");

                let test_name = java_file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");

                assert_eq!(
                    result.$result_field,
                    expected.[< expected_ $result_field >],
                    "Test case '{test_name}': {} mismatch",
                    stringify!($result_field),
                );
            }
        }
    };
}

/// Generate a file-driven `#[rstest]` test for Java matching.
///
/// # Test data format
///
/// The glob points to **`.json` descriptor files** (not `.java` files).  Each descriptor
/// lives in a directory that also contains the referenced `.java` source files and a
/// shared service-index JSON file.
///
/// ## Descriptor JSON schema
///
/// ```json
/// {
///   "SourceFiles": ["file_a.java", "file_b.java"],
///   "ServiceIndexFile": "service_index.json",
///   "ExpectedSdkCalls": [
///     {
///       "Name": "putObject",
///       "PossibleServices": ["s3"],
///       "Metadata": {
///         "Expr": "s3.putObject(req)",
///         "Location": "simple_call.java:5.9-5.27",
///         "Parameters": []
///       }
///     }
///   ]
/// }
/// ```
///
/// - `SourceFiles` — one or more `.java` filenames relative to the descriptor's directory.
///   All files are extracted together and their results merged before matching.
/// - `ServiceIndexFile` — path to a shared service-index JSON file, relative to the
///   descriptor's directory.  Multiple descriptors in the same directory may share one
///   index file.
/// - `ExpectedSdkCalls` — the expected [`SdkMethodCall`] list after matching.
///   Each entry is a full [`SdkMethodCall`] including the `Metadata` field.
///
/// ## Service-index JSON schema
///
/// ```json
/// {
///   "Services": { "<service>": { ... SdkServiceDefinition ... } },
///   "MethodLookup": { "<camelCaseMethod>": [{ "ServiceName": "s3", "OperationName": "PutObject" }] },
///   "WaiterLookup": { "<camelCaseWaiterType>": [{ "ServiceName": "s3", "OperationName": "HeadBucket" }] }
/// }
/// ```
///
/// # Usage
///
/// ```ignore
/// java_matcher_test!(
///     "tests/java/matchers/service_calls/*.json",
///     test_service_call_matching,
/// );
/// ```
///
/// The macro generates a `#[rstest]` test function with the given name that:
/// 1. Reads the descriptor JSON
/// 2. Loads the referenced service-index JSON
/// 3. Reads and extracts all referenced `.java` source files
/// 4. Runs [`JavaMatcher::match_calls`] on the merged extraction result
/// 5. Asserts the full [`SdkMethodCall`] output (including `Metadata`) matches `ExpectedSdkCalls`
#[macro_export]
macro_rules! java_matcher_test {
    (
        $glob:literal,
        $test_fn_name:ident
    ) => {
        #[rstest::rstest]
        fn $test_fn_name(#[files($glob)] descriptor_file: std::path::PathBuf) {
            use std::collections::HashMap;
            use std::fs;
            use $crate::extraction::java::extractor::JavaLanguageExtractorSet;
            use $crate::extraction::java::matcher::JavaMatcher;
            use $crate::extraction::java::types::ExtractionResult;
            use $crate::extraction::sdk_model::{
                SdkServiceDefinition, ServiceMethodRef, ServiceModelIndex,
            };
            use $crate::{Language, SdkMethodCall, SourceFile};

            // ── descriptor ────────────────────────────────────────────────────

            #[derive(Debug, serde::Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct Descriptor {
                source_files: Vec<String>,
                service_index_file: String,
                expected_sdk_calls: Vec<SdkMethodCall>,
            }

            // ── service-index JSON ────────────────────────────────────────────

            #[derive(Debug, serde::Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct ServiceIndexJson {
                #[serde(default)]
                services: HashMap<String, SdkServiceDefinition>,
                #[serde(default)]
                method_lookup: HashMap<String, Vec<ServiceMethodRef>>,
                #[serde(default)]
                waiter_lookup: HashMap<String, Vec<ServiceMethodRef>>,
            }

            // ── load descriptor ───────────────────────────────────────────────

            let test_name = descriptor_file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let descriptor_dir = descriptor_file
                .parent()
                .expect("descriptor file must have a parent directory");

            let descriptor_json = fs::read_to_string(&descriptor_file).unwrap_or_else(|e| {
                panic!("[{test_name}] Failed to read descriptor {descriptor_file:?}: {e}")
            });

            let descriptor: Descriptor =
                serde_json::from_str(&descriptor_json).unwrap_or_else(|e| {
                    panic!("[{test_name}] Failed to parse descriptor {descriptor_file:?}: {e}")
                });

            // ── load service index ────────────────────────────────────────────
            // ServiceIndexFile is resolved relative to the crate root so that
            // multiple test directories can share a single service-index file.

            let index_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(&descriptor.service_index_file);
            let index_json = fs::read_to_string(&index_path).unwrap_or_else(|e| {
                panic!("[{test_name}] Failed to read service index {index_path:?}: {e}")
            });

            let index_data: ServiceIndexJson =
                serde_json::from_str(&index_json).unwrap_or_else(|e| {
                    panic!("[{test_name}] Failed to parse service index {index_path:?}: {e}")
                });

            let service_index = ServiceModelIndex {
                services: index_data.services,
                method_lookup: index_data.method_lookup,
                waiter_lookup: index_data.waiter_lookup,
            };

            // ── extract all source files ──────────────────────────────────────

            let extractor_set = JavaLanguageExtractorSet::default_aws_v2();
            let mut merged = ExtractionResult::default();

            for source_file_name in &descriptor.source_files {
                let java_path = descriptor_dir.join(source_file_name);
                let source_code = fs::read_to_string(&java_path).unwrap_or_else(|e| {
                    panic!("[{test_name}] Failed to read Java file {java_path:?}: {e}")
                });

                let source = SourceFile::with_language(
                    java_path
                        .file_name()
                        .expect("java file must have a name")
                        .into(),
                    source_code,
                    Language::Java,
                );

                let partial = extractor_set
                    .extract_from_file(&source)
                    .unwrap_or_else(|e| {
                        panic!("[{test_name}] Extraction failed for {java_path:?}: {e}")
                    });

                merged.extend(partial);
            }

            // ── match ─────────────────────────────────────────────────────────

            let matcher = JavaMatcher::new(
                &service_index,
                &$crate::extraction::java::extractor::JAVA_UTILITIES_MODEL,
            );
            let actual_calls = matcher.match_calls(&merged);

            // ── compare (full SdkMethodCall including Metadata) ───────────────

            assert_eq!(
                actual_calls, descriptor.expected_sdk_calls,
                "[{test_name}] SdkMethodCall mismatch",
            );
        }
    };
}
