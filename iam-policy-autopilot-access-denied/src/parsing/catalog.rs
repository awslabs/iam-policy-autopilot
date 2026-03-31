//! String-based AccessDenied message parser using split heuristics.

use super::utils::{
    extract_action, extract_context, extract_principal, extract_resource, is_explicit_deny,
};
use crate::types::{DenialType, ParsedDenial};

fn infer_denial_type(message: &str) -> DenialType {
    if message.is_empty() {
        return DenialType::Other;
    }
    let msg_lower = message.to_lowercase();
    let context = extract_context(message).to_lowercase();
    if is_explicit_deny(message) {
        if msg_lower.contains("identity-based policy") || context.contains("identity-based policy")
        {
            return DenialType::ExplicitIdentity;
        }
        return DenialType::ExplicitIdentity;
    }
    if msg_lower.contains("resource-based policy") || context.contains("resource-based policy") {
        return DenialType::ResourcePolicy;
    }
    if msg_lower.contains("no identity-based policy allows")
        || context.contains("no identity-based policy allows")
        || msg_lower.contains("because no identity-based policy")
    {
        return DenialType::ImplicitIdentity;
    }
    DenialType::Other
}

/// Parse AccessDenied message.
/// Returns None if message is empty or cannot be parsed.
#[must_use]
pub fn parse(message: &str) -> Option<ParsedDenial> {
    if message.is_empty() {
        return None;
    }
    let principal_arn = extract_principal(message)?;
    let action = extract_action(message)?;
    if message.to_lowercase().contains("on resource:") {
        let resource = extract_resource(message)?;
        let denial_type = infer_denial_type(message);
        Some(ParsedDenial::new(
            principal_arn,
            action,
            resource,
            denial_type,
        ))
    } else {
        let resource = extract_resource(message).unwrap_or_else(|| {
            if let Some(pos) = action.find(':') {
                let service_name = &action[..pos];
                match service_name.to_lowercase().as_str() {
                    "s3" if action.to_lowercase().contains("list") => "*".to_string(),
                    "ec2" if action.to_lowercase().contains("describe") => "*".to_string(),
                    _ => "*".to_string(),
                }
            } else {
                "*".to_string()
            }
        });
        let denial_type = infer_denial_type(message);
        Some(ParsedDenial::new(
            principal_arn,
            action,
            resource,
            denial_type,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_s3_message() {
        let message = "User: arn:aws:iam::123456789012:user/testuser is not authorized to perform: s3:GetObject on resource: arn:aws:s3:::my-bucket/my-key";
        let result = parse(message).unwrap();
        assert_eq!(
            result.principal_arn,
            "arn:aws:iam::123456789012:user/testuser"
        );
        assert_eq!(result.action, "s3:GetObject");
        assert_eq!(result.resource, "arn:aws:s3:::my-bucket/my-key");
    }
}
