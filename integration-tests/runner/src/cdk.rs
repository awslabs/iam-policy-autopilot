//! CDK lifecycle helpers — deploy and destroy CDK stacks.

use std::{collections::HashMap, fs, path::Path};

use anyhow::Result;
use aws_sdk_sts::Client as StsClient;
use tracing::{debug, error, info, warn};

use crate::aws::{assume_role_env, get_aws_account_id};
use crate::helpers::{npm_install_if_needed, run_command};

// ── CDK lifecycle helpers ─────────────────────────────────────────────────────

/// Deploy the CDK stack in `run_dir/cdk/` by running `deploy.sh`.
///
/// Runs `npm install` and `cdk bootstrap` if needed, then `bash deploy.sh`.
/// If `deploy_role_arn` is provided, the role is assumed and its temporary
/// credentials are injected into the CDK subprocess environment.
/// Returns `Ok(true)` on success, `Ok(false)` on any failure.
pub async fn cdk_deploy(
    run_dir: &Path,
    deploy_role_arn: Option<&str>,
    region: &str,
    sts: &StsClient,
) -> Result<bool> {
    let cdk_dir = run_dir.join("cdk");
    let deploy_sh = cdk_dir.join("deploy.sh");
    if !deploy_sh.exists() {
        error!("deploy.sh not found at {:?}", deploy_sh);
        return Ok(false);
    }

    let mut extra_env: HashMap<String, String> = HashMap::new();
    extra_env.insert("AWS_DEFAULT_REGION".into(), region.into());

    if let Some(role_arn) = deploy_role_arn {
        debug!("Assuming deploy role for CDK deploy");
        match assume_role_env(sts, role_arn, "cdk-deploy-session").await {
            Ok(Some(creds)) => extra_env.extend(creds),
            Ok(None) => {}
            Err(e) => {
                error!("Failed to assume deploy role: {}", e);
                return Ok(false);
            }
        }
    }

    // Remove stale cdk.out to avoid lock issues from interrupted previous runs.
    let cdk_out = cdk_dir.join("cdk.out");
    if cdk_out.exists() {
        let _ = fs::remove_dir_all(&cdk_out);
    }

    if !npm_install_if_needed(&cdk_dir, &extra_env) {
        error!("npm install failed in cdk dir {:?}", cdk_dir);
        return Ok(false);
    }

    let account = match get_aws_account_id(sts).await {
        Ok(a) => a,
        Err(e) => {
            error!("Could not get account ID for bootstrap: {}", e);
            return Ok(false);
        }
    };
    let bootstrap_target = format!("aws://{account}/{region}");

    let bootstrap_ok = run_command(
        &["npx", "cdk", "bootstrap", &bootstrap_target],
        &cdk_dir,
        &extra_env,
    );
    if !bootstrap_ok {
        error!("CDK bootstrap failed");
        return Ok(false);
    }
    info!("[CDK] Bootstrap complete");

    info!("[CDK] Deploying in {:?} ...", cdk_dir);
    let deploy_ok = run_command(&["bash", "deploy.sh"], &cdk_dir, &extra_env);
    if !deploy_ok {
        error!("CDK deploy failed");
        return Ok(false);
    }

    let config_file = run_dir.join("config.json");
    if !config_file.exists() {
        error!(
            "config.json not written by deploy.sh (expected at {:?})",
            config_file
        );
        return Ok(false);
    }

    info!(
        "[CDK] Deploy succeeded -- config.json written to {:?}",
        config_file
    );
    Ok(true)
}

/// Destroy the CDK stack in `run_dir/cdk/` by running `cdk destroy --force --all`.
///
/// If `deploy_role_arn` is provided, the role is assumed and its temporary
/// credentials are injected into the CDK subprocess environment.
/// Returns `Ok(true)` on success, `Ok(false)` on any failure.
pub async fn cdk_destroy(
    run_dir: &Path,
    deploy_role_arn: Option<&str>,
    region: &str,
    sts: &StsClient,
) -> Result<bool> {
    let cdk_dir = run_dir.join("cdk");
    if !cdk_dir.exists() {
        warn!("CDK directory not found at {:?}, skipping destroy", cdk_dir);
        return Ok(false);
    }

    let mut extra_env: HashMap<String, String> = HashMap::new();
    extra_env.insert("AWS_DEFAULT_REGION".into(), region.into());

    if let Some(role_arn) = deploy_role_arn {
        debug!("Assuming deploy role for CDK destroy");
        match assume_role_env(sts, role_arn, "cdk-destroy-session").await {
            Ok(Some(creds)) => extra_env.extend(creds),
            Ok(None) => {}
            Err(e) => {
                error!("Failed to assume deploy role for destroy: {}", e);
                return Ok(false);
            }
        }
    }

    info!("[CDK] Destroying stack in {:?} ...", cdk_dir);
    let ok = run_command(
        &["npx", "cdk", "destroy", "--force", "--all"],
        &cdk_dir,
        &extra_env,
    );
    if ok {
        info!("[CDK] Destroy succeeded");
    } else {
        warn!("CDK destroy exited with non-zero code");
    }
    Ok(ok)
}
