//! Utility functions for string-based AccessDenied message parsing.

use regex::Regex;
use std::sync::LazyLock;

/// Compiled regex for ARN validation - only place we use regex in parsing
static ARN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"arn:aws(?:-[a-z]+)*:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:\d{12}:[^\s\x22]+")
        .expect("Valid ARN regex pattern")
});

pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
}

/// Validates AWS ARN format using basic pattern matching.
pub fn is_arn(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let value = value.trim();
    if !value.starts_with("arn:aws:") && !value.starts_with("arn:aws-") {
        return false;
    }

    let parts: Vec<&str> = value.splitn(6, ':').collect();
    if parts.len() < 6 {
        return false;
    }

    let partition = parts[1];
    let service = parts[2];
    let region = parts[3];
    let account = parts[4];
    let resource = parts[5];

    if partition != "aws" && !partition.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return false;
    }
    if service.is_empty() || !service.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return false;
    }
    if resource.is_empty() {
        return false;
    }

    match service {
        "s3" => {
            (region.is_empty() && account.is_empty())
                || (region.is_empty() && !account.is_empty() && is_account_id(account))
                || (!region.is_empty() && !account.is_empty() && is_account_id(account))
        }
        "iam" => region.is_empty() && (account == "aws" || is_account_id(account)),
        _ => account.is_empty() || is_account_id(account),
    }
}

/// Finds the first ARN in the message, typically the user/role principal.
pub fn extract_principal(message: &str) -> Option<String> {
    if message.is_empty() {
        return None;
    }
    ARN_PATTERN.find(message).map(|m| m.as_str().to_string())
}

/// Splits on ' is not authorized to perform: ' pattern to extract the action.
pub fn extract_action(message: &str) -> Option<String> {
    if message.is_empty() {
        return None;
    }
    let marker = " is not authorized to perform:";
    let parts: Vec<&str> = message.splitn(2, marker).collect();
    if parts.len() < 2 {
        return None;
    }
    let after_marker = parts[1].trim();
    if after_marker.is_empty() {
        return None;
    }

    let because_marker = " because";
    let explicit_deny_marker = " with an explicit deny";
    let action_end_markers = [
        " on resource:",
        " on role ",
        " on user ",
        " on policy ",
        " on group ",
        because_marker,
        explicit_deny_marker,
    ];

    let mut action = after_marker;
    for end_marker in &action_end_markers {
        if let Some(pos) = after_marker.find(end_marker) {
            action = &after_marker[..pos];
            break;
        }
    }
    let action = action.trim().trim_matches(|c| c == '"' || c == '\'');
    if action.is_empty() {
        return None;
    }
    if action.contains(':') && !action.starts_with("on resource") {
        Some(action.to_string())
    } else {
        None
    }
}

/// Extract resource after the standard marker or fallbacks.
pub fn extract_resource(message: &str) -> Option<String> {
    if message.is_empty() {
        return None;
    }
    let marker = " on resource:";
    if let Some(pos) = message.to_lowercase().find(marker) {
        let after = &message[pos + marker.len()..];
        let trimmed = after.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Resource is typically the next token or a quoted string
        if let Some(stripped) = trimmed.strip_prefix('"') {
            // Quoted resource
            if let Some(end) = stripped.find('"') {
                let resource = stripped[..end].to_string();
                return Some(resource);
            }
        } else {
            // Take until whitespace or punctuation
            let candidates = [" ", ".", ",", ";"];
            let mut end = trimmed.len();
            for c in &candidates {
                if let Some(i) = trimmed.find(c) {
                    end = end.min(i);
                }
            }
            let first_part = trimmed[..end].trim();
            if !first_part.is_empty() {
                if is_arn(first_part) || first_part == "*" {
                    return Some(first_part.to_string());
                }
                if matches!(first_part, "role" | "user" | "policy" | "group") {
                    let resource_parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if resource_parts.len() >= 2 {
                        let resource_name =
                            resource_parts[1].trim_end_matches(&['.', ',', ';'] as &[_]);
                        let formatted = format!("arn:*:iam::*:{}/{}", first_part, resource_name);
                        return Some(formatted);
                    }
                }
                return Some(first_part.to_string());
            }
        }
    }

    // Fallback: second ARN in the message
    let principal = extract_principal(message);
    let arns: Vec<&str> = ARN_PATTERN.find_iter(message).map(|m| m.as_str()).collect();
    if arns.len() >= 2 {
        return Some(arns[1].to_string());
    }

    // Context-based fallback for list/describe
    if let Some(action) = extract_action(message) {
        let service = action.split(':').next().unwrap_or("");
        match service.to_lowercase().as_str() {
            "s3" if action.to_lowercase().contains("list") => {
                return Some("*".to_string());
            }
            "ec2" if action.to_lowercase().contains("describe") => {
                return Some("*".to_string());
            }
            _ => {}
        }
    }
    // If we saw a principal earlier, return it as a last resort
    principal
}

/// Parses denial reason from trailing context.
pub fn extract_context(message: &str) -> String {
    if message.is_empty() {
        return String::new();
    }
    let context_markers = [" because ", " with an explicit deny"]; // lowercased search below
    for marker in &context_markers {
        if let Some(pos) = message.to_lowercase().find(marker) {
            return message[pos + marker.len()..].trim().to_string();
        }
    }
    String::new()
}

/// Detect explicit vs implicit denial patterns.
pub fn is_explicit_deny(message: &str) -> bool {
    if message.is_empty() {
        return false;
    }
    let msg_lower = message.to_lowercase();
    let explicit_patterns = [
        "explicit deny",
        "with an explicit deny",
        "denied by an explicit deny",
    ];
    explicit_patterns.iter().any(|p| msg_lower.contains(p))
}

/// Checks if an IAM action operates on S3 objects (vs buckets).
///
/// Returns true for S3 object-level operations that should use bucket wildcard resources.
/// Returns false for bucket-level operations and non-S3 actions.
///
/// Uses a deterministic allowlist of S3 object operations based on AWS IAM documentation.
pub fn is_s3_object_operation(action: &str) -> bool {
    // Hardcoded allowlist of S3 object-level actions
    const S3_OBJECT_ACTIONS: &[&str] = &[
        // Read operations
        "s3:GetObject",
        "s3:GetObjectVersion",
        "s3:GetObjectAcl",
        "s3:GetObjectAttributes",
        "s3:GetObjectLegalHold",
        "s3:GetObjectRetention",
        "s3:GetObjectTagging",
        "s3:GetObjectTorrent",
        "s3:GetObjectVersionAcl",
        "s3:GetObjectVersionAttributes",
        "s3:GetObjectVersionTagging",
        "s3:GetObjectVersionTorrent",
        // Write operations
        "s3:PutObject",
        "s3:PutObjectAcl",
        "s3:PutObjectLegalHold",
        "s3:PutObjectRetention",
        "s3:PutObjectTagging",
        "s3:PutObjectVersionAcl",
        "s3:PutObjectVersionTagging",
        // Delete operations
        "s3:DeleteObject",
        "s3:DeleteObjectVersion",
        "s3:DeleteObjectTagging",
        "s3:DeleteObjectVersionTagging",
        // Other operations
        "s3:RestoreObject",
        "s3:AbortMultipartUpload",
    ];

    S3_OBJECT_ACTIONS.contains(&action)
}

/// Normalizes S3 resource ARNs for object operations to bucket wildcard patterns.
///
/// For S3 object-level operations (e.g., GetObject, PutObject), transforms object-specific
/// ARNs like `arn:aws:s3:::bucket/path/file.txt` to bucket wildcard `arn:aws:s3:::bucket/*`.
/// This prevents policy bloat from individual object ARNs while maintaining proper permissions.
///
/// # Arguments
/// * `action` - The IAM action (e.g., "s3:GetObject")
/// * `resource` - The resource ARN to potentially normalize
///
/// # Returns
/// The normalized resource ARN, or the original if no transformation is needed.
///
/// # Examples
/// ```
/// use iam_policy_autopilot_access_denied::normalize_s3_resource;
///
/// // Object operation with object ARN -> bucket wildcard
/// let result = normalize_s3_resource("s3:GetObject", "arn:aws:s3:::bucket/file.txt");
/// assert_eq!(result, "arn:aws:s3:::bucket/*");
///
/// // Bucket operation -> no change
/// let result = normalize_s3_resource("s3:ListBucket", "arn:aws:s3:::bucket/file.txt");
/// assert_eq!(result, "arn:aws:s3:::bucket/file.txt");
///
/// // Non-S3 action -> no change
/// let result = normalize_s3_resource("dynamodb:GetItem", "arn:aws:dynamodb:us-east-1:123:table/T");
/// assert_eq!(result, "arn:aws:dynamodb:us-east-1:123:table/T");
/// ```
pub fn normalize_s3_resource(action: &str, resource: &str) -> String {
    // Only normalize for S3 object operations
    if !is_s3_object_operation(action) {
        return resource.to_string();
    }

    // Check if it's an S3 ARN
    if !resource.starts_with("arn:") {
        return resource.to_string();
    }
    let mut trimmed = &resource["arn:".len()..];
    let partition = if let Some(colon) = trimmed.find(':') {
        &trimmed[..colon]
    } else {
        return resource.to_string();
    };
    if partition != "aws" && !partition.starts_with("aws-") {
        return resource.to_string();
    }
    trimmed = &trimmed[partition.len()..];
    if !trimmed.starts_with(":s3:::") {
        return resource.to_string();
    }
    trimmed = &trimmed[":s3:::".len()..];

    // Extract bucket name and check if this is an object ARN (contains '/')
    // Find the first '/' which indicates this is an object ARN
    if let Some(slash_pos) = trimmed.find('/') {
        let bucket_name = &trimmed[..slash_pos];
        // Return bucket wildcard pattern
        format!("arn:{partition}:s3:::{bucket_name}/*")
    } else {
        // No '/' found - this is already a bucket ARN or wildcard
        resource.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_arn_valid_arns() {
        // Allow IAM ARNs with account IDs or 'aws'
        assert!(is_arn("arn:aws:iam::123456789012:role/MyRole"));
        assert!(is_arn("arn:aws:iam::aws:contextProvider/IdentityCenter"));

        // Allow S3 ARNs without region and account, with account but no region, or with both
        assert!(is_arn("arn:aws:s3:::my-bucket"));
        assert!(is_arn("arn:aws:s3:::my-bucket/my-key"));
        assert!(is_arn(
            "arn:aws:s3::123456789012:accesspoint/test/object/unit-01"
        ));
        assert!(is_arn("arn:aws:s3:us-east-1:123456789012:job/example-job"));

        // Allow non-commercial AWS partitions and regions
        assert!(is_arn(
            "arn:aws:ec2:us-west-2:123456789012:instance/i-1234567890abcdef0"
        ));
        assert!(is_arn(
            "arn:aws-cn:ec2:cn-north-1:123456789012:instance/i-1234567890abcdef0"
        ));
        assert!(is_arn(
            "arn:aws-us-gov:sns:us-gov-east-1:123456789012:example-sns-topic-name"
        ));
        assert!(is_arn("arn:aws-eusc:sqs:eusc-de-east-1:123456789012:queue"));
    }

    #[test]
    fn test_is_arn_invalid_arns() {
        assert!(!is_arn(""));
        assert!(!is_arn("not-an-arn"));
        assert!(!is_arn("arn:aws:iam"));
        assert!(!is_arn("arn:aws:iam::notvalid:role/MyRole"));
        assert!(!is_arn("arn:aws::123456789012:role/MyRole"));
        assert!(!is_arn("arn:aws:s3:us-east-1:aws:job/example-job"));
    }

    // Tests for extract_principal()

    #[test]
    fn test_extract_principal_present() {
        assert_eq!(
            extract_principal("An error occurred (AccessDenied) when calling the GetWidget operation: User: arn:aws:iam::123456789012:user/mateojackson is not authorized to perform: widgets:GetWidget on resource: my-example-widget"),
            Some("arn:aws:iam::123456789012:user/mateojackson".to_string())
        );
        assert_eq!(
            extract_principal("An error occurred (AccessDenied) when calling the ListRepositories operation: User: arn:aws-cn:iam::123456789012:role/HR is not authorized to perform: codecommit:ListRepositories because no identity-based policy allows the codecommit:ListRepositories action"),
            Some("arn:aws-cn:iam::123456789012:role/HR".to_string())
        );
        assert_eq!(
            extract_principal("An error occurred (AccessDenied) when calling the Publish operation: User: arn:aws-us-gov:sts::111122223333:assumed-role/role-name/role-session-name is not authorized to perform: SNS:Publish on resource: arn:aws:sns:us-east-1:444455556666:role-name-2 with an explicit deny in a VPC endpoint policy transitively through a service control policy"),
            Some("arn:aws-us-gov:sts::111122223333:assumed-role/role-name/role-session-name".to_string())
        );
        assert_eq!(
            extract_principal("An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws-eusc:iam::111122223333:oidc-provider/tokens.actions.githubusercontent.com is not authorized to perform: SNS:Publish on resource: arn:aws:sns:us-east-1:444455556666:role-name-2 with an explicit deny in a VPC endpoint policy transitively through a service control policy"),
            Some("arn:aws-eusc:iam::111122223333:oidc-provider/tokens.actions.githubusercontent.com".to_string())
        );
    }

    #[test]
    fn test_extract_principal_absent() {
        assert_eq!(extract_principal("An error occurred (AccessDeniedException) when calling the GetObject operation: User is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key"), None);
        assert_eq!(extract_principal("An error occurred (AccessDeniedException) when calling the InvokeAgent operation: User is not authorized to perform: bedrock-agentcore-runtime:InvokeAgent"), None);
        assert_eq!(extract_principal("An error occurred (ExpiredToken) when calling the ListBuckets operation: The provided token has expired."), None);
        assert_eq!(extract_principal("An error occurred (AccessDeniedException) when calling the EnableRegion operation: Account region opt status can only be modified through Isengard."), None);
    }

    // Tests for extract_resource()

    #[test]
    fn test_extract_resource_present() {
        assert_eq!(
            extract_resource("An error occurred (AccessDenied) when calling the GetRole operation: User: arn:aws-us-gov:sts::111122223333:assumed-role/role-name/role-session-name is not authorized to perform: iam:GetRole on resource: role second-role-name because no identity-based policy allows the iam:GetRole action"),
            Some("arn:*:iam::*:role/second-role-name".to_string())
        );
        assert_eq!(
            extract_resource("An error occurred (AccessDenied) when calling the DeleteRole operation: User: arn:aws:sts::111122223333:assumed-role/role-name/role-session-name is not authorized to perform: iam:DeleteRole on resource: role ThirdRoleName with an explicit deny in a service control policy"),
            Some("arn:*:iam::*:role/ThirdRoleName".to_string())
        );
        assert_eq!(
            extract_resource("An error occurred (AccessDeniedException) when calling the GetObject operation: User is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key"),
            Some("arn:aws:s3:::my-bucket/my-key".to_string())
        );
        assert_eq!(
            extract_resource("An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws-eusc:iam::111122223333:oidc-provider/tokens.actions.githubusercontent.com is not authorized to perform: SNS:Publish on resource: arn:aws-eusc:sns:eusc-de-east-1:444455556666:role-name-2 with an explicit deny in a VPC endpoint policy transitively through a service control policy"),
            Some("arn:aws-eusc:sns:eusc-de-east-1:444455556666:role-name-2".to_string())
        );
        assert_eq!(
            extract_resource("An error occurred (AccessDeniedException) when calling the GetWidget operation: User: arn:aws:iam::123456789012:user/mateojackson is not authorized to perform: widgets:GetWidget on resource: my-example-widget"),
            Some("my-example-widget".to_string())
        );
    }

    #[test]
    fn test_extract_resource_absent() {
        assert_eq!(
            extract_resource("An error occurred (AccessDeniedException) when calling the EnableRegion operation: Account region opt status can only be modified through Isengard."),
            None
        );
        assert_eq!(
            extract_resource("An error occurred (AccessDeniedException) when calling the InvokeAgent operation: User is not authorized to perform: bedrock-agentcore-runtime:InvokeAgent"),
            None
        );
        // In cases where the resource is absent but the principal is present, the principal will be returned
        assert_eq!(
            extract_resource("An error occurred (AccessDeniedException) when calling the ListRepositories operation: User: arn:aws-cn:iam::123456789012:role/HR is not authorized to perform: codecommit:ListRepositories because no identity-based policy allows the codecommit:ListRepositories action"),
            Some("arn:aws-cn:iam::123456789012:role/HR".to_string())
        );
    }

    // Tests for is_s3_object_operation()

    #[test]
    fn test_is_s3_object_operation_read_operations() {
        assert!(is_s3_object_operation("s3:GetObject"));
        assert!(is_s3_object_operation("s3:GetObjectVersion"));
        assert!(is_s3_object_operation("s3:GetObjectAcl"));
        assert!(is_s3_object_operation("s3:GetObjectAttributes"));
        assert!(is_s3_object_operation("s3:GetObjectTagging"));
    }

    #[test]
    fn test_is_s3_object_operation_write_operations() {
        assert!(is_s3_object_operation("s3:PutObject"));
        assert!(is_s3_object_operation("s3:PutObjectAcl"));
        assert!(is_s3_object_operation("s3:PutObjectTagging"));
        assert!(is_s3_object_operation("s3:PutObjectRetention"));
    }

    #[test]
    fn test_is_s3_object_operation_delete_operations() {
        assert!(is_s3_object_operation("s3:DeleteObject"));
        assert!(is_s3_object_operation("s3:DeleteObjectVersion"));
        assert!(is_s3_object_operation("s3:DeleteObjectTagging"));
    }

    #[test]
    fn test_is_s3_object_operation_bucket_operations() {
        assert!(!is_s3_object_operation("s3:ListBucket"));
        assert!(!is_s3_object_operation("s3:GetBucketPolicy"));
        assert!(!is_s3_object_operation("s3:PutBucketPolicy"));
        assert!(!is_s3_object_operation("s3:DeleteBucket"));
        assert!(!is_s3_object_operation("s3:CreateBucket"));
    }

    #[test]
    fn test_is_s3_object_operation_non_s3_actions() {
        assert!(!is_s3_object_operation("dynamodb:GetItem"));
        assert!(!is_s3_object_operation("ec2:DescribeInstances"));
        assert!(!is_s3_object_operation("iam:GetUser"));
        assert!(!is_s3_object_operation("lambda:InvokeFunction"));
    }

    // Tests for normalize_s3_resource()

    #[test]
    fn test_normalize_s3_resource_object_arn_to_wildcard() {
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws:s3:::bucket/path/file.txt"),
            "arn:aws:s3:::bucket/*"
        );
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws-us-gov:s3:::bucket/path/file.txt"),
            "arn:aws-us-gov:s3:::bucket/*"
        );
    }

    #[test]
    fn test_normalize_s3_resource_nested_path() {
        assert_eq!(
            normalize_s3_resource("s3:PutObject", "arn:aws:s3:::bucket/logs/2024/file.log"),
            "arn:aws:s3:::bucket/*"
        );
        assert_eq!(
            normalize_s3_resource(
                "s3:PutObject",
                "arn:aws-eusc:s3:::bucket/logs/2024/file.log"
            ),
            "arn:aws-eusc:s3:::bucket/*"
        );
    }

    #[test]
    fn test_normalize_s3_resource_bucket_arn_unchanged() {
        // No '/' means it's a bucket ARN, not an object ARN
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws:s3:::bucket"),
            "arn:aws:s3:::bucket"
        );
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws-cn:s3:::bucket"),
            "arn:aws-cn:s3:::bucket"
        );
    }

    #[test]
    fn test_normalize_s3_resource_bucket_operation_unchanged() {
        // ListBucket is a bucket operation, not an object operation
        assert_eq!(
            normalize_s3_resource("s3:ListBucket", "arn:aws:s3:::bucket/file.txt"),
            "arn:aws:s3:::bucket/file.txt"
        );
        assert_eq!(
            normalize_s3_resource("s3:ListBucket", "arn:aws-us-gov:s3:::bucket/file.txt"),
            "arn:aws-us-gov:s3:::bucket/file.txt"
        );
    }

    #[test]
    fn test_normalize_s3_resource_non_s3_arn_unchanged() {
        assert_eq!(
            normalize_s3_resource(
                "dynamodb:GetItem",
                "arn:aws:dynamodb:us-east-1:123456789012:table/MyTable",
            ),
            "arn:aws:dynamodb:us-east-1:123456789012:table/MyTable"
        );
        assert_eq!(
            normalize_s3_resource(
                "dynamodb:GetItem",
                "arn:aws-us-gov:dynamodb:us-gov-west-1:123456789012:table/MyTable",
            ),
            "arn:aws-us-gov:dynamodb:us-gov-west-1:123456789012:table/MyTable"
        );
    }

    #[test]
    fn test_normalize_s3_resource_wildcard_unchanged() {
        // Already a wildcard pattern
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws:s3:::bucket/*"),
            "arn:aws:s3:::bucket/*"
        );
        assert_eq!(
            normalize_s3_resource("s3:GetObject", "arn:aws-cn:s3:::bucket/*"),
            "arn:aws-cn:s3:::bucket/*"
        );
    }

    #[test]
    fn test_normalize_s3_resource_non_arn_unchanged() {
        // Not an ARN format (just wildcard)
        assert_eq!(normalize_s3_resource("s3:GetObject", "*"), "*");
    }
}
