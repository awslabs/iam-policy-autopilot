//! [`JavaMethodCallExtractor`] — extracts `receiver.method(args)` calls from Java source files.
//!
//! Uses labeled captures so the process stage can reliably retrieve each component:
//! - `$MC_OBJ` — the receiver expression (e.g. `"s3Client"`)  [discriminator]
//! - `$MC_METHOD` — the method name identifier (e.g. `"putObject"`)
//!
//! Plain function calls without a receiver are excluded by requiring the `object` field.

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{Call, ExtractionResult};
use crate::Location;
use crate::SourceFile;

/// Extracts all `receiver.method(args)` method invocations from a Java source file.
///
/// Uses labeled captures so the process stage can reliably retrieve each component:
/// - `$MC_OBJ` — the receiver expression (e.g. `"s3Client"`)  [discriminator]
/// - `$MC_METHOD` — the method name identifier (e.g. `"putObject"`)
///
/// Plain function calls without a receiver are excluded by requiring the `object` field.
///
/// # Rule body
///
/// ```yaml
/// kind: method_invocation
/// all:
///   - has:
///       field: object
///       pattern: $MC_OBJ
///   - has:
///       field: name
///       pattern: $MC_METHOD
/// ```
pub(crate) struct JavaMethodCallExtractor;

impl SdkExtractor for JavaMethodCallExtractor {
    fn rule_yaml(&self) -> &'static str {
        r"kind: method_invocation
all:
  - has:
      field: object
      pattern: $MC_OBJ
  - has:
      field: name
      pattern: $MC_METHOD"
    }

    fn discriminator_label(&self) -> &'static str {
        "MC_OBJ"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let method = match node_match.get_env().get_match("MC_METHOD") {
            Some(n) => n.text().to_string(),
            None => return,
        };

        let node = node_match.get_node();
        let parameters = utils::extract_arguments_from_node(node);
        let location = Location::from_node(source_file.path.clone(), node);
        let expr = node.text().to_string();

        let receiver_declaration =
            utils::find_receiver_declaration_from_env(node_match, "MC_OBJ", source_file);

        result.calls.push(Call {
            expr,
            method,
            parameters,
            location,
            receiver_declaration,
        });
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::java_extractor_test;

    java_extractor_test!(
        "tests/java/extractors/methods/*.java",
        crate::extraction::java::types::Call,
        calls
    );
}
