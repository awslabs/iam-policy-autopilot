//! AWS credential verification and STS/IAM helper functions.

use std::{collections::HashMap, time::Duration};

use anyhow::{anyhow, Context, Result};
use aws_sdk_sts::Client as StsClient;
use tokio::time::sleep;
use tracing::debug;

// ---------------------------------------------------------------------------
// Credential verification
// ---------------------------------------------------------------------------

/// Build an STS client from explicit credentials (e.g. from an AssumeRole
/// response) and call `sts:GetCallerIdentity` to verify they are usable.
///
/// Returns `true` if the credentials are accepted by STS, `false` otherwise.
///
/// **Limitation:** `sts:GetCallerIdentity` does NOT require IAM authorization
/// and always succeeds as long as STS itself recognises the credentials.
/// This means it cannot detect `InvalidAccessKeyId` / `InvalidSecurityToken`
/// errors that occur when other services (S3, Glue, DynamoDB, …) have not
/// yet received the credential propagation.  The caller should also check
/// the script's stderr for transient credential error patterns using
/// [`stderr_indicates_transient_credential_error`].
pub(crate) async fn verify_credentials(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
    region: &str,
) -> bool {
    let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .credentials_provider(aws_sdk_sts::config::Credentials::new(
            access_key_id,
            secret_access_key,
            Some(session_token.to_string()),
            None,
            "runner-verify",
        ))
        .load()
        .await;
    let probe_sts = StsClient::new(&conf);
    probe_sts.get_caller_identity().send().await.is_ok()
}

/// Check whether the script's stderr contains error patterns that indicate a
/// transient AWS credential propagation failure (as opposed to a genuine IAM
/// permission denial or application error).
///
/// These errors occur when STS-issued temporary credentials have not yet
/// propagated to other AWS service endpoints (S3, Glue, DynamoDB, etc.).
/// The [`verify_credentials`] probe (which uses `sts:GetCallerIdentity`)
/// cannot detect these because `sts:GetCallerIdentity` does not require IAM
/// authorization and validates against STS itself — it will succeed even when
/// the credentials are not yet recognised by other services.
#[must_use]
pub(crate) fn stderr_indicates_transient_credential_error(stderr: &str) -> bool {
    const TRANSIENT_PATTERNS: &[&str] = &[
        "InvalidAccessKeyId",
        "The AWS Access Key Id you provided does not exist in our records",
        "InvalidSecurityToken",
        "The security token included in the request is invalid",
        "ExpiredToken",
        "The security token included in the request is expired",
        "RequestExpired",
        "Request has expired",
        "InvalidClientTokenId",
        "UnrecognizedClientException",
    ];

    let stderr_lower = stderr.to_lowercase();
    TRANSIENT_PATTERNS
        .iter()
        .any(|pat| stderr_lower.contains(&pat.to_lowercase()))
}

// ---------------------------------------------------------------------------
// AWS helpers
// ---------------------------------------------------------------------------

/// Return the AWS account ID for the current credentials.
pub async fn get_aws_account_id(sts: &StsClient) -> Result<String> {
    let resp = sts
        .get_caller_identity()
        .send()
        .await
        .context("sts:GetCallerIdentity failed")?;
    resp.account
        .ok_or_else(|| anyhow!("GetCallerIdentity returned no account"))
}

/// Return the caller's IAM ARN (role or user ARN) for the current credentials.
///
/// This is used to scope trust policies on temporary roles so that only the
/// runner's own identity can assume them — not any principal in the account.
pub async fn get_caller_arn(sts: &StsClient) -> Result<String> {
    let resp = sts
        .get_caller_identity()
        .send()
        .await
        .context("sts:GetCallerIdentity failed")?;
    let arn = resp
        .arn
        .ok_or_else(|| anyhow!("GetCallerIdentity returned no ARN"))?;
    Ok(normalize_caller_arn(&arn))
}

/// Normalize an assumed-role session ARN to the underlying IAM role ARN.
///
/// If the ARN is an assumed-role session
/// (`arn:aws:sts::ACCT:assumed-role/ROLE/SESSION`), it is converted to the
/// underlying IAM role ARN (`arn:aws:iam::ACCT:role/ROLE`) so the trust policy
/// remains valid even if the session name changes between calls.
///
/// Non-assumed-role ARNs are returned unchanged.
#[must_use]
fn normalize_caller_arn(arn: &str) -> String {
    if arn.contains(":assumed-role/") {
        // Format: arn:aws:sts::123456789012:assumed-role/MyRole/session-name
        let parts: Vec<&str> = arn.splitn(2, ":assumed-role/").collect();
        if parts.len() == 2 {
            let prefix = parts[0].replace(":sts:", ":iam:");
            let role_and_session = parts[1];
            let role_name = role_and_session
                .split('/')
                .next()
                .unwrap_or(role_and_session);
            return format!("{prefix}:role/{role_name}");
        }
    }
    arn.to_string()
}

/// Assume *role_arn* and return a map of env-var overrides for child processes.
/// Returns `None` if the caller is already running as that role.
pub(crate) async fn assume_role_env(
    sts: &StsClient,
    role_arn: &str,
    session_name: &str,
) -> Result<Option<HashMap<String, String>>> {
    const MAX_ATTEMPTS: u32 = 8;

    // Check whether we are already this role to avoid self-assume errors.
    if let Ok(identity) = sts.get_caller_identity().send().await {
        let caller_arn = identity.arn.unwrap_or_default();
        let target_role_name = role_arn.split('/').next_back().unwrap_or("");
        if caller_arn.contains(&format!("assumed-role/{target_role_name}/"))
            || caller_arn == role_arn
        {
            debug!(
                "Already running as {} -- skipping role assumption",
                target_role_name
            );
            return Ok(None);
        }
    }

    // Retry sts:AssumeRole with exponential back-off.  Newly created IAM roles
    // are not immediately assumable — IAM propagation typically takes 5–15 s.
    // Retrying here lets us reduce (or eliminate) the fixed upfront sleep while
    // still being robust against propagation delays.
    let mut last_err = anyhow!("sts:AssumeRole: no attempts made");
    let target_role_name = role_arn.split('/').next_back().unwrap_or("<unknown>");
    for attempt in 0..MAX_ATTEMPTS {
        match sts
            .assume_role()
            .role_arn(role_arn)
            .role_session_name(session_name)
            .send()
            .await
        {
            Ok(resp) => {
                let creds = resp
                    .credentials
                    .ok_or_else(|| anyhow!("AssumeRole returned no credentials"))?;
                let mut env_vars = HashMap::new();
                env_vars.insert("AWS_ACCESS_KEY_ID".into(), creds.access_key_id);
                env_vars.insert("AWS_SECRET_ACCESS_KEY".into(), creds.secret_access_key);
                env_vars.insert("AWS_SESSION_TOKEN".into(), creds.session_token);
                return Ok(Some(env_vars));
            }
            Err(e) => {
                last_err = anyhow!("sts:AssumeRole failed for role '{target_role_name}': {e}");
                if attempt + 1 < MAX_ATTEMPTS {
                    let wait = 2u64.pow(attempt); // 1, 2, 4, 8, 16, 32, 64 s
                    debug!(
                        "sts:AssumeRole attempt {}/{} failed for role '{}' ({}), retrying in {}s ...",
                        attempt + 1,
                        MAX_ATTEMPTS,
                        target_role_name,
                        e,
                        wait
                    );
                    sleep(Duration::from_secs(wait)).await;
                } else {
                    debug!(
                        "sts:AssumeRole attempt {}/{} failed for role '{}' ({}) — no retries remaining",
                        attempt + 1,
                        MAX_ATTEMPTS,
                        target_role_name,
                        e,
                    );
                }
            }
        }
    }
    Err(last_err)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ── stderr_indicates_transient_credential_error ──────────────────────────

    #[rstest]
    #[case::invalid_access_key_id(
        "An error occurred (InvalidAccessKeyId) when calling the PutObject operation",
        true
    )]
    #[case::invalid_security_token("The security token included in the request is invalid", true)]
    #[case::expired_token(
        "ExpiredToken: The security token included in the request is expired",
        true
    )]
    #[case::request_expired("RequestExpired: Request has expired", true)]
    #[case::invalid_client_token_id(
        "InvalidClientTokenId: The security token included in the request is invalid",
        true
    )]
    #[case::unrecognized_client_exception(
        "UnrecognizedClientException: The security token is invalid",
        true
    )]
    #[case::case_insensitive("invalidaccesskeyid: something went wrong", true)]
    #[case::access_denied(
        "An error occurred (AccessDenied) when calling the PutObject operation: Access Denied",
        false
    )]
    #[case::empty_stderr("", false)]
    #[case::generic_error("Error: connection refused", false)]
    #[case::permission_error(
        "User: arn:aws:iam::123456789012:user/test is not authorized to perform: s3:PutObject",
        false
    )]
    fn transient_credential_error_detection(#[case] stderr: &str, #[case] expected: bool) {
        assert_eq!(
            stderr_indicates_transient_credential_error(stderr),
            expected,
            "stderr={stderr:?}"
        );
    }

    // ── normalize_caller_arn ─────────────────────────────────────────────────

    #[rstest]
    #[case::assumed_role(
        "arn:aws:sts::123456789012:assumed-role/MyRole/session-name",
        "arn:aws:iam::123456789012:role/MyRole"
    )]
    #[case::assumed_role_complex_session(
        "arn:aws:sts::123456789012:assumed-role/MyRole/i-0abc123def456",
        "arn:aws:iam::123456789012:role/MyRole"
    )]
    #[case::assumed_role_botocore_session(
        "arn:aws:sts::123456789012:assumed-role/admin/botocore-session-1234",
        "arn:aws:iam::123456789012:role/admin"
    )]
    #[case::iam_user_unchanged(
        "arn:aws:iam::123456789012:user/testuser",
        "arn:aws:iam::123456789012:user/testuser"
    )]
    #[case::iam_role_unchanged(
        "arn:aws:iam::123456789012:role/MyRole",
        "arn:aws:iam::123456789012:role/MyRole"
    )]
    #[case::root_unchanged("arn:aws:iam::123456789012:root", "arn:aws:iam::123456789012:root")]
    #[case::govcloud_assumed_role(
        "arn:aws-us-gov:sts::123456789012:assumed-role/GovRole/session",
        "arn:aws-us-gov:iam::123456789012:role/GovRole"
    )]
    #[case::china_assumed_role(
        "arn:aws-cn:sts::123456789012:assumed-role/ChinaRole/session",
        "arn:aws-cn:iam::123456789012:role/ChinaRole"
    )]
    fn arn_normalization(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(normalize_caller_arn(input), expected);
    }
}
