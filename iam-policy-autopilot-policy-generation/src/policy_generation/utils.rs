//! ARN pattern parsing and placeholder replacement functionality
//!
//! This module provides functionality to parse ARN patterns and replace placeholder variables
//! with actual values or wildcards. Placeholder variables are in the format ${VariableName}.

use crate::errors::{ExtractorError, Result};
use regex::{Captures, Regex};
use std::sync::OnceLock;

/// Regex pattern to match ARN placeholder variables in the format ${VariableName}
static ARN_PLACEHOLDER_REGEX: OnceLock<Regex> = OnceLock::new();

/// Get the compiled regex for ARN placeholder matching
fn get_placeholder_regex() -> &'static Regex {
    ARN_PLACEHOLDER_REGEX
        .get_or_init(|| Regex::new(r"\$\{([^}]+)\}").expect("Invalid ARN placeholder regex"))
}

/// Process a value by replacing placeholder variables with case-insensitive matching
///
/// Replaces the following placeholders (case-insensitive):
/// - ${partition} or ${Partition} -> provided partition value
/// - ${region} or ${Region} -> provided region value
/// - ${account} or ${Account} -> provided account value
/// - All other ${...} -> "*" (wildcard)
///
/// # Arguments
/// * `value` - The value containing placeholder variables
/// * `partition` - The partition value to substitute
/// * `region` - The region value to substitute
/// * `account` - The account value to substitute
///
/// # Returns
/// A tuple containing the processed value and a boolean indicating
/// whether wildcards were introduced (true if any unknown placeholders were replaced with "*")
///
/// # Errors
/// Returns an error if the value contains invalid placeholders (e.g., empty placeholders like ${})
fn process_placeholder_value(
    value: &str,
    partition: &str,
    region: &str,
    account: &str,
) -> Result<(String, bool)> {
    // Check for empty placeholders like ${}
    if value.contains("${}") {
        return Err(ExtractorError::policy_generation(format!(
            "Invalid value '{value}': contains empty placeholder ${{}}"
        )));
    }

    let regex = get_placeholder_regex();
    let mut wildcards_introduced = false;

    let result = regex
        .replace_all(value, |caps: &Captures| {
            if let Some(placeholder) = caps.get(1).map(|m| m.as_str()) {
                match placeholder.to_lowercase().as_str() {
                    "partition" => {
                        if partition == "*" {
                            wildcards_introduced = true;
                        }
                        partition
                    }
                    "region" => {
                        if region == "*" {
                            wildcards_introduced = true;
                        }
                        region
                    }
                    "account" => {
                        if account == "*" {
                            wildcards_introduced = true;
                        }
                        account
                    }
                    _ => {
                        wildcards_introduced = true;
                        "*" // All other variables become wildcards
                    }
                }
            } else {
                wildcards_introduced = true;
                "*" // Fallback (should not happen due to validation)
            }
        })
        .to_string();

    Ok((result, wildcards_introduced))
}

/// ARN pattern processor for replacing placeholder variables
#[derive(Debug, Clone)]
pub(crate) struct ArnParser<'a> {
    /// AWS partition (e.g., "aws", "aws-cn", "aws-us-gov")
    partition: &'a str,
    /// AWS region (e.g., "us-east-1", "eu-west-1")
    region: &'a str,
    /// AWS account number (e.g., "123456789012")
    account: &'a str,
}

impl<'a> ArnParser<'a> {
    /// Create a new ARN parser with AWS context
    pub(crate) fn new(partition: &'a str, region: &'a str, account: &'a str) -> Self {
        Self {
            partition,
            region,
            account,
        }
    }

    /// Process an ARN pattern by replacing placeholder variables
    ///
    /// Replaces the following placeholders (case-insensitive):
    /// - ${Partition} or ${partition} -> provided partition value
    /// - ${Region} or ${region} -> provided region value
    /// - ${Account} or ${account} -> provided account value
    /// - All other ${...} -> "*" (wildcard)
    ///
    /// # Arguments
    /// * `pattern` - The ARN pattern containing placeholder variables
    ///
    /// # Returns
    /// The processed ARN pattern with placeholders replaced
    ///
    /// # Errors
    /// Returns an error if the pattern contains invalid placeholders (e.g., empty placeholders like ${})
    pub(crate) fn process_arn_pattern(&self, pattern: &str) -> Result<String> {
        let (result, _wildcards_introduced) =
            process_placeholder_value(pattern, self.partition, self.region, self.account)?;
        Ok(result)
    }

    /// Process multiple ARN patterns
    ///
    /// # Arguments
    /// * `patterns` - A slice of ARN patterns to process
    ///
    /// # Returns
    /// A vector of processed ARN patterns
    ///
    /// # Errors
    /// Returns an error if any pattern contains invalid placeholders
    pub(crate) fn process_arn_patterns(&self, patterns: &[String]) -> Result<Vec<String>> {
        patterns
            .iter()
            .map(|pattern| self.process_arn_pattern(pattern))
            .collect()
    }
}

/// Condition value processor for replacing placeholder variables in condition values
#[derive(Debug, Clone)]
pub(crate) struct ConditionValueProcessor<'a> {
    /// AWS partition (e.g., "aws", "aws-cn", "aws-us-gov")
    partition: &'a str,
    /// AWS region (e.g., "us-east-1", "eu-west-1")
    region: &'a str,
    /// AWS account number (e.g., "123456789012")
    account: &'a str,
}

impl<'a> ConditionValueProcessor<'a> {
    /// Create a new condition value processor with AWS context
    pub(crate) fn new(partition: &'a str, region: &'a str, account: &'a str) -> Self {
        Self {
            partition,
            region,
            account,
        }
    }

    /// Process a condition value by replacing placeholder variables
    ///
    /// Replaces the following placeholders (case-insensitive):
    /// - ${partition} or ${Partition} -> provided partition value
    /// - ${region} or ${Region} -> provided region value
    /// - ${account} or ${Account} -> provided account value
    /// - All other ${...} -> "*" (wildcard)
    ///
    /// # Arguments
    /// * `value` - The condition value containing placeholder variables
    ///
    /// # Returns
    /// A tuple containing the processed condition value and a boolean indicating
    /// whether wildcards were introduced (true if any unknown placeholders were replaced with "*")
    ///
    /// # Errors
    /// Returns an error if the value contains invalid placeholders (e.g., empty placeholders like ${})
    pub(crate) fn process_condition_value(&self, value: &str) -> Result<(String, bool)> {
        process_placeholder_value(value, self.partition, self.region, self.account)
    }

    /// Process multiple condition values
    ///
    /// # Arguments
    /// * `values` - A slice of condition values to process
    ///
    /// # Returns
    /// A tuple containing a vector of processed condition values and a boolean indicating
    /// whether wildcards were introduced in any of the values
    ///
    /// # Errors
    /// Returns an error if any value contains invalid placeholders
    pub(crate) fn process_condition_values(
        &self,
        values: &[String],
    ) -> Result<(Vec<String>, bool)> {
        let mut processed_values = Vec::new();
        let mut any_wildcards_introduced = false;

        for value in values {
            let (processed_value, wildcards_introduced) = self.process_condition_value(value)?;
            processed_values.push(processed_value);
            if wildcards_introduced {
                any_wildcards_introduced = true;
            }
        }

        Ok((processed_values, any_wildcards_introduced))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_parser() -> ArnParser<'static> {
        ArnParser::new("aws", "us-east-1", "123456789012")
    }

    #[test]
    fn test_process_arn_pattern_basic() {
        let parser = create_test_parser();
        let pattern = "arn:${Partition}:s3:${Region}:${Account}:bucket/${BucketName}";
        let result = parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/*");
    }

    #[test]
    fn test_process_arn_pattern_s3_examples() {
        let parser = create_test_parser();

        // S3 bucket ARN
        let bucket_pattern = "arn:${Partition}:s3:::${BucketName}";
        let bucket_result = parser.process_arn_pattern(bucket_pattern).unwrap();
        assert_eq!(bucket_result, "arn:aws:s3:::*");

        // S3 object ARN
        let object_pattern = "arn:${Partition}:s3:::${BucketName}/${ObjectName}";
        let object_result = parser.process_arn_pattern(object_pattern).unwrap();
        assert_eq!(object_result, "arn:aws:s3:::*/*");

        // S3 access point ARN
        let access_point_pattern =
            "arn:${Partition}:s3:${Region}:${Account}:accesspoint/${AccessPointName}";
        let access_point_result = parser.process_arn_pattern(access_point_pattern).unwrap();
        assert_eq!(
            access_point_result,
            "arn:aws:s3:us-east-1:123456789012:accesspoint/*"
        );
    }

    #[test]
    fn test_process_arn_pattern_no_placeholders() {
        let parser = create_test_parser();
        let pattern = "arn:aws:s3:::my-bucket/*";
        let result = parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:::my-bucket/*");
    }

    #[test]
    fn test_process_multiple_arn_patterns() {
        let parser = create_test_parser();
        let patterns = vec![
            "arn:${Partition}:s3:::${BucketName}".to_string(),
            "arn:${Partition}:s3:::${BucketName}/${ObjectName}".to_string(),
        ];
        let results = parser.process_arn_patterns(&patterns).unwrap();
        assert_eq!(results, vec!["arn:aws:s3:::*", "arn:aws:s3:::*/*",]);
    }

    #[test]
    fn test_different_aws_partitions() {
        // Test China partition
        let china_parser = ArnParser::new("aws-cn", "cn-north-1", "123456789012");
        let pattern = "arn:${Partition}:s3:${Region}:${Account}:bucket/${BucketName}";
        let result = china_parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws-cn:s3:cn-north-1:123456789012:bucket/*");

        // Test GovCloud partition
        let gov_parser = ArnParser::new("aws-us-gov", "us-gov-west-1", "123456789012");
        let result = gov_parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(
            result,
            "arn:aws-us-gov:s3:us-gov-west-1:123456789012:bucket/*"
        );
    }

    #[test]
    fn test_edge_cases() {
        let parser = create_test_parser();

        // Empty string
        assert_eq!(parser.process_arn_pattern("").unwrap(), "");

        // Only placeholders
        assert_eq!(parser.process_arn_pattern("${Partition}").unwrap(), "aws");
        assert_eq!(
            parser.process_arn_pattern("${Region}").unwrap(),
            "us-east-1"
        );
        assert_eq!(
            parser.process_arn_pattern("${Account}").unwrap(),
            "123456789012"
        );
        assert_eq!(parser.process_arn_pattern("${Unknown}").unwrap(), "*");

        // Malformed placeholders (should not be replaced)
        assert_eq!(parser.process_arn_pattern("${").unwrap(), "${");
        assert_eq!(parser.process_arn_pattern("}").unwrap(), "}");
    }

    #[test]
    fn test_empty_placeholder_validation() {
        let parser = create_test_parser();

        // Empty placeholder should result in an error
        let result = parser.process_arn_pattern("arn:${Partition}:s3:${}:bucket");
        assert!(result.is_err());

        if let Err(ExtractorError::PolicyGeneration { message, .. }) = result {
            assert!(message.contains("empty placeholder"));
            assert!(message.contains("${}"));
        } else {
            panic!("Expected PolicyGeneration error for empty placeholder");
        }

        // Multiple empty placeholders should also fail
        let result = parser.process_arn_pattern("arn:${}:s3:${}:bucket");
        assert!(result.is_err());
    }

    fn create_test_condition_processor() -> ConditionValueProcessor<'static> {
        ConditionValueProcessor::new("aws", "us-east-1", "123456789012")
    }

    #[test]
    fn test_process_condition_value_basic() {
        let processor = create_test_condition_processor();

        // Test basic region replacement
        let value = "s3.${region}.amazonaws.com";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-east-1.amazonaws.com");
        assert!(!wildcards_introduced);

        // Test uppercase variant - this should work with case-insensitive matching
        let value = "s3.${Region}.amazonaws.com";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-east-1.amazonaws.com");
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_process_condition_value_multiple_placeholders() {
        let processor = create_test_condition_processor();

        let value = "arn:${partition}:s3:${region}:${account}:bucket/test";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/test");
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_process_condition_value_unknown_placeholder() {
        let processor = create_test_condition_processor();

        let value = "s3.${region}.amazonaws.com/${unknown}";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-east-1.amazonaws.com/*");
        assert!(wildcards_introduced);
    }

    #[test]
    fn test_process_condition_value_no_placeholders() {
        let processor = create_test_condition_processor();

        let value = "s3.us-west-2.amazonaws.com";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-west-2.amazonaws.com");
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_process_condition_values_multiple() {
        let processor = create_test_condition_processor();

        let values = vec![
            "s3.${region}.amazonaws.com".to_string(),
            "dynamodb.${region}.amazonaws.com".to_string(),
            "ec2.${region}.amazonaws.com".to_string(),
        ];

        let (results, wildcards_introduced) = processor.process_condition_values(&values).unwrap();
        assert_eq!(
            results,
            vec![
                "s3.us-east-1.amazonaws.com",
                "dynamodb.us-east-1.amazonaws.com",
                "ec2.us-east-1.amazonaws.com",
            ]
        );
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_process_condition_value_empty_placeholder() {
        let processor = create_test_condition_processor();

        let result = processor.process_condition_value("s3.${}.amazonaws.com");
        assert!(result.is_err());

        if let Err(ExtractorError::PolicyGeneration { message, .. }) = result {
            assert!(message.contains("empty placeholder"));
            assert!(message.contains("${}"));
        } else {
            panic!("Expected PolicyGeneration error for empty placeholder");
        }
    }

    #[test]
    fn test_condition_processor_different_regions() {
        // Test with different region
        let processor = ConditionValueProcessor::new("aws", "eu-west-1", "987654321098");

        let value = "s3.${region}.amazonaws.com";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.eu-west-1.amazonaws.com");
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_condition_processor_china_partition() {
        // Test with China partition
        let processor = ConditionValueProcessor::new("aws-cn", "cn-north-1", "123456789012");

        let value = "s3.${region}.amazonaws.com.cn";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.cn-north-1.amazonaws.com.cn");
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_process_condition_values_with_wildcards() {
        let processor = create_test_condition_processor();

        let values = vec![
            "s3.${region}.amazonaws.com".to_string(), // Known placeholder, no wildcards
            "dynamodb.${unknown}.amazonaws.com".to_string(), // Unknown placeholder, introduces wildcards
            "ec2.${region}.amazonaws.com".to_string(),       // Known placeholder, no wildcards
        ];

        let (results, wildcards_introduced) = processor.process_condition_values(&values).unwrap();
        assert_eq!(
            results,
            vec![
                "s3.us-east-1.amazonaws.com",
                "dynamodb.*.amazonaws.com",
                "ec2.us-east-1.amazonaws.com",
            ]
        );
        // Should be true because at least one value introduced wildcards
        assert!(wildcards_introduced);
    }

    #[test]
    fn test_process_condition_values_no_wildcards() {
        let processor = create_test_condition_processor();

        let values = vec![
            "s3.${region}.amazonaws.com".to_string(),
            "dynamodb.${partition}.amazonaws.com".to_string(),
            "ec2.${account}.amazonaws.com".to_string(),
        ];

        let (results, wildcards_introduced) = processor.process_condition_values(&values).unwrap();
        assert_eq!(
            results,
            vec![
                "s3.us-east-1.amazonaws.com",
                "dynamodb.aws.amazonaws.com",
                "ec2.123456789012.amazonaws.com",
            ]
        );
        // Should be false because no wildcards were introduced
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_case_insensitive_arn_patterns() {
        let parser = create_test_parser();

        // Test mixed case placeholders in ARN patterns
        let pattern = "arn:${partition}:s3:${Region}:${ACCOUNT}:bucket/${BucketName}";
        let result = parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/*");

        // Test all uppercase
        let pattern = "arn:${PARTITION}:s3:${REGION}:${ACCOUNT}:bucket/${BucketName}";
        let result = parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/*");

        // Test mixed case variations
        let pattern = "arn:${Partition}:s3:${region}:${Account}:bucket/${BucketName}";
        let result = parser.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/*");
    }

    #[test]
    fn test_case_insensitive_condition_values() {
        let processor = create_test_condition_processor();

        // Test mixed case placeholders in condition values
        let value = "arn:${Partition}:s3:${region}:${ACCOUNT}:bucket/test";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:123456789012:bucket/test");
        assert!(!wildcards_introduced);

        // Test all uppercase
        let value = "s3.${REGION}.amazonaws.com";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-east-1.amazonaws.com");
        assert!(!wildcards_introduced);

        // Test mixed case with unknown placeholder
        let value = "s3.${Region}.amazonaws.com/${UnknownPlaceholder}";
        let (result, wildcards_introduced) = processor.process_condition_value(value).unwrap();
        assert_eq!(result, "s3.us-east-1.amazonaws.com/*");
        assert!(wildcards_introduced);
    }

    #[test]
    fn test_case_insensitive_multiple_values() {
        let processor = create_test_condition_processor();

        let values = vec![
            "s3.${REGION}.amazonaws.com".to_string(),
            "dynamodb.${region}.amazonaws.com".to_string(),
            "ec2.${Region}.amazonaws.com".to_string(),
        ];

        let (results, wildcards_introduced) = processor.process_condition_values(&values).unwrap();
        assert_eq!(
            results,
            vec![
                "s3.us-east-1.amazonaws.com",
                "dynamodb.us-east-1.amazonaws.com",
                "ec2.us-east-1.amazonaws.com",
            ]
        );
        assert!(!wildcards_introduced);
    }

    #[test]
    fn test_wildcards_introduced_when_partition_region_account_is_wildcard() {
        // Test when partition is "*"
        let processor_wildcard_partition =
            ConditionValueProcessor::new("*", "us-east-1", "123456789012");

        let value = "arn:${partition}:s3:${region}:${account}:bucket/test";
        let (result, wildcards_introduced) = processor_wildcard_partition
            .process_condition_value(value)
            .unwrap();
        assert_eq!(result, "arn:*:s3:us-east-1:123456789012:bucket/test");
        assert!(wildcards_introduced);

        // Test when region is "*"
        let processor_wildcard_region = ConditionValueProcessor::new("aws", "*", "123456789012");

        let (result, wildcards_introduced) = processor_wildcard_region
            .process_condition_value(value)
            .unwrap();
        assert_eq!(result, "arn:aws:s3:*:123456789012:bucket/test");
        assert!(wildcards_introduced);

        // Test when account is "*"
        let processor_wildcard_account = ConditionValueProcessor::new("aws", "us-east-1", "*");

        let (result, wildcards_introduced) = processor_wildcard_account
            .process_condition_value(value)
            .unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:*:bucket/test");
        assert!(wildcards_introduced);

        // Test when multiple are "*"
        let processor_multiple_wildcards = ConditionValueProcessor::new("*", "*", "123456789012");

        let (result, wildcards_introduced) = processor_multiple_wildcards
            .process_condition_value(value)
            .unwrap();
        assert_eq!(result, "arn:*:s3:*:123456789012:bucket/test");
        assert!(wildcards_introduced);

        // Test when all are "*"
        let processor_all_wildcards = ConditionValueProcessor::new("*", "*", "*");

        let (result, wildcards_introduced) = processor_all_wildcards
            .process_condition_value(value)
            .unwrap();
        assert_eq!(result, "arn:*:s3:*:*:bucket/test");
        assert!(wildcards_introduced);
    }

    #[test]
    fn test_arn_parser_wildcards_introduced_when_partition_region_account_is_wildcard() {
        // Test when partition is "*"
        let parser_wildcard_partition = ArnParser::new("*", "us-east-1", "123456789012");

        let pattern = "arn:${Partition}:s3:${Region}:${Account}:bucket/${BucketName}";
        let result = parser_wildcard_partition
            .process_arn_pattern(pattern)
            .unwrap();
        assert_eq!(result, "arn:*:s3:us-east-1:123456789012:bucket/*");

        // Test when region is "*"
        let parser_wildcard_region = ArnParser::new("aws", "*", "123456789012");

        let result = parser_wildcard_region.process_arn_pattern(pattern).unwrap();
        assert_eq!(result, "arn:aws:s3:*:123456789012:bucket/*");

        // Test when account is "*"
        let parser_wildcard_account = ArnParser::new("aws", "us-east-1", "*");

        let result = parser_wildcard_account
            .process_arn_pattern(pattern)
            .unwrap();
        assert_eq!(result, "arn:aws:s3:us-east-1:*:bucket/*");
    }
}
