//! Parser and resolver for the Terraform AWS provider `names_data.hcl`.
//!
//! This module parses the embedded HCL file into a structured representation
//! that retains the `sdk.arn_namespace` and `resource_prefix` fields (both
//! `actual` regex and `correct` prefix). It pre-computes a reversed mapping
//! from Terraform resource types to `(arn_namespace, resource_suffix)` tuples,
//! using exact HashMap lookups for the vast majority of patterns and falling
//! back to regex only for the handful of patterns that require it.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

/// Embedded `names_data.hcl` from the Terraform AWS provider.
/// Sourced from the `terraform-provider-aws` submodule via git sparse-checkout.
const NAMES_DATA_HCL: &str = include_str!(
    "../../../resources/config/terraform/terraform-provider-aws/names/data/names_data.hcl"
);

// ---------------------------------------------------------------------------
// HCL schema constants for `names_data.hcl` structure
// ---------------------------------------------------------------------------

/// Top-level block identifier: `service "label" { ... }`
const HCL_BLOCK_SERVICE: &str = "service";
/// Inner block: `sdk { arn_namespace = "..." }`
const HCL_BLOCK_SDK: &str = "sdk";
/// Attribute inside `sdk` block: the IAM ARN namespace (e.g., `"s3"`, `"dynamodb"`)
const HCL_ATTR_ARN_NAMESPACE: &str = "arn_namespace";
/// Inner block or top-level attribute: `resource_prefix { correct = "..." actual = "..." }`
const HCL_BLOCK_RESOURCE_PREFIX: &str = "resource_prefix";
/// Attribute inside `resource_prefix`: the canonical prefix (e.g., `"aws_s3_"`)
const HCL_ATTR_CORRECT: &str = "correct";
/// Attribute inside `resource_prefix`: optional regex pattern for actual resource types
const HCL_ATTR_ACTUAL: &str = "actual";

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single service entry parsed from `names_data.hcl`.
#[derive(Debug, Clone)]
struct ServiceEntry {
    /// The service block label (e.g., `"s3"`, `"dynamodb"`).
    service_key: String,
    /// ARN namespace from `sdk { arn_namespace = "..." }`.
    arn_namespace: String,
    /// The `resource_prefix` block data.
    resource_prefix: ResourcePrefix,
}

/// The `resource_prefix` block inside a service entry.
///
/// Some services only have `correct`; others also have `actual` which is a
/// regex pattern matching the *real* Terraform resource type prefixes that
/// map to this service.
///
/// # Examples
///
/// **Simple service (SQS)** — only `correct` is set:
/// ```text
/// correct = "aws_sqs_"    // matches aws_sqs_queue, aws_sqs_queue_policy, …
/// actual  = None
/// ```
///
/// **Aliased service (RDS)** — `actual` captures multiple legacy prefixes:
/// ```text
/// correct = "aws_rds_"
/// actual  = Some("aws_(db_|rds_)")   // matches aws_db_instance, aws_rds_cluster, …
/// ```
///
/// **Aliased service (AMP / Prometheus)** — `actual` is a plain literal prefix:
/// ```text
/// correct = "aws_amp_"
/// actual  = Some("aws_prometheus_")  // matches aws_prometheus_workspace, …
/// ```
#[derive(Debug, Clone)]
struct ResourcePrefix {
    /// The canonical prefix (e.g., `"aws_s3_"`). Always present.
    correct: String,
    /// Optional regex pattern for the *actual* Terraform resource types.
    /// When present, this takes precedence over `correct` for matching.
    actual: Option<String>,
}

// ---------------------------------------------------------------------------
// TypeResolver
// ---------------------------------------------------------------------------

/// Pre-computed lookup structure for resolving Terraform resource types
/// to `(arn_namespace, resource_suffix)` tuples.
///
/// # Examples
///
/// ```text
/// resolver.resolve("aws_s3_bucket")           → Some(("s3",       "bucket"))
/// resolver.resolve("aws_db_instance")         → Some(("rds",      "instance"))
/// resolver.resolve("aws_lambda_function")     → Some(("lambda",   "function"))
/// resolver.resolve("aws_canonical_user_id")   → Some(("s3",       "canonical_user_id"))
/// ```
///
/// # Resolution strategy (in order)
///
/// 1. **Exact full-type lookup** in `exact_map` — for expanded `actual` entries
///    that represent complete resource type names (e.g., `"aws_canonical_user_id"` → `"s3"`).
/// 2. **Regex fallback** for the small number of `actual` patterns that use
///    advanced regex features (negative lookahead, `\b`, `$`, etc.) —
///    evaluated before prefix matching so overrides take precedence.
/// 3. **Longest-prefix match** in `prefix_map` — for `correct` prefixes and expanded
///    `actual` entries that are prefixes (ending with `_`).
pub(super) struct TerraformServiceAndResourceResolver {
    /// Maps exact Terraform resource type → ARN namespace.
    exact_map: HashMap<String, String>,
    /// Maps Terraform resource type prefix → ARN namespace.
    prefix_map: HashMap<String, String>,
    regex_fallbacks: Vec<(Regex, String, String)>,
}

/// Single cache for the pre-computed resolver.
static TYPE_RESOLVER: OnceLock<TerraformServiceAndResourceResolver> = OnceLock::new();

impl TerraformServiceAndResourceResolver {
    /// Build a resolver from parsed service entries.
    ///
    /// For each entry:
    /// - The `correct` prefix is always inserted into the prefix map.
    /// - If `actual` is present, we try to expand it into literal strings
    ///   and insert those into the appropriate map. If expansion fails
    ///   (complex regex), we compile it as a regex fallback.
    fn new(entries: &[ServiceEntry]) -> Self {
        let mut exact_map: HashMap<String, String> = HashMap::new();
        let mut prefix_map: HashMap<String, String> = HashMap::new();
        let mut regex_fallbacks = Vec::new();

        for entry in entries {
            let arn_ns = &entry.arn_namespace;

            // Always add the `correct` prefix to the prefix map
            prefix_map.insert(entry.resource_prefix.correct.clone(), arn_ns.clone());

            // Handle `actual` pattern
            if let Some(actual_pattern) = &entry.resource_prefix.actual {
                if let Some(expanded) = try_expand_pattern(actual_pattern) {
                    for candidate in expanded {
                        if candidate.ends_with('_') {
                            prefix_map.insert(candidate, arn_ns.clone());
                        } else if candidate.starts_with(&entry.resource_prefix.correct) {
                            // Already covered by the `correct` prefix — skip
                        } else {
                            exact_map.insert(candidate, arn_ns.clone());
                        }
                    }
                } else {
                    let anchored = format!("^(?:{actual_pattern})");
                    match Regex::new(&anchored) {
                        Ok(re) => {
                            regex_fallbacks.push((
                                re,
                                entry.arn_namespace.clone(),
                                entry.resource_prefix.correct.clone(),
                            ));
                        }
                        Err(e) => {
                            log::warn!(
                                "Invalid regex in names_data.hcl for service '{}': {e}",
                                entry.service_key
                            );
                        }
                    }
                }
            }
        }

        log::debug!(
            "Built resolver: {} exact entries, {} prefix entries, {} regex fallbacks",
            exact_map.len(),
            prefix_map.len(),
            regex_fallbacks.len()
        );

        Self {
            exact_map,
            prefix_map,
            regex_fallbacks,
        }
    }

    /// Return the lazily-initialized global resolver backed by the embedded
    /// `names_data.hcl`.
    pub(super) fn global() -> &'static Self {
        TYPE_RESOLVER.get_or_init(|| {
            let entries = parse_names_data(NAMES_DATA_HCL);
            Self::new(&entries)
        })
    }

    /// Resolve a Terraform resource type like `"aws_s3_bucket"` into an
    /// `(arn_namespace, resource_suffix)` tuple (e.g., `("s3", "bucket")`).
    ///
    /// The tuple can be used to obtain necessary information from service_reference
    /// for proper resource arn generation.
    ///
    /// Resolution order:
    /// 1. Exact full-type lookup.
    /// 2. Regex fallback for complex `actual` patterns — evaluated before
    ///    prefix matching so overrides take precedence.
    /// 3. Longest-prefix match.
    ///
    /// Returns `None` if the resource type doesn't match any known service.
    pub(super) fn resolve(&self, terraform_type: &str) -> Option<(String, String)> {
        // 1. Exact full-type lookup (e.g., "aws_canonical_user_id" → s3)
        if let Some(arn_ns) = self.exact_map.get(terraform_type) {
            let suffix = terraform_type
                .strip_prefix(crate::extraction::terraform::AWS_RESOURCE_PREFIX)
                .unwrap_or(terraform_type);
            return Some((arn_ns.clone(), suffix.to_string()));
        }

        // 2. Regex fallback — before prefix matching so overrides win
        for (regex, arn_ns, correct_prefix) in &self.regex_fallbacks {
            if regex.is_match(terraform_type) {
                let suffix = if terraform_type.starts_with(correct_prefix.as_str()) {
                    &terraform_type[correct_prefix.len()..]
                } else {
                    terraform_type
                        .strip_prefix(crate::extraction::terraform::AWS_RESOURCE_PREFIX)
                        .unwrap_or(terraform_type)
                };
                return Some((arn_ns.clone(), suffix.to_string()));
            }
        }

        // 3. Prefix lookup — find the longest matching prefix
        if let Some((prefix, arn_ns)) = self
            .prefix_map
            .iter()
            .filter(|(prefix, _)| terraform_type.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
        {
            let suffix = &terraform_type[prefix.len()..];
            return Some((arn_ns.clone(), suffix.to_string()));
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse `names_data.hcl` into a list of [`ServiceEntry`] values.
///
/// # Panics
///
/// Panics if the HCL content cannot be parsed. Since `names_data.hcl` is
/// embedded at compile time and validated by unit tests, a parse failure
/// indicates a corrupted build and should never reach production.
fn parse_names_data(content: &str) -> Vec<ServiceEntry> {
    let body: hcl::Body =
        hcl::from_str(content).unwrap_or_else(|e| panic!("Failed to parse names_data.hcl: {e}"));

    let mut entries = Vec::new();

    for structure in body {
        let hcl::Structure::Block(block) = structure else {
            continue;
        };
        if block.identifier.as_str() != HCL_BLOCK_SERVICE {
            continue;
        }

        let service_key = block
            .labels
            .first()
            .map(|l| l.as_str().to_string())
            .unwrap_or_default();

        let mut arn_namespace = String::new();
        let mut correct = String::new();
        let mut actual: Option<String> = None;

        for inner in &block.body {
            match inner {
                hcl::Structure::Block(inner_block) => match inner_block.identifier.as_str() {
                    HCL_BLOCK_SDK => {
                        for attr in &inner_block.body {
                            if let hcl::Structure::Attribute(a) = attr {
                                if a.key.as_str() == HCL_ATTR_ARN_NAMESPACE {
                                    if let hcl::Expression::String(s) = &a.expr {
                                        arn_namespace.clone_from(s);
                                    }
                                }
                            }
                        }
                    }
                    HCL_BLOCK_RESOURCE_PREFIX => {
                        for attr in &inner_block.body {
                            if let hcl::Structure::Attribute(a) = attr {
                                match a.key.as_str() {
                                    HCL_ATTR_CORRECT => {
                                        if let hcl::Expression::String(s) = &a.expr {
                                            correct.clone_from(s);
                                        }
                                    }
                                    HCL_ATTR_ACTUAL => {
                                        if let hcl::Expression::String(s) = &a.expr {
                                            actual = Some(s.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ => {}
                },
                hcl::Structure::Attribute(attr) => {
                    if attr.key.as_str() == HCL_BLOCK_RESOURCE_PREFIX {
                        if let hcl::Expression::String(s) = &attr.expr {
                            correct.clone_from(s);
                        }
                    }
                }
            }
        }

        if arn_namespace.is_empty() || correct.is_empty() {
            continue;
        }

        entries.push(ServiceEntry {
            service_key,
            arn_namespace,
            resource_prefix: ResourcePrefix { correct, actual },
        });
    }

    log::debug!(
        "Parsed {} service entries from names_data.hcl",
        entries.len()
    );
    entries
}

// ---------------------------------------------------------------------------
// Pattern expansion helpers
// ---------------------------------------------------------------------------

// The two constants below define the "reject gates" for [`try_expand_pattern`].
// Together they ensure that only plain literals and simple `(a|b|c)`
// alternations are expanded, while anything more complex falls back to
// runtime regex compilation.
//
// Cross-referencing the 51 `actual` patterns in `names_data.hcl`:
//
// • 34 are plain literals (no special chars, no parens) — expanded directly.
//     e.g., `"aws_prometheus_"`, `"aws_api_gateway_"`, `"aws_cloudtrail"`
//
// • 10 are simple alternations — decomposed by `try_expand_pattern`.
//     e.g., `"aws_(db_|rds_)"`, `"aws_(s3_account_|s3control_|s3_access_)"`
//
// • 3 are rejected by REGEX_SPECIAL_SEQUENCES (all use `(?!`):
//     `"aws_cloudwatch_(?!(event_|log_|query_))"`
//     `"aws_cognito_identity_(?!provider)"`
//     `"aws_route53_(?!resolver_)"`
//
// • 3 are rejected by REGEX_SPECIAL_CHARS:
//     `"aws_a?lb(\\b|...)"` — `?` and `\\`
//     `"aws_(...|regions?|...)$"` — `?` and `$`
//     `"aws_((...)?...|route\\b)"` — `?` and `\\`
//
// • 1 is rejected by the nested-parenthesis check inside try_expand_pattern:
//     `"aws_(ami|...|ec2_(...)|...)"` — inner `(` `)` inside outer group

/// Single regex metacharacters that prevent a string fragment from being
/// treated as a literal.
///
/// Quantifiers (`?`, `+`, `*`), anchors (`^`, `$`), character class opener
/// (`[`), wildcard (`.`), and escape (`\\`).
///
/// **Not included:** `(`, `)`, and `|` — these are the characters that
/// [`try_expand_pattern`] explicitly decomposes when handling simple
/// `(a|b|c)` alternations.
///
/// Real examples from `names_data.hcl` caught by this list:
/// - `"aws_a?lb(\\b|...)"` — `?` triggers rejection (ELBv2 service)
/// - `"aws_(...|regions?|...)$"` — `?` and `$` trigger rejection (meta service)
/// - `"aws_((...)?...|route\\b)"` — `?` and `\\` trigger rejection (VPC service)
const REGEX_SPECIAL_CHARS: &[char] = &['?', '+', '*', '.', '^', '$', '[', '\\'];

/// Multi-character sequences that introduce advanced regex group types
/// (negative lookahead `(?!`, non-capturing group `(?:`, positive lookahead
/// `(?=`, lookbehind / named capture `(?<`).
///
/// These are checked *before* any parenthesis parsing in [`try_expand_pattern`]
/// so that patterns like `"aws_cloudwatch_(?!(event_|log_))"` are immediately
/// rejected without attempting expansion.
///
/// Real examples from `names_data.hcl` caught by this list (all use `(?!`):
/// - `"aws_cloudwatch_(?!(event_|log_|query_))"` — CloudWatch service
/// - `"aws_cognito_identity_(?!provider)"` — Cognito Identity service
/// - `"aws_route53_(?!resolver_)"` — Route 53 service
const REGEX_SPECIAL_SEQUENCES: &[&str] = &["(?!", "(?:", "(?=", "(?<"];

/// Returns `true` if the string contains any character from [`REGEX_SPECIAL_CHARS`].
fn has_regex_chars(s: &str) -> bool {
    s.contains(REGEX_SPECIAL_CHARS)
}

/// Try to expand an `actual` pattern into a list of literal prefix strings.
///
/// Handles two forms:
/// - Plain literal prefix: `"aws_prometheus_"` → `["aws_prometheus_"]`
/// - Simple alternation: `"aws_(db_|rds_)"` → `["aws_db_", "aws_rds_"]`
///
/// Returns `None` if the pattern uses advanced regex features (detected via
/// [`REGEX_SPECIAL_SEQUENCES`] and [`REGEX_SPECIAL_CHARS`]), or nested
/// parentheses, in which case the caller compiles the pattern as a real
/// [`Regex`] for runtime matching.
fn try_expand_pattern(pattern: &str) -> Option<Vec<String>> {
    for seq in REGEX_SPECIAL_SEQUENCES {
        if pattern.contains(seq) {
            return None;
        }
    }

    let Some(open) = pattern.find('(') else {
        if has_regex_chars(pattern) {
            return None;
        }
        return Some(vec![pattern.to_string()]);
    };

    let close = pattern.rfind(')')?;

    let inner = &pattern[open + 1..close];
    if inner.contains('(') || inner.contains(')') {
        return None;
    }

    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    if has_regex_chars(prefix) || has_regex_chars(suffix) {
        return None;
    }

    let alternatives: Vec<&str> = inner.split('|').collect();
    for alt in &alternatives {
        if has_regex_chars(alt) {
            return None;
        }
    }

    let expanded = alternatives
        .into_iter()
        .map(|alt| format!("{prefix}{alt}{suffix}"))
        .collect();

    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // --- parse_names_data tests ---

    #[test]
    fn test_parse_names_data_returns_entries() {
        let entries = parse_names_data(NAMES_DATA_HCL);
        assert!(
            entries.len() > 50,
            "Expected >50 entries, got {}",
            entries.len()
        );
    }

    #[test]
    #[should_panic(expected = "Failed to parse names_data.hcl")]
    fn test_parse_names_data_panics_on_invalid_hcl() {
        parse_names_data("this is {{ not valid HCL");
    }

    #[rstest]
    #[case("s3", "s3", "aws_s3_", true)]
    #[case("sqs", "sqs", "aws_sqs_", false)]
    #[case("amp", "aps", "aws_amp_", true)]
    fn test_parse_names_data_service_entry(
        #[case] service_key: &str,
        #[case] expected_arn_ns: &str,
        #[case] expected_resource_prefix: &str,
        #[case] has_actual: bool,
    ) {
        let entries = parse_names_data(NAMES_DATA_HCL);
        let entry = entries
            .iter()
            .find(|e| e.service_key == service_key)
            .unwrap_or_else(|| panic!("service_key '{service_key}' not found in names_data"));
        assert_eq!(
            entry.arn_namespace, expected_arn_ns,
            "arn_namespace mismatch for {service_key}"
        );
        assert_eq!(
            entry.resource_prefix.correct, expected_resource_prefix,
            "correct prefix mismatch for {service_key}"
        );
        assert_eq!(
            entry.resource_prefix.actual.is_some(),
            has_actual,
            "actual pattern presence mismatch for {service_key}"
        );
    }

    // --- try_expand_pattern tests ---

    #[rstest]
    #[case("aws_prometheus_",                                                    Some(vec!["aws_prometheus_"]))]
    #[case("aws_(db_|rds_)",                                                     Some(vec!["aws_db_", "aws_rds_"]))]
    #[case("aws_(canonical_user_id|s3_bucket|s3_object|s3_directory_bucket)",     Some(vec!["aws_canonical_user_id", "aws_s3_bucket", "aws_s3_object", "aws_s3_directory_bucket"]))]
    #[case("aws_cloudwatch_(?!(event_|log_|query_))", None)]
    #[case("aws_a?lb(\\b|_listener|_target_group)", None)]
    #[case("aws_(arn|billing_service_account|regions?)$", None)]
    fn test_try_expand_pattern(#[case] pattern: &str, #[case] expected: Option<Vec<&str>>) {
        let result = try_expand_pattern(pattern);
        let expected_owned: Option<Vec<String>> =
            expected.map(|v| v.into_iter().map(String::from).collect());
        assert_eq!(
            result, expected_owned,
            "try_expand_pattern mismatch for pattern: {pattern}"
        );
    }

    // --- TypeResolver::resolve tests (parameterized) ---

    /// Parameterized test for successful Terraform type → (service, suffix) resolution.
    ///
    /// Each case represents a different resolution strategy:
    /// - Prefix match via `correct` prefix (e.g., aws_sqs_queue)
    /// - Exact match via expanded `actual` pattern (e.g., aws_canonical_user_id)
    /// - Prefix match via expanded `actual` alternation (e.g., aws_db_instance → rds)
    /// - Regex fallback for complex patterns (e.g., aws_cloudwatch_metric_alarm)
    #[rstest]
    // Standard prefix matches
    #[case("aws_s3_bucket", "s3", "bucket")]
    #[case("aws_dynamodb_table", "dynamodb", "table")]
    #[case("aws_lambda_function", "lambda", "function")]
    #[case("aws_sqs_queue", "sqs", "queue")]
    #[case("aws_s3control_bucket_policy", "s3", "bucket_policy")]
    // Exact match via expanded `actual` alternation
    #[case("aws_canonical_user_id", "s3", "canonical_user_id")]
    #[case("aws_s3_directory_bucket", "s3", "directory_bucket")]
    // Prefix match via expanded `actual` alternation
    #[case("aws_db_instance", "rds", "instance")]
    #[case("aws_rds_cluster", "rds", "cluster")]
    #[case("aws_prometheus_workspace", "aps", "workspace")]
    #[case("aws_api_gateway_rest_api", "apigateway", "rest_api")]
    #[case("aws_dx_connection", "directconnect", "connection")]
    #[case("aws_cloudwatch_event_rule", "events", "rule")]
    #[case("aws_elb", "elb", "elb")]
    // Regex fallback matches
    #[case("aws_cloudwatch_metric_alarm", "cloudwatch", "metric_alarm")]
    #[case("aws_cloudwatch_log_group", "logs", "group")]
    fn test_resolve_terraform_type(
        #[case] terraform_type: &str,
        #[case] expected_service: &str,
        #[case] expected_resource_type: &str,
    ) {
        let resolver = TerraformServiceAndResourceResolver::global();
        let (service, resource_type) = resolver
            .resolve(terraform_type)
            .unwrap_or_else(|| panic!("Failed to resolve '{terraform_type}'"));
        assert_eq!(
            service, expected_service,
            "service mismatch for '{terraform_type}'"
        );
        assert_eq!(
            resource_type, expected_resource_type,
            "suffix mismatch for '{terraform_type}'"
        );
    }

    /// Parameterized test for types that should NOT resolve.
    #[rstest]
    #[case("google_storage_bucket")]
    #[case("not_a_resource")]
    #[case("azurerm_storage_account")]
    fn test_resolve_unknown_returns_none(#[case] terraform_type: &str) {
        let resolver = TerraformServiceAndResourceResolver::global();
        assert!(
            resolver.resolve(terraform_type).is_none(),
            "Expected None for '{terraform_type}', got {:?}",
            resolver.resolve(terraform_type)
        );
    }

    // --- TypeResolver internals ---

    #[test]
    fn test_resolver_has_expanded_entries() {
        let resolver = TerraformServiceAndResourceResolver::global();
        assert!(resolver.prefix_map.contains_key("aws_db_"));
        assert!(resolver.prefix_map.contains_key("aws_rds_"));
        assert!(resolver.exact_map.contains_key("aws_canonical_user_id"));
    }

    #[test]
    fn test_resolver_has_few_regex_fallbacks() {
        let resolver = TerraformServiceAndResourceResolver::global();
        assert!(
            resolver.regex_fallbacks.len() < 20,
            "Expected <20 regex fallbacks, got {}",
            resolver.regex_fallbacks.len()
        );
    }

    #[test]
    fn test_resolver_pointer_identity() {
        let a = TerraformServiceAndResourceResolver::global();
        let b = TerraformServiceAndResourceResolver::global();
        assert!(std::ptr::eq(a, b), "Should return the same cached instance");
    }
}
