//! Argument extraction utilities for JavaScript/TypeScript AST processing
//!
//! This module provides standardized ways to extract and parse arguments
//! from ast-grep nodes, handling object literals, shorthand properties,
//! spread operators, and proper value resolution (literal vs identifier).

use ast_grep_core::tree_sitter::StrDoc;

use crate::extraction::{Parameter, ParameterValue};

/// Utility for extracting arguments from JavaScript/TypeScript AST nodes
pub(crate) struct ArgumentExtractor;

impl ArgumentExtractor {
    /// Extract parameters from an object literal argument node
    ///
    /// Handles various JavaScript/TypeScript object patterns:
    /// - Regular properties: `{ Bucket: "test", Key: keyName }`
    /// - Shorthand properties: `{ client }` → `{ client: client }`
    /// - Spread elements: `{ ...config, Bucket: "override" }`
    ///
    /// Returns a vector of Parameters with proper Resolved/Unresolved classification
    pub fn extract_object_parameters<T>(
        args_node: Option<&ast_grep_core::Node<StrDoc<T>>>,
    ) -> Vec<Parameter>
    where
        T: ast_grep_language::LanguageExt,
    {
        let Some(node) = args_node else {
            return Vec::new();
        };

        let text = node.text();
        let text_str = text.as_ref();

        // Parse the object literal structure
        Self::parse_object_literal_with_resolution(text_str)
    }

    /// Parse object literal text and create Parameters with proper value resolution
    fn parse_object_literal_with_resolution(obj_text: &str) -> Vec<Parameter> {
        let mut parameters = Vec::new();
        let trimmed = obj_text.trim();

        // Handle empty objects
        if trimmed == "{}" || trimmed.is_empty() {
            return parameters;
        }

        // Remove outer braces if present
        let content = if trimmed.starts_with('{') && trimmed.ends_with('}') {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };

        // Parse key-value pairs with proper resolution
        let pairs = Self::split_object_properties(content);

        for (position, pair) in pairs.iter().enumerate() {
            if let Some(param) = Self::parse_property_to_parameter(pair, position) {
                parameters.push(param);
            }
        }

        parameters
    }

    /// Split object content into property strings, handling nested structures
    fn split_object_properties(content: &str) -> Vec<String> {
        let mut properties = Vec::new();
        let mut current_property = String::new();
        let mut brace_level = 0;
        let mut bracket_level = 0;
        let mut paren_level = 0;
        let mut in_string = false;
        let mut string_char = '\0';

        for ch in content.chars() {
            match ch {
                '"' | '\'' if !in_string => {
                    in_string = true;
                    string_char = ch;
                    current_property.push(ch);
                }
                ch if in_string && ch == string_char => {
                    in_string = false;
                    current_property.push(ch);
                }
                '{' if !in_string => {
                    brace_level += 1;
                    current_property.push(ch);
                }
                '}' if !in_string => {
                    brace_level -= 1;
                    current_property.push(ch);
                }
                '[' if !in_string => {
                    bracket_level += 1;
                    current_property.push(ch);
                }
                ']' if !in_string => {
                    bracket_level -= 1;
                    current_property.push(ch);
                }
                '(' if !in_string => {
                    paren_level += 1;
                    current_property.push(ch);
                }
                ')' if !in_string => {
                    paren_level -= 1;
                    current_property.push(ch);
                }
                ',' if !in_string && brace_level == 0 && bracket_level == 0 && paren_level == 0 => {
                    if !current_property.trim().is_empty() {
                        properties.push(current_property.trim().to_string());
                    }
                    current_property.clear();
                }
                _ => {
                    current_property.push(ch);
                }
            }
        }

        // Don't forget the last property
        if !current_property.trim().is_empty() {
            properties.push(current_property.trim().to_string());
        }

        properties
    }

    /// Parse a single property into a Parameter
    /// Handles: regular (key: value), shorthand (key), spread (...obj)
    fn parse_property_to_parameter(property: &str, position: usize) -> Option<Parameter> {
        let trimmed = property.trim();

        // Handle spread operator (similar to Python's **kwargs)
        if trimmed.starts_with("...") {
            // This is a spread element - we can't resolve what properties it contains
            // Note: JS/TS doesn't have a DictionarySplat equivalent in Parameter enum
            // For now, we'll skip spread properties (could be added later)
            return None;
        }

        // Check for key-value separator
        if let Some(colon_pos) = Self::find_property_separator(trimmed) {
            // Regular property: key: value
            let key = trimmed[..colon_pos].trim();
            let value_text = trimmed[colon_pos + 1..].trim();

            Some(Parameter::Keyword {
                name: Self::normalize_property_key(key),
                value: Self::classify_value(value_text),
                position,
                type_annotation: None,
            })
        } else {
            // Shorthand property: { client } means { client: client }
            let key = Self::normalize_property_key(trimmed);
            Some(Parameter::Keyword {
                name: key.clone(),
                value: ParameterValue::Unresolved(key), // Shorthand is always unresolved
                position,
                type_annotation: None,
            })
        }
    }

    /// Find the colon separator in a property, ignoring colons inside strings/nested objects
    fn find_property_separator(text: &str) -> Option<usize> {
        let mut in_string = false;
        let mut string_char = '\0';
        let mut depth = 0;

        for (i, ch) in text.chars().enumerate() {
            match ch {
                '"' | '\'' if !in_string => {
                    in_string = true;
                    string_char = ch;
                }
                ch if in_string && ch == string_char => {
                    in_string = false;
                }
                '{' | '[' | '(' if !in_string => {
                    depth += 1;
                }
                '}' | ']' | ')' if !in_string => {
                    depth -= 1;
                }
                ':' if !in_string && depth == 0 => {
                    return Some(i);
                }
                _ => {}
            }
        }

        None
    }

    /// Normalize property key (remove quotes if present)
    fn normalize_property_key(key: &str) -> String {
        key.trim().trim_matches('"').trim_matches('\'').to_string()
    }

    /// Classify a value as Resolved (literal) or Unresolved (expression)
    fn classify_value(value_text: &str) -> ParameterValue {
        let trimmed = value_text.trim();

        // String literals (single or double quotes)
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            return ParameterValue::Resolved(trimmed[1..trimmed.len() - 1].to_string());
        }

        // Template literals without interpolation
        if trimmed.starts_with('`') && trimmed.ends_with('`') {
            let content = &trimmed[1..trimmed.len() - 1];
            if !content.contains("${") {
                return ParameterValue::Resolved(content.to_string());
            }
            // Has interpolation - treat as unresolved
            return ParameterValue::Unresolved(trimmed.to_string());
        }

        // Boolean literals
        if trimmed == "true" || trimmed == "false" {
            return ParameterValue::Resolved(trimmed.to_string());
        }

        // Numeric literals (integer or float)
        if trimmed.parse::<f64>().is_ok() {
            return ParameterValue::Resolved(trimmed.to_string());
        }

        // null/undefined literals
        if trimmed == "null" || trimmed == "undefined" {
            return ParameterValue::Resolved(trimmed.to_string());
        }

        // Everything else is unresolved (identifiers, function calls, member expressions, etc.)
        ParameterValue::Unresolved(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_value() {
        // String literals - resolved
        assert!(matches!(
            ArgumentExtractor::classify_value("\"my-bucket\""),
            ParameterValue::Resolved(s) if s == "my-bucket"
        ));
        assert!(matches!(
            ArgumentExtractor::classify_value("'my-bucket'"),
            ParameterValue::Resolved(s) if s == "my-bucket"
        ));

        // Template literal without interpolation - resolved
        assert!(matches!(
            ArgumentExtractor::classify_value("`my-bucket`"),
            ParameterValue::Resolved(s) if s == "my-bucket"
        ));

        // Template literal with interpolation - unresolved
        assert!(matches!(
            ArgumentExtractor::classify_value("`bucket-${name}`"),
            ParameterValue::Unresolved(_)
        ));

        // Boolean literals - resolved
        assert!(matches!(
            ArgumentExtractor::classify_value("true"),
            ParameterValue::Resolved(s) if s == "true"
        ));
        assert!(matches!(
            ArgumentExtractor::classify_value("false"),
            ParameterValue::Resolved(s) if s == "false"
        ));

        // Numeric literals - resolved
        assert!(matches!(
            ArgumentExtractor::classify_value("42"),
            ParameterValue::Resolved(s) if s == "42"
        ));
        assert!(matches!(
            ArgumentExtractor::classify_value("3.14"),
            ParameterValue::Resolved(s) if s == "3.14"
        ));

        // null/undefined - resolved
        assert!(matches!(
            ArgumentExtractor::classify_value("null"),
            ParameterValue::Resolved(s) if s == "null"
        ));
        assert!(matches!(
            ArgumentExtractor::classify_value("undefined"),
            ParameterValue::Resolved(s) if s == "undefined"
        ));

        // Identifiers - unresolved
        assert!(matches!(
            ArgumentExtractor::classify_value("bucketName"),
            ParameterValue::Unresolved(s) if s == "bucketName"
        ));

        // Member expressions - unresolved
        assert!(matches!(
            ArgumentExtractor::classify_value("config.bucket"),
            ParameterValue::Unresolved(s) if s == "config.bucket"
        ));

        // Function calls - unresolved
        assert!(matches!(
            ArgumentExtractor::classify_value("getBucket()"),
            ParameterValue::Unresolved(s) if s == "getBucket()"
        ));
    }

    #[test]
    fn test_parse_object_literal_with_resolution() {
        // Test regular properties with mixed resolved/unresolved
        let params = ArgumentExtractor::parse_object_literal_with_resolution(
            r#"{ Bucket: "my-bucket", Key: fileName, Count: 5 }"#,
        );

        assert_eq!(params.len(), 3);

        // Verify Bucket (resolved)
        if let Parameter::Keyword {
            name,
            value,
            position,
            ..
        } = &params[0]
        {
            assert_eq!(name, "Bucket");
            assert!(matches!(value, ParameterValue::Resolved(s) if s == "my-bucket"));
            assert_eq!(*position, 0);
        } else {
            panic!("Expected Keyword parameter");
        }

        // Verify Key (unresolved)
        if let Parameter::Keyword {
            name,
            value,
            position,
            ..
        } = &params[1]
        {
            assert_eq!(name, "Key");
            assert!(matches!(value, ParameterValue::Unresolved(s) if s == "fileName"));
            assert_eq!(*position, 1);
        } else {
            panic!("Expected Keyword parameter");
        }

        // Verify Count (resolved)
        if let Parameter::Keyword {
            name,
            value,
            position,
            ..
        } = &params[2]
        {
            assert_eq!(name, "Count");
            assert!(matches!(value, ParameterValue::Resolved(s) if s == "5"));
            assert_eq!(*position, 2);
        } else {
            panic!("Expected Keyword parameter");
        }
    }

    #[test]
    fn test_parse_shorthand_properties() {
        // Test shorthand: { client } means { client: client }
        let params = ArgumentExtractor::parse_object_literal_with_resolution("{ client, region }");

        assert_eq!(params.len(), 2);

        // Verify client shorthand
        if let Parameter::Keyword { name, value, .. } = &params[0] {
            assert_eq!(name, "client");
            assert!(matches!(value, ParameterValue::Unresolved(s) if s == "client"));
        } else {
            panic!("Expected Keyword parameter");
        }

        // Verify region shorthand
        if let Parameter::Keyword { name, value, .. } = &params[1] {
            assert_eq!(name, "region");
            assert!(matches!(value, ParameterValue::Unresolved(s) if s == "region"));
        } else {
            panic!("Expected Keyword parameter");
        }
    }

    #[test]
    fn test_parse_nested_objects() {
        // Test nested object literals
        let params = ArgumentExtractor::parse_object_literal_with_resolution(
            r#"{ Bucket: "test", Metadata: { Key: "value" } }"#,
        );

        assert_eq!(params.len(), 2);

        // Verify Bucket
        if let Parameter::Keyword { name, value, .. } = &params[0] {
            assert_eq!(name, "Bucket");
            assert!(matches!(value, ParameterValue::Resolved(s) if s == "test"));
        } else {
            panic!("Expected Keyword parameter");
        }

        // Verify Metadata (nested object should be unresolved)
        if let Parameter::Keyword { name, value, .. } = &params[1] {
            assert_eq!(name, "Metadata");
            assert!(matches!(value, ParameterValue::Unresolved(s) if s.contains("Key")));
        } else {
            panic!("Expected Keyword parameter");
        }
    }

    #[test]
    fn test_split_object_properties() {
        // Test basic splitting
        let props = ArgumentExtractor::split_object_properties(r#"Bucket: "test", Key: fileName"#);
        assert_eq!(props.len(), 2);
        assert_eq!(props[0], r#"Bucket: "test""#);
        assert_eq!(props[1], "Key: fileName");

        // Test nested objects
        let props = ArgumentExtractor::split_object_properties(
            r#"Bucket: "test", Config: { RetryMode: "standard" }, Key: "file""#,
        );
        assert_eq!(props.len(), 3);
        assert!(props[1].contains("RetryMode"));

        // Test with arrays
        let props = ArgumentExtractor::split_object_properties(r#"Items: [1, 2, 3], Name: "test""#);
        assert_eq!(props.len(), 2);
        assert!(props[0].contains("[1, 2, 3]"));
    }

    #[test]
    fn test_normalize_property_key() {
        assert_eq!(
            ArgumentExtractor::normalize_property_key("Bucket"),
            "Bucket"
        );
        assert_eq!(
            ArgumentExtractor::normalize_property_key("\"Bucket\""),
            "Bucket"
        );
        assert_eq!(ArgumentExtractor::normalize_property_key("'Key'"), "Key");
        assert_eq!(
            ArgumentExtractor::normalize_property_key(" Region "),
            "Region"
        );
    }

    #[test]
    fn test_find_property_separator() {
        // Basic case
        assert_eq!(
            ArgumentExtractor::find_property_separator("Bucket: value"),
            Some(6)
        );

        // Colon in string should be ignored
        assert_eq!(
            ArgumentExtractor::find_property_separator(r#"Key: "file:name""#),
            Some(3)
        );

        // Colon in nested object should be ignored
        assert_eq!(
            ArgumentExtractor::find_property_separator("Config: { Mode: 'test' }"),
            Some(6)
        );

        // No separator (shorthand)
        assert_eq!(ArgumentExtractor::find_property_separator("client"), None);
    }

    #[test]
    fn test_empty_object() {
        let params = ArgumentExtractor::parse_object_literal_with_resolution("{}");
        assert!(params.is_empty());

        let params = ArgumentExtractor::parse_object_literal_with_resolution("  {  }  ");
        assert!(params.is_empty());
    }
}
