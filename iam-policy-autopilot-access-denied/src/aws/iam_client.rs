//! AWS IAM client wrapper for policy operations
//!
// TODO: Consider consolidating `put_role_policy` and `put_user_policy` into a generic
// implementation when adding support for additional principal types.

use crate::aws::principal::PrincipalKind;
use crate::aws::{AwsError, AwsResult};
use crate::types::{PolicyDocument, PolicyMetadata};
use aws_sdk_iam::Client as IamClient;

pub struct AwsIamClient {
    client: IamClient,
}

impl AwsIamClient {
    pub fn new(client: IamClient) -> Self {
        Self { client }
    }

    pub async fn put_role_policy(
        &self,
        role_name: &str,
        policy_name: &str,
        policy_document: &PolicyDocument,
    ) -> AwsResult<()> {
        let policy_json = serde_json::to_string(policy_document)
            .map_err(|e| AwsError::PolicyError(format!("Failed to serialize policy: {e}")))?;

        self.client
            .put_role_policy()
            .role_name(role_name)
            .policy_name(policy_name)
            .policy_document(policy_json)
            .send()
            .await
            .map_err(|e| {
                AwsError::IamError(format!(
                    "Failed to put role policy '{policy_name}' on role '{role_name}': {e:?}"
                ))
            })?;
        Ok(())
    }

    pub async fn put_user_policy(
        &self,
        user_name: &str,
        policy_name: &str,
        policy_document: &PolicyDocument,
    ) -> AwsResult<()> {
        let policy_json = serde_json::to_string(policy_document)
            .map_err(|e| AwsError::PolicyError(format!("Failed to serialize policy: {e}")))?;
        self.client
            .put_user_policy()
            .user_name(user_name)
            .policy_name(policy_name)
            .policy_document(policy_json)
            .send()
            .await
            .map_err(|e| AwsError::IamError(format!("Failed to put user policy: {e}")))?;
        Ok(())
    }
}

/// Put an inline policy on a principal (role or user)
pub(crate) async fn put_inline_policy(
    client: &IamClient,
    kind: &PrincipalKind,
    principal_name: &str,
    policy_name: &str,
    policy_doc: &PolicyDocument,
) -> AwsResult<()> {
    let iam_client = AwsIamClient::new(client.clone());
    match kind {
        PrincipalKind::Role => {
            iam_client
                .put_role_policy(principal_name, policy_name, policy_doc)
                .await
        }
        PrincipalKind::User => {
            iam_client
                .put_user_policy(principal_name, policy_name, policy_doc)
                .await
        }
    }
}

/// List all inline policy names for a principal
pub(crate) async fn list_inline_policies(
    client: &IamClient,
    kind: &PrincipalKind,
    principal_name: &str,
) -> AwsResult<Vec<String>> {
    match kind {
        PrincipalKind::Role => {
            let response = client
                .list_role_policies()
                .role_name(principal_name)
                .send()
                .await
                .map_err(|e| AwsError::IamError(format!("Failed to list role policies: {e}")))?;
            Ok(response.policy_names)
        }
        PrincipalKind::User => {
            let response = client
                .list_user_policies()
                .user_name(principal_name)
                .send()
                .await
                .map_err(|e| AwsError::IamError(format!("Failed to list user policies: {e}")))?;
            Ok(response.policy_names)
        }
    }
}

/// Fetch and parse a specific inline policy document
pub(crate) async fn get_inline_policy(
    client: &IamClient,
    kind: &PrincipalKind,
    principal_name: &str,
    policy_name: &str,
) -> AwsResult<PolicyDocument> {
    let policy_json = match kind {
        PrincipalKind::Role => {
            let response = client
                .get_role_policy()
                .role_name(principal_name)
                .policy_name(policy_name)
                .send()
                .await
                .map_err(|e| AwsError::IamError(format!("Failed to get role policy: {e}")))?;
            response.policy_document
        }
        PrincipalKind::User => {
            let response = client
                .get_user_policy()
                .user_name(principal_name)
                .policy_name(policy_name)
                .send()
                .await
                .map_err(|e| AwsError::IamError(format!("Failed to get user policy: {e}")))?;
            response.policy_document
        }
    };

    // URL decode the policy document (AWS returns URL-encoded JSON)
    let decoded = percent_encoding::percent_decode_str(&policy_json)
        .decode_utf8()
        .map_err(|e| AwsError::PolicyError(format!("Failed to URL decode policy document: {e}")))?;

    // Parse JSON
    serde_json::from_str(&decoded)
        .map_err(|e| AwsError::PolicyError(format!("Failed to parse policy document JSON: {e}")))
}

/// Find existing canonical IAM Policy Autopilot policy for a principal
/// Returns policy name and document if found
pub async fn find_canonical_policy(
    client: &IamClient,
    kind: &PrincipalKind,
    principal_name: &str,
) -> AwsResult<Option<PolicyMetadata>> {
    use crate::aws::policy_naming::build_canonical_policy_name;

    let canonical_name = build_canonical_policy_name(kind, principal_name);
    let policy_names = list_inline_policies(client, kind, principal_name).await?;

    if policy_names.contains(&canonical_name) {
        let document = get_inline_policy(client, kind, principal_name, &canonical_name).await?;
        Ok(Some(PolicyMetadata {
            name: canonical_name,
            document,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        aws::policy_naming::POLICY_PREFIX,
        types::{ActionType, Statement},
    };

    fn sample_policy() -> PolicyDocument {
        PolicyDocument {
            id: Some(POLICY_PREFIX.to_string()),
            version: "2012-10-17".to_string(),
            statement: vec![Statement {
                sid: "Test".into(),
                effect: "Allow".into(),
                action: ActionType::Single("s3:GetObject".into()),
                resource: "arn:aws:s3:::bucket/*".into(),
            }],
        }
    }

    #[test]
    fn test_policy_json() {
        let json = serde_json::to_string(&sample_policy()).unwrap();
        assert!(json.contains("2012-10-17"));
        assert!(json.contains("s3:GetObject"));
    }
}
