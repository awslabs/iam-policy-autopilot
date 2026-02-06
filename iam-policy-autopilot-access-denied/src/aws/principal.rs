//! Principal ARN parsing and resolution

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PrincipalKind {
    Role,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct PrincipalInfo {
    pub kind: PrincipalKind,
    pub name: String,
}

impl PrincipalInfo {
    #[must_use]
    pub fn new(kind: PrincipalKind, name: &str) -> Self {
        Self {
            kind,
            name: name.to_string(),
        }
    }
}

/// Resolve principal information from an ARN (supports IAM role/user and STS assumed-role)
///
/// Returns an error for unsupported principal types:
/// - Root users
/// - Service-linked roles
/// - Federated users
/// - Invalid ARN formats
pub fn resolve_principal(principal_arn: &str) -> Result<PrincipalInfo, String> {
    // Use splitn(6) to avoid splitting on colons in the resource part of the ARN
    // ARN format: arn:partition:service:region:account:resource
    // The resource part may contain colons (e.g., S3 bucket names)
    let parts: Vec<&str> = principal_arn.splitn(6, ':').collect();
    if parts.len() < 6 {
        return Err("invalid ARN format: expected at least 6 colon-separated parts".to_string());
    }
    let service = parts[2];
    let resource = parts[5];
    match service {
        "iam" => resolve_iam_principal(resource),
        "sts" => resolve_sts_principal(resource),
        _ => Err(format!(
            "unsupported service '{service}': only IAM and STS principals are supported"
        )),
    }
}

fn resolve_iam_principal(resource: &str) -> Result<PrincipalInfo, String> {
    if resource == "root" {
        return Err("root user is not supported".to_string());
    }

    let resource_parts: Vec<&str> = resource.split('/').collect();
    if resource_parts.len() < 2 {
        return Err("invalid IAM resource format: expected resource type and name".to_string());
    }
    match resource_parts[0] {
        "role" => {
            let role_path = resource_parts[1..].join("/");
            if role_path.starts_with("aws-service-role/") {
                return Err(
                    "service-linked roles are managed by AWS and cannot be modified".to_string(),
                );
            }
            Ok(PrincipalInfo::new(PrincipalKind::Role, &role_path))
        }
        "user" => {
            let user_path = resource_parts[1..].join("/");
            Ok(PrincipalInfo::new(PrincipalKind::User, &user_path))
        }
        _ => Err(format!(
            "unsupported IAM resource type '{}': only 'role' and 'user' are supported",
            resource_parts[0]
        )),
    }
}

fn resolve_sts_principal(resource: &str) -> Result<PrincipalInfo, String> {
    let resource_parts: Vec<&str> = resource.split('/').collect();
    if resource_parts.len() < 3 {
        return Err("invalid STS resource format: expected at least 3 parts".to_string());
    }
    match resource_parts[0] {
        "assumed-role" => {
            let role_name = resource_parts[1];
            if role_name.starts_with("aws-service-role") {
                return Err(
                    "service-linked roles are managed by AWS and cannot be modified".to_string(),
                );
            }
            Ok(PrincipalInfo::new(PrincipalKind::Role, role_name))
        }
        "federated-user" => Err("federated users are not supported".to_string()),
        _ => Err(format!(
            "unsupported STS resource type '{}': only 'assumed-role' is supported",
            resource_parts[0]
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_iam_role() {
        let arn = "arn:aws:iam::123456789012:role/MyRole";
        let info = resolve_principal(arn).unwrap();
        assert_eq!(info.kind, PrincipalKind::Role);
        assert_eq!(info.name, "MyRole".to_string());
    }

    #[test]
    fn test_resolve_iam_role_with_path() {
        let arn = "arn:aws:iam::123456789012:role/application/MyRole";
        let info = resolve_principal(arn).unwrap();
        assert_eq!(info.kind, PrincipalKind::Role);
        assert_eq!(info.name, "application/MyRole".to_string());
    }

    #[test]
    fn test_resolve_iam_role_with_nested_path() {
        let arn = "arn:aws:iam::123456789012:role/application/dev/MyRole";
        let info = resolve_principal(arn).unwrap();
        assert_eq!(info.kind, PrincipalKind::Role);
        assert_eq!(info.name, "application/dev/MyRole".to_string());
    }

    #[test]
    fn test_resolve_iam_user_with_path() {
        let arn = "arn:aws:iam::123456789012:user/developers/alice";
        let info = resolve_principal(arn).unwrap();
        assert_eq!(info.kind, PrincipalKind::User);
        assert_eq!(info.name, "developers/alice".to_string());
    }

    #[test]
    fn test_resolve_root_user() {
        let arn = "arn:aws:iam::123456789012:root";
        let result = resolve_principal(arn);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "root user is not supported");
    }

    #[test]
    fn test_resolve_service_linked_role() {
        let arn = "arn:aws:iam::123456789012:role/aws-service-role/elasticbeanstalk.amazonaws.com/AWSServiceRoleForElasticBeanstalk";
        let result = resolve_principal(arn);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "service-linked roles are managed by AWS and cannot be modified"
        );
    }

    #[test]
    fn test_resolve_assumed_role_with_path() {
        let arn = "arn:aws:sts::123456789012:assumed-role/MyRole/session-name";
        let info = resolve_principal(arn).unwrap();
        assert_eq!(info.kind, PrincipalKind::Role);
        assert_eq!(info.name, "MyRole".to_string());
    }

    #[test]
    fn test_resolve_assumed_service_linked_role() {
        let arn =
            "arn:aws:sts::123456789012:assumed-role/aws-service-role-elasticbeanstalk/session";
        let result = resolve_principal(arn);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "service-linked roles are managed by AWS and cannot be modified"
        );
    }

    #[test]
    fn test_resolve_federated_user() {
        let arn = "arn:aws:sts::123456789012:federated-user/alice/session";
        let result = resolve_principal(arn);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "federated users are not supported");
    }

    #[test]
    fn test_principal_info_serialization() {
        let principal_info = PrincipalInfo::new(PrincipalKind::Role, "MyRole");

        let json = serde_json::to_string(&principal_info).unwrap();

        // Verify PascalCase field names
        assert!(json.contains("\"Kind\""));
        assert!(json.contains("\"Name\""));
    }
}
