//! [`JavaImportExtractor`] — extracts `import` declarations from Java source files.
//!
//! Also populates [`ExtractionResult::utility_imports`] for non-`services.*` SDK packages
//! (e.g. S3TransferManager, S3Presigner, DynamoDbEnhancedClient) by delegating to
//! [`classify_utility_import`].

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utility_import_extractor::classify_utility_import;
use crate::extraction::java::types::{ExtractionResult, Import, UtilityImport};
use crate::service_configuration::load_service_configuration;
use crate::Location;
use crate::SourceFile;

/// Lookup map from **dash-free Java import segment** to **Botocore service name**.
///
/// The Java SDK derives its package segment from the Smithy service name by removing all
/// dashes (e.g. Smithy `"cloudwatch-logs"` → Java import segment `"cloudwatchlogs"`).
/// This static is built once from [`SmithyBotocoreServiceNameMapping`] via
/// [`ServiceConfiguration::build_java_import_service_map`] and used by
/// [`extract_service_from_import`] to normalise extracted service names.
///
/// # Panics
///
/// Panics on first access if the embedded service-configuration JSON is missing or
/// malformed — both conditions indicate a corrupt binary and are unrecoverable.
///
/// [`SmithyBotocoreServiceNameMapping`]: crate::service_configuration::ServiceConfiguration
static JAVA_IMPORT_SERVICE_MAP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    load_service_configuration()
        .expect("service-configuration.json must be present in embedded data")
        .build_java_import_service_map()
});

/// Extracts all `import` declarations from a Java source file.
///
/// For each import, it attempts to extract:
/// - The AWS service name from the `services.<name>` segment of the import path
/// - The simple class name (last segment of the dotted path)
/// - Whether the import is `import static`
///
/// # Rule body
///
/// ```yaml
/// kind: import_declaration
/// has:
///   pattern: $IMPORT_MARKER
/// ```
///
/// The label `$IMPORT_MARKER` is the discriminator: it captures the first child of the
/// `import_declaration` node (the dotted path or `static` keyword), uniquely identifying
/// this extractor's matches in the combined rule.
pub(crate) struct JavaImportExtractor;

impl SdkExtractor for JavaImportExtractor {
    fn rule_yaml(&self) -> &'static str {
        "kind: import_declaration\nhas:\n  pattern: $IMPORT_MARKER"
    }

    fn discriminator_label(&self) -> &'static str {
        "IMPORT_MARKER"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let node = node_match.get_node();
        let full_text = node.text().to_string();

        // Strip "import" keyword, then optionally "static", then trailing ";"
        let after_import = full_text
            .strip_prefix("import")
            .unwrap_or(&full_text)
            .trim();

        let is_static = after_import.starts_with("static");

        let path_part = after_import
            .strip_prefix("static")
            .unwrap_or(after_import)
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();

        let location = Location::from_node(source_file.path.clone(), node);

        // Try to classify as a utility import first (non-services.* SDK packages)
        if let Some((utility_name, class_name)) = classify_utility_import(&path_part) {
            result.utility_imports.push(UtilityImport {
                expr: path_part,
                utility_name,
                class_name,
                location,
            });
            return;
        }

        let Some(service) = extract_service_from_import(&path_part) else {
            return; // discard non-AWS imports
        };

        result.imports.push(Import {
            expr: path_part,
            service,
            location,
            is_static,
        });
    }
}

/// Extract the AWS service name from an import path, normalised to the Botocore name.
///
/// Looks for the `services.<name>` segment in the dotted path, then applies the
/// [`JAVA_IMPORT_SERVICE_MAP`] to translate the Java SDK package segment (which has all
/// dashes removed from the Smithy name) to the canonical Botocore service name used by
/// the rest of the pipeline.
///
/// # Examples
/// - `"software.amazon.awssdk.services.s3.S3Client"` → `Some("s3")`
/// - `"software.amazon.awssdk.services.dynamodb.model.GetItemRequest"` → `Some("dynamodb")`
/// - `"software.amazon.awssdk.services.cloudwatchlogs.CloudWatchLogsClient"` → `Some("logs")`
/// - `"java.util.List"` → `None`
fn extract_service_from_import(import_path: &str) -> Option<String> {
    let segment = import_path
        .strip_prefix("software.amazon.awssdk.services.")?
        .split('.')
        .next()?
        .to_string();

    // Translate the Java SDK package segment to the Botocore service name when a mapping
    // exists; otherwise use the segment as-is (it already matches the Botocore name).
    let botocore_name = JAVA_IMPORT_SERVICE_MAP
        .get(&segment)
        .cloned()
        .unwrap_or(segment);

    Some(botocore_name)
}

#[cfg(test)]
mod tests {
    use super::extract_service_from_import;
    use rstest::rstest;

    // ── extract_service_from_import unit tests (parameterized) ───────────────

    /// Parameterized test: `(import_path, expected_service)`
    #[rstest]
    // Services whose Java package segment already matches the Botocore name
    #[case("software.amazon.awssdk.services.s3.S3Client", Some("s3"))]
    #[case(
        "software.amazon.awssdk.services.dynamodb.model.GetItemRequest",
        Some("dynamodb")
    )]
    #[case("software.amazon.awssdk.services.sts.StsClient", Some("sts"))]
    #[case(
        "software.amazon.awssdk.services.s3control.S3ControlClient",
        Some("s3control")
    )]
    // Services whose Java package segment is the Smithy name with dashes removed;
    // the mapping must translate them to the Botocore name.
    #[case(
        "software.amazon.awssdk.services.cloudwatchlogs.CloudWatchLogsClient",
        Some("logs")
    )]
    #[case(
        "software.amazon.awssdk.services.elasticloadbalancing.ElasticLoadBalancingClient",
        Some("elb")
    )]
    #[case(
        "software.amazon.awssdk.services.cognitoidentityprovider.CognitoIdentityProviderClient",
        Some("cognito-idp")
    )]
    // Services whose botocore name contains dashes but have no Smithy mapping entry;
    // the auto-generated self-mapping must restore the dashes.
    #[case(
        "software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeClient",
        Some("bedrock-runtime")
    )]
    #[case(
        "software.amazon.awssdk.services.bedrockruntime.model.InvokeModelRequest",
        Some("bedrock-runtime")
    )]
    // Non-AWS imports must be discarded
    #[case("java.util.List", None)]
    #[case("com.example.MyClass", None)]
    #[case("com.example.services.foo.Bar", None)]
    fn test_extract_service_from_import(#[case] import_path: &str, #[case] expected: Option<&str>) {
        assert_eq!(
            extract_service_from_import(import_path).as_deref(),
            expected,
            "unexpected service for import path '{import_path}'"
        );
    }

    // ── file-driven import extraction tests ───────────────────────────────────

    use crate::java_extractor_test;
    java_extractor_test!(
        "tests/java/extractors/imports/*.java",
        crate::extraction::java::types::Import,
        imports
    );
}
