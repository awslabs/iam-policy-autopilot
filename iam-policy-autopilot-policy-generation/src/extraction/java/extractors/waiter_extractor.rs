//! [`JavaWaiterCallExtractor`] — extracts `waitUntil*` calls from Java source files.
//!
//! The extractor captures each `waitUntil*` invocation as a [`Waiter`]. When the receiver
//! is a plain identifier (Tier 1), it performs a scope walk to find the receiver variable's
//! declaration (local var declaration or formal parameter) and stores it in
//! [`Waiter::receiver_declaration`].
//!
//! The scope-walk logic is shared with the paginator and method-call extractors via
//! [`utils::find_receiver_declaration`].

use convert_case::{Case, Casing};

use crate::extraction::java::extractor::{JavaNodeMatch, SdkExtractor};
use crate::extraction::java::extractors::utils;
use crate::extraction::java::types::{ExtractionResult, Waiter};
use crate::Location;
use crate::SourceFile;

// ================================================================================================
// JavaWaiterCallExtractor
// ================================================================================================

/// Extracts `waitUntil*` calls (e.g. `waiter.waitUntilBucketExists(request)`).
///
/// When the receiver is a plain identifier, performs a scope walk to find the receiver
/// variable's declaration and stores it in [`Waiter::receiver_declaration`].
///
/// # Rule body
///
/// ```yaml
/// kind: method_invocation
/// all:
///   - has:
///       field: object
///       pattern: $WAITER_OBJ
///   - has:
///       field: name
///       regex: '^waitUntil'
///       pattern: $WAIT_METHOD
/// ```
///
/// The label `$WAIT_METHOD` is the discriminator (captures the `waitUntil*` method name).
pub(crate) struct JavaWaiterCallExtractor;

impl SdkExtractor for JavaWaiterCallExtractor {
    fn rule_yaml(&self) -> &'static str {
        r"kind: method_invocation
all:
  - has:
      field: object
      pattern: $WAITER_OBJ
  - has:
      field: name
      regex: '^waitUntil'
      pattern: $WAIT_METHOD"
    }

    fn discriminator_label(&self) -> &'static str {
        "WAIT_METHOD"
    }

    fn process(
        &self,
        node_match: &JavaNodeMatch<'_>,
        source_file: &SourceFile,
        result: &mut ExtractionResult,
    ) {
        let full_method = match node_match.get_env().get_match("WAIT_METHOD") {
            Some(n) => n.text().to_string(),
            None => return,
        };

        // Strip the "waitUntil" prefix and convert to camelCase so that waiter_type
        // matches the waiter_lookup index keys produced by ServiceDiscovery
        // (e.g. "waitUntilBucketExists" → strip → "BucketExists" → camel → "bucketExists").
        let waiter_type = full_method
            .strip_prefix("waitUntil")
            .unwrap_or(&full_method)
            .to_case(Case::Camel);

        let node = node_match.get_node();
        let location = Location::from_node(source_file.path.clone(), node);
        let expr = node.text().to_string();

        let parameters = utils::extract_arguments_from_node(node);

        let receiver_declaration =
            utils::find_receiver_declaration_from_env(node_match, "WAITER_OBJ", source_file);

        result.waiters.push(Waiter {
            expr,
            name: waiter_type,
            parameters,
            location,
            receiver_declaration,
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::java_extractor_test;

    java_extractor_test!(
        "tests/java/extractors/waiters/*.java",
        crate::extraction::java::types::Waiter,
        waiters
    );
}
