//! Parameter filtering utilities for Python extraction
//!
//! This module provides standardized parameter filtering for removing
//! SDK-specific parameters that shouldn't be included in policy generation.

use crate::extraction::Parameter;

/// Utility for filtering parameters based on common patterns
pub struct ParameterFilter;

impl ParameterFilter {
    /// Filter out pagination-specific parameters
    pub fn filter_pagination_parameters(parameters: Vec<Parameter>) -> Vec<Parameter> {
        parameters
            .into_iter()
            .filter(|param| {
                match param {
                    Parameter::Keyword { name, .. } => {
                        // Filter out known pagination-specific parameters
                        !matches!(
                            name.as_str(),
                            "PaginationConfig" | "StartingToken" | "PageSize"
                        )
                    }
                    // Keep positional arguments and dictionary splats
                    Parameter::Positional { .. } | Parameter::DictionarySplat { .. } => true,
                }
            })
            .collect()
    }

    /// Filter out waiter-specific parameters
    pub fn filter_waiter_parameters(parameters: Vec<Parameter>) -> Vec<Parameter> {
        parameters
            .into_iter()
            .filter(|param| {
                match param {
                    Parameter::Keyword { name, .. } => {
                        // Filter out waiter-specific parameters
                        name != "WaiterConfig"
                    }
                    // Keep positional arguments and dictionary splats
                    Parameter::Positional { .. } | Parameter::DictionarySplat { .. } => true,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::ParameterValue;

    use super::*;

    #[test]
    fn test_filter_pagination_parameters() {
        let parameters = vec![
            Parameter::Keyword {
                name: "Bucket".to_string(),
                value: ParameterValue::Resolved("my-bucket".to_string()),
                position: 0,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "PaginationConfig".to_string(),
                value: ParameterValue::Unresolved("{'MaxItems': 10}".to_string()),
                position: 1,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "Prefix".to_string(),
                value: ParameterValue::Resolved("test/".to_string()),
                position: 2,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "StartingToken".to_string(),
                value: ParameterValue::Unresolved("token".to_string()),
                position: 3,
                type_annotation: None,
            },
        ];

        let filtered = ParameterFilter::filter_pagination_parameters(parameters);

        // Should keep Bucket and Prefix, filter out PaginationConfig and StartingToken
        assert_eq!(filtered.len(), 2);

        let param_names: Vec<String> = filtered
            .iter()
            .filter_map(|param| {
                if let Parameter::Keyword { name, .. } = param {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(param_names.contains(&"Bucket".to_string()));
        assert!(param_names.contains(&"Prefix".to_string()));
        assert!(!param_names.contains(&"PaginationConfig".to_string()));
        assert!(!param_names.contains(&"StartingToken".to_string()));
    }

    #[test]
    fn test_filter_waiter_parameters() {
        let parameters = vec![
            Parameter::Keyword {
                name: "InstanceIds".to_string(),
                value: ParameterValue::Resolved("['i-123']".to_string()),
                position: 0,
                type_annotation: None,
            },
            Parameter::Keyword {
                name: "WaiterConfig".to_string(),
                value: ParameterValue::Unresolved("{'Delay': 15}".to_string()),
                position: 1,
                type_annotation: None,
            },
        ];

        let filtered = ParameterFilter::filter_waiter_parameters(parameters);

        // Should keep InstanceIds, filter out WaiterConfig
        assert_eq!(filtered.len(), 1);

        if let Parameter::Keyword { name, .. } = &filtered[0] {
            assert_eq!(name, "InstanceIds");
        } else {
            panic!("Expected keyword parameter");
        }
    }
}
