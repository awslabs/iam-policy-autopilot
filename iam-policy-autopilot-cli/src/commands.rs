//! Unified command implementation for fixing AccessDenied errors.
//! This module provides a single fix-access-denied command that handles
//! all denial types with appropriate branching logic.

use crate::{output, types::ExitCode};
use iam_policy_autopilot_access_denied::{ApplyError, ApplyOptions, DenialType};

fn is_tty() -> bool {
    atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stderr)
}

/// Returns Some(true) if user confirmed, Some(false) if declined, None if not in TTY.
fn prompt_yes_no() -> Option<bool> {
    if !is_tty() {
        return None;
    }
    output::prompt_apply_once();
    let mut s = String::new();
    use std::io::{self, BufRead};
    let stdin = io::stdin();
    let _ = stdin.lock().read_line(&mut s);
    let line = s.lines().next().unwrap_or("").trim().to_ascii_lowercase();
    if line == "y" || line == "yes" {
        Some(true)
    } else {
        Some(false)
    }
}

pub async fn fix_access_denied(error_text: &str, yes: bool) -> ExitCode {
    let service = match iam_policy_autopilot_access_denied::IamPolicyAutopilotService::new().await {
        Ok(s) => s,
        Err(e) => {
            output::note(&format!("Failed to initialize service: {}", e));
            return ExitCode::Error;
        }
    };

    match service.plan(error_text).await {
        Ok(plan) => fix_access_denied_with_service(plan, yes, service).await,
        Err(e) => {
            if matches!(e, iam_policy_autopilot_access_denied::IamPolicyAutopilotError::Parsing(_)) {
                output::note("No AccessDenied found in provided text");
            } else {
                output::note(&format!("Failed to create plan: {}", e));
            }
            ExitCode::Error
        }
    }
}

async fn fix_access_denied_with_service(
    plan: iam_policy_autopilot_access_denied::PlanResult,
    yes: bool,
    service: iam_policy_autopilot_access_denied::IamPolicyAutopilotService,
) -> ExitCode {
    match plan.diagnosis.denial_type {
        DenialType::ImplicitIdentity => {
            output::print_plan(&plan);

            if !is_tty() && !yes {
                output::print_apply_refused(
                    "refused_non_tty",
                    "run interactively in a TTY to apply changes, or use --yes flag",
                );
                return ExitCode::Success;
            }

            if !yes {
                match prompt_yes_no() {
                    Some(true) => {}
                    Some(false) => {
                        output::print_apply_refused("aborted_by_user", "apply aborted by user");
                        return ExitCode::Success;
                    }
                    None => {
                        output::print_apply_refused(
                            "refused_non_tty",
                            "run interactively in a TTY to apply changes",
                        );
                        return ExitCode::Success;
                    }
                }
            }

            let options = ApplyOptions::default();
            let result = service.apply(&plan, options).await;

            match result {
                Ok(apply_result) => {
                    if apply_result.is_new_policy {
                        output::print_apply_success(
                            &apply_result.policy_name,
                            &apply_result.principal_kind,
                            &apply_result.principal_name,
                        );
                    } else {
                        output::print_statement_added(
                            &apply_result.policy_name,
                            &apply_result.principal_kind,
                            &apply_result.principal_name,
                            apply_result.statement_count,
                        );
                    }
                    ExitCode::Success
                }
                Err(apply_error) => handle_apply_error(apply_error),
            }
        }
        DenialType::ResourcePolicy => {
            let action = plan.diagnosis.action.clone();
            let resource = plan.diagnosis.resource.clone();

            use iam_policy_autopilot_access_denied::build_single_statement;
            let statement =
                build_single_statement(action.clone(), resource.clone(), "AllowAccess".to_string());

            let statement_json = match serde_json::to_string_pretty(&statement) {
                Ok(json) => json,
                Err(e) => {
                    output::warn(&format!("Failed to serialize statement: {}", e));
                    return ExitCode::Error;
                }
            };

            output::print_resource_policy_fix(&action, &resource, &statement_json);
            ExitCode::Error
        }
        DenialType::ExplicitIdentity => {
            output::print_explicit_deny_explanation();
            ExitCode::Error
        }
        DenialType::Other => {
            output::print_unsupported_denial(
                &plan.diagnosis.denial_type,
                "This denial type cannot be automatically analyzed or fixed",
            );
            ExitCode::Error
        }
    }
}

fn handle_apply_error(apply_error: ApplyError) -> ExitCode {
    match apply_error {
        ApplyError::UnsupportedDenialType => {
            output::print_apply_refused(
                "explain_only",
                "this denial type is not fixable with an inline identity policy",
            );
            ExitCode::Error
        }
        ApplyError::UnsupportedPrincipal(msg) => {
            output::print_apply_refused("unsupported_principal", &msg);
            ExitCode::Error
        }
        ApplyError::AccountMismatch {
            principal_account,
            caller_account,
        } => {
            output::print_apply_refused(
                "account_mismatch",
                &format!(
                    "principal account ({}) differs from caller account ({})",
                    principal_account, caller_account
                ),
            );
            ExitCode::Error
        }
        ApplyError::DuplicateStatement { action, resource } => {
            output::print_duplicate_statement(&action, &resource);
            ExitCode::Duplicate
        }
        ApplyError::MultiActionError(count) => {
            output::print_apply_refused(
                "multi_action_error",
                &format!(
                    "Expected exactly 1 action for canonical policy, got {}. This is a bug.",
                    count
                ),
            );
            ExitCode::Error
        }
        ApplyError::Aws(e) => {
            let msg = e.to_string();
            if msg.contains("NoSuchEntity") {
                output::print_apply_refused(
                    "principal_not_found",
                    "the IAM principal no longer exists or is not visible",
                );
            } else {
                output::print_apply_refused("mutation_error", &msg);
            }
            ExitCode::Error
        }
    }
}
