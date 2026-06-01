//! Cross-cutting test: verifies every `record_result_*` key in production code
//! is documented in the Result Data table of TELEMETRY.md.
//!
//! Uses `syn` to parse each `.rs` file into an AST and skip `#[cfg(test)]` items,
//! giving 100% accurate production-vs-test separation without fragile string heuristics.

use std::collections::HashSet;

use quote::ToTokens;
use syn::{visit::Visit, Attribute, File, Item};

/// AST visitor that collects all token streams from non-test items.
struct ProductionCodeCollector {
    /// Accumulated token strings from production items only
    tokens: String,
}

impl ProductionCodeCollector {
    fn new() -> Self {
        Self {
            tokens: String::new(),
        }
    }
}

/// Check if an attribute list contains `#[cfg(test)]`.
fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        // Parse the content inside cfg(...) to check for `test`
        attr.to_token_stream().to_string().contains("test")
    })
}

impl<'ast> Visit<'ast> for ProductionCodeCollector {
    fn visit_item(&mut self, item: &'ast Item) {
        // Skip items annotated with #[cfg(test)]
        let attrs: &[Attribute] = match item {
            Item::Mod(m) => &m.attrs,
            Item::Fn(f) => &f.attrs,
            Item::Impl(i) => &i.attrs,
            Item::Struct(s) => &s.attrs,
            Item::Enum(e) => &e.attrs,
            Item::Const(c) => &c.attrs,
            Item::Static(s) => &s.attrs,
            Item::Trait(t) => &t.attrs,
            Item::Type(t) => &t.attrs,
            Item::Use(u) => &u.attrs,
            _ => {
                // Unknown item kinds — include them (no attrs to check)
                self.tokens.push_str(&item.to_token_stream().to_string());
                self.tokens.push('\n');
                return;
            }
        };

        if has_cfg_test(attrs) {
            return; // Skip this entire item and its children
        }

        // Collect the token representation of this production item
        self.tokens.push_str(&item.to_token_stream().to_string());
        self.tokens.push('\n');
    }
}

/// Extract production code tokens from a Rust source file using `syn` AST parsing.
/// Returns the stringified tokens of all items NOT annotated with `#[cfg(test)]`.
fn extract_production_tokens(content: &str) -> String {
    let file: File = match syn::parse_file(content) {
        Ok(f) => f,
        Err(_) => {
            // If parsing fails (e.g., macro-heavy files), fall back to full content
            // with truncation at first #[cfg(test)] as a best-effort
            return match content.find("#[cfg(test)]") {
                Some(pos) => content[..pos].to_string(),
                None => content.to_string(),
            };
        }
    };

    let mut collector = ProductionCodeCollector::new();
    collector.visit_file(&file);
    collector.tokens
}

const RESULT_DATA_BEGIN: &str = "<!-- BEGIN RESULT DATA TABLE -->";
const RESULT_DATA_END: &str = "<!-- END RESULT DATA TABLE -->";

#[test]
fn span_result_fields_documented_in_telemetry_md() {
    let telemetry_md =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../TELEMETRY.md"))
            .expect("Failed to read TELEMETRY.md");

    // Extract only the Result Data section between markers
    let result_section = {
        let start = telemetry_md
            .find(RESULT_DATA_BEGIN)
            .expect("TELEMETRY.md missing BEGIN RESULT DATA TABLE marker");
        let end = telemetry_md
            .find(RESULT_DATA_END)
            .expect("TELEMETRY.md missing END RESULT DATA TABLE marker");
        &telemetry_md[start..end + RESULT_DATA_END.len()]
    };

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();

    let mut recorded_keys = HashSet::new();
    // Note: syn's to_token_stream() may insert spaces before `(`, so we use `\s*\(` instead of `\(`.
    // Span-level recording functions + event-level setters with explicit string keys:
    let patterns = [
        regex::Regex::new(r#"record_result_str\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"record_result_number\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"record_result_set\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"set_result_str\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"set_result_number\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"set_result_list\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"with_result_str\s*\(\s*"([^"]+)""#).unwrap(),
        regex::Regex::new(r#"with_result_list\s*\(\s*"([^"]+)""#).unwrap(),
    ];
    // Dedicated methods with hardcoded result key names (no string-key argument)
    let hardcoded_methods: &[(&str, &str)] = &[
        (r"with_result_success\s*\(", "success"),
        (r"set_result_success\s*\(", "success"),
        (r"with_result_num_policies\s*\(", "num_policies_generated"),
        (r"set_result_num_policies\s*\(", "num_policies_generated"),
    ];
    let hardcoded_patterns: Vec<(regex::Regex, &str)> = hardcoded_methods
        .iter()
        .map(|(pat, key)| (regex::Regex::new(pat).unwrap(), *key))
        .collect();

    for entry in walkdir::WalkDir::new(workspace_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.extension().is_some_and(|ext| ext == "rs")
                && !path.components().any(|c| c.as_os_str() == "target")
                && !path.components().any(|c| c.as_os_str() == "tests")
        })
    {
        let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
        let production_code = extract_production_tokens(&content);

        for pattern in &patterns {
            for cap in pattern.captures_iter(&production_code) {
                recorded_keys.insert(cap[1].to_string());
            }
        }
        for (pattern, key) in &hardcoded_patterns {
            if pattern.is_match(&production_code) {
                recorded_keys.insert(key.to_string());
            }
        }
    }

    assert!(
        !recorded_keys.is_empty(),
        "Should find at least one record_result_* call in the workspace"
    );

    // Direction 1 — code → doc: every key used in code is documented
    for key in &recorded_keys {
        assert!(
            result_section.contains(&format!("| `{key}` |")),
            "TELEMETRY.md Result Data section missing documentation for `{key}`. \
             Every record_result_* key must have a row between the \
             BEGIN/END RESULT DATA TABLE markers."
        );
    }

    // Direction 2 — doc → code: every documented field exists in code
    let doc_field_pattern = regex::Regex::new(r"(?m)^\|\s*`([^`]+)`\s*\|").unwrap();
    let documented_fields: HashSet<String> = doc_field_pattern
        .captures_iter(result_section)
        .map(|cap| cap[1].to_string())
        .collect();

    assert!(
        !documented_fields.is_empty(),
        "Should find at least one field in the TELEMETRY.md Result Data table"
    );

    let stale_fields: Vec<&String> = documented_fields.difference(&recorded_keys).collect();

    assert!(
        stale_fields.is_empty(),
        "TELEMETRY.md Result Data table documents fields not found in production code: {stale_fields:?}. \
         Remove stale rows from the Result Data table or add the corresponding \
         record_result_* / set_result_* calls in production code."
    );
}

// =============================================================================
// Unit tests for extract_production_tokens
// =============================================================================

#[test]
fn extract_production_tokens_skips_cfg_test_module() {
    let input = r#"
fn production() { iam_policy_autopilot_common::telemetry::span::record_result_str("real_key", "v"); }

#[cfg(test)]
mod tests {
    fn test_thing() { record_result_str("test_key", "v"); }
}
"#;
    let output = extract_production_tokens(input);
    assert!(output.contains("real_key"), "should keep production code");
    assert!(!output.contains("test_key"), "should skip test code");
}

#[test]
fn extract_production_tokens_handles_nested_code_in_test() {
    let input = r#"
fn prod() { record_result_str("keep", "v"); }

#[cfg(test)]
mod tests {
    fn test() {
        if true {
            record_result_str("skip_nested", "v");
        }
    }
}
"#;
    let output = extract_production_tokens(input);
    assert!(output.contains("keep"));
    assert!(!output.contains("skip_nested"));
}

#[test]
fn extract_production_tokens_preserves_code_after_test_block() {
    let input = r#"
fn before() { record_result_str("before_key", "v"); }

#[cfg(test)]
mod tests {
    fn test() { record_result_str("test_key", "v"); }
}

fn after() { record_result_str("after_key", "v"); }
"#;
    let output = extract_production_tokens(input);
    assert!(output.contains("before_key"));
    assert!(output.contains("after_key"));
    assert!(!output.contains("test_key"));
}

#[test]
fn extract_production_tokens_no_test_block() {
    let input = r#"fn only_prod() { record_result_str("only", "v"); }"#;
    let output = extract_production_tokens(input);
    assert!(output.contains("only"));
}
