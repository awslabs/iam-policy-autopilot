//! LLM-based IAM policy generation via AWS Bedrock and policy validation via
//! AWS IAM Access Analyzer.
//!
//! For each language, this module:
//!   1. Reads the source file for that language.
//!   2. Calls the configured Bedrock model (default:
//!      `global.anthropic.claude-opus-4-6-v1`) with the prompt
//!      "Generate an identity-based AWS IAM Policy which allows me to execute
//!       this program: <source code>".
//!   3. Extracts the JSON policy document from the model response.
//!   4. Runs `iam:ValidatePolicy` (Access Analyzer) on the extracted document.
//!   5. Returns a [`LlmPolicyOutcome`] with the policy, validation findings,
//!      and concrete-action count.

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_accessanalyzer::{
    Client as AaClient,
    error::SdkError as AaSdkError,
    types::{PolicyType, ValidatePolicyFinding},
};
use aws_sdk_bedrockruntime::{
    Client as BedrockClient,
    error::SdkError as BedrockSdkError,
    types::{ContentBlock, ConversationRole, Message},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};

use crate::managed_policies::extract_allow_statements;
use crate::service_ref::{action_covered_by, ServiceCatalogue};

// ---------------------------------------------------------------------------
// Resource prompt strategy enum
// ---------------------------------------------------------------------------

/// Selects which resource-handling instruction is appended to the LLM prompt.
///
/// Used in the 3×3 benchmark matrix: each context scenario (script-only,
/// script+context, script+CDK+context) is crossed with each prompt strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourcePromptStrategy {
    /// No resource instruction — just "generate a policy".
    Bare,
    /// "Fill in all placeholder variables; if you don't know what to put, use the wildcard *."
    Wildcards,
    /// "Use \"Resource\": \"*\"."
    ResourceStar,
}

impl ResourcePromptStrategy {
    /// Parse a strategy from a CLI string (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "bare" => Some(Self::Bare),
            "wildcards" => Some(Self::Wildcards),
            "resource-star" | "resource_star" | "resourcestar" => Some(Self::ResourceStar),
            _ => None,
        }
    }

    /// Short slug used in composite tags (e.g. `"LLM/bare"`).
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Bare => "bare",
            Self::Wildcards => "wildcards",
            Self::ResourceStar => "resource-star",
        }
    }
}

impl fmt::Display for ResourcePromptStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// Token usage reported by the Bedrock Converse API for a single LLM call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BedrockTokenUsage {
    /// Number of tokens in the input (prompt).
    pub input_tokens: u32,
    /// Number of tokens in the output (completion).
    pub output_tokens: u32,
    /// Total tokens (input + output).
    pub total_tokens: u32,
}

/// A single finding returned by IAM Access Analyzer `ValidatePolicy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessAnalyzerFinding {
    pub finding_type: String,
    pub issue_code: String,
    pub details: String,
    pub learn_more_link: Option<String>,
}

/// The outcome of LLM policy generation for one language in one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmPolicyOutcome {
    /// Programming language (python, go, java, typescript).
    pub language: String,
    /// Whether the LLM returned a parseable IAM policy document.
    pub policy_generated: bool,
    /// The raw text response from the LLM (for debugging).
    pub llm_raw_response: Option<String>,
    /// The extracted IAM policy document (if parseable).
    pub policy_document: Option<Value>,
    /// Concrete IAM actions allowed by the LLM-generated policy.
    pub concrete_actions: u32,
    /// Concrete actions in the minimal policy (baseline for ratio).
    pub minimal_concrete_actions: u32,
    /// Concrete actions in the managed-policy set (baseline for ratio).
    pub managed_policy_concrete_actions: u32,
    /// concrete_actions / minimal_concrete_actions.
    pub over_permission_ratio_vs_minimal: f64,
    /// concrete_actions / managed_policy_concrete_actions.
    pub over_permission_ratio_vs_managed: f64,
    /// Whether the LLM-generated policy allowed the script to run successfully.
    pub validation_success: bool,
    /// Findings from IAM Access Analyzer `ValidatePolicy`.
    pub access_analyzer_findings: Vec<AccessAnalyzerFinding>,
    /// Number of errors reported by Access Analyzer.
    pub access_analyzer_error_count: usize,
    /// Number of warnings reported by Access Analyzer.
    pub access_analyzer_warning_count: usize,
    /// Number of suggestions reported by Access Analyzer.
    pub access_analyzer_suggestion_count: usize,
    /// Token usage reported by the Bedrock Converse API.
    pub token_usage: BedrockTokenUsage,
}

// ---------------------------------------------------------------------------
// Bedrock call
// ---------------------------------------------------------------------------

/// Construct the default Bedrock inference profile ARN for LLM policy generation
/// using the resolved AWS account ID and region.
pub fn default_bedrock_model_id(region: &str, account: &str) -> String {
    format!(
        "arn:aws:bedrock:{}:{}:inference-profile/global.anthropic.claude-opus-4-6-v1",
        region, account
    )
}

/// Build the LLM prompt for IAM policy generation.
///
/// Factored out of [`call_bedrock_for_policy`] so the prompt logic is in one
/// place regardless of whether CDK context or resource prompt strategy is active.
fn build_prompt(source_code: &str, cdk_stack_code: Option<&str>, strategy: ResourcePromptStrategy) -> String {
    let base = "Generate an identity-based AWS IAM Policy which allows me to execute this application.";

    let resource_instruction = match strategy {
        ResourcePromptStrategy::Bare => "",
        ResourcePromptStrategy::Wildcards =>
            " Fill in all placeholder variables; if you don't know what to put, use the wildcard *.",
        ResourcePromptStrategy::ResourceStar =>
            " Use \"Resource\": \"*\".",
    };

    if let Some(stack_code) = cdk_stack_code {
        format!(
            "{}{} \
             The application interacts with AWS infrastructure defined by the following CDK template — \
             use it to understand resource configurations (e.g. encryption, access patterns) \
             that may require additional permissions:\n\n\
             CDK Template:\n{}\n\n\
             Application:\n{}",
            base, resource_instruction, stack_code, source_code
        )
    } else {
        format!(
            "{}{} \
             Application:\n{}",
            base, resource_instruction, source_code
        )
    }
}

/// Call Bedrock with *model_id* and the given source code, returning the raw
/// text response together with token usage metrics.  Logs the full AWS service
/// error (including the encapsulated `ServiceError` message) on failure.
///
/// If `prior_messages` is non-empty, those messages are prepended to the
/// conversation before the final user prompt (context-filling experiment).
///
/// If `cdk_stack_code` is `Some`, the CDK stack definition is included in the
/// prompt so the LLM can reason about infrastructure details (e.g. KMS-encrypted
/// S3 buckets requiring `kms:Decrypt` / `kms:GenerateDataKey` permissions).
///
/// The `strategy` parameter selects which resource-handling instruction is
/// appended to the prompt (bare, wildcards, or `"Resource": "*"`).
///
/// Transient errors (HTTP 503 / 429, timeouts, dispatch failures) are retried
/// with exponential backoff (up to [`MAX_BEDROCK_ATTEMPTS`] attempts).
pub async fn call_bedrock_for_policy(
    bedrock: &BedrockClient,
    source_code: &str,
    language: &str,
    model_id: &str,
    prior_messages: Vec<Message>,
    cdk_stack_code: Option<&str>,
    strategy: ResourcePromptStrategy,
) -> Result<(String, BedrockTokenUsage)> {
    const MAX_BEDROCK_ATTEMPTS: u32 = 5;
    const INITIAL_BACKOFF_SECS: u64 = 2;
    const MAX_BACKOFF_SECS: u64 = 60;
    /// Per-request timeout for a single Bedrock Converse call.
    /// Normal calls complete in 10-30 s; 5 minutes is generous enough to
    /// accommodate slow responses while preventing indefinite hangs.
    const BEDROCK_CALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

    let prompt = build_prompt(source_code, cdk_stack_code, strategy);

    info!(
        "[LLM][{}] Calling Bedrock model {} ({} chars, {} prior messages) ...",
        language,
        model_id,
        source_code.len(),
        prior_messages.len(),
    );

    let user_message = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(prompt))
        .build()
        .context("Failed to build Bedrock message")?;

    // Build the full message list: prior context messages + final user prompt.
    let mut all_messages = prior_messages;
    all_messages.push(user_message);

    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 1..=MAX_BEDROCK_ATTEMPTS {
        // Clone messages for each attempt (the builder consumes them).
        let mut req = bedrock.converse().model_id(model_id);
        for msg in &all_messages {
            req = req.messages(msg.clone());
        }

        // Wrap the SDK call with a client-side timeout so a hanging HTTP
        // connection cannot block the entire benchmark run indefinitely.
        let send_result = tokio::time::timeout(BEDROCK_CALL_TIMEOUT, req.send()).await;

        match send_result {
            Err(_elapsed) => {
                // tokio::time::timeout expired — the Bedrock call hung.
                warn!(
                    "[LLM][{}] Bedrock call timed out after {}s (attempt {}/{})",
                    language,
                    BEDROCK_CALL_TIMEOUT.as_secs(),
                    attempt,
                    MAX_BEDROCK_ATTEMPTS,
                );

                if attempt < MAX_BEDROCK_ATTEMPTS {
                    let backoff = std::cmp::min(
                        INITIAL_BACKOFF_SECS * 2u64.pow(attempt - 1),
                        MAX_BACKOFF_SECS,
                    );
                    warn!(
                        "[LLM][{}] Retrying in {}s ...",
                        language, backoff,
                    );
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    last_err = Some(anyhow::anyhow!(
                        "Bedrock converse call timed out after {}s",
                        BEDROCK_CALL_TIMEOUT.as_secs(),
                    ));
                    continue;
                }

                return Err(anyhow::anyhow!(
                    "Bedrock converse call timed out after {}s (all {} attempts exhausted)",
                    BEDROCK_CALL_TIMEOUT.as_secs(),
                    MAX_BEDROCK_ATTEMPTS,
                ));
            }
            Ok(Ok(response)) => {
                // Extract token usage from the response.
                let token_usage = response.usage.as_ref().map(|u| {
                    let input = u.input_tokens as u32;
                    let output = u.output_tokens as u32;
                    BedrockTokenUsage {
                        input_tokens: input,
                        output_tokens: output,
                        total_tokens: input + output,
                    }
                }).unwrap_or_default();

                info!(
                    "[LLM][{}] Token usage: input={}, output={}, total={}",
                    language, token_usage.input_tokens, token_usage.output_tokens, token_usage.total_tokens
                );

                // Extract the text from the first content block of the response.
                let output = response.output
                    .context("Bedrock response had no output")?;

                let message = match output {
                    aws_sdk_bedrockruntime::types::ConverseOutput::Message(m) => m,
                    _ => anyhow::bail!("Unexpected Bedrock output type"),
                };

                let text = message
                    .content
                    .into_iter()
                    .find_map(|block| {
                        if let ContentBlock::Text(t) = block {
                            Some(t)
                        } else {
                            None
                        }
                    })
                    .context("No text content block in Bedrock response")?;

                info!("[LLM][{}] Received response ({} chars)", language, text.len());
                return Ok((text, token_usage));
            }
            Ok(Err(sdk_err)) => {
                let retryable = is_bedrock_error_retryable(&sdk_err);

                // Log the error details.
                log_bedrock_error(language, &sdk_err);

                if retryable && attempt < MAX_BEDROCK_ATTEMPTS {
                    let backoff = std::cmp::min(
                        INITIAL_BACKOFF_SECS * 2u64.pow(attempt - 1),
                        MAX_BACKOFF_SECS,
                    );
                    warn!(
                        "[LLM][{}] Transient Bedrock error (attempt {}/{}), retrying in {}s ...",
                        language, attempt, MAX_BEDROCK_ATTEMPTS, backoff,
                    );
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    last_err = Some(anyhow::anyhow!("Bedrock converse call failed: {:#}", sdk_err));
                    continue;
                }

                // Non-retryable or final attempt — return the error.
                return Err(anyhow::anyhow!("Bedrock converse call failed: {:#}", sdk_err));
            }
        }
    }

    // Should only be reached if all attempts were transient failures.
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Bedrock call failed after all retries")))
}

/// Returns `true` if the Bedrock SDK error is transient and worth retrying.
///
/// Retryable conditions:
/// - `ServiceError` with HTTP status 503 (ServiceUnavailable) or 429 (Throttling)
/// - `TimeoutError`
/// - `DispatchFailure` (transient network issue)
fn is_bedrock_error_retryable<E: std::fmt::Debug>(sdk_err: &BedrockSdkError<E>) -> bool {
    match sdk_err {
        BedrockSdkError::ServiceError(svc) => {
            let status = svc.raw().status().as_u16();
            status == 503 || status == 429
        }
        BedrockSdkError::TimeoutError(_) => true,
        BedrockSdkError::DispatchFailure(_) => true,
        _ => false,
    }
}

/// Log a Bedrock SDK error with full detail.
fn log_bedrock_error<E: std::fmt::Display + std::fmt::Debug>(
    language: &str,
    sdk_err: &BedrockSdkError<E>,
) {
    match sdk_err {
        BedrockSdkError::ServiceError(svc) => {
            error!(
                "[LLM][{}] Bedrock ServiceError: {} | meta: {:?}",
                language,
                svc.err(),
                svc.raw().status()
            );
        }
        BedrockSdkError::ConstructionFailure(_) => {
            error!("[LLM][{}] Bedrock request construction failure", language);
        }
        BedrockSdkError::TimeoutError(_) => {
            error!("[LLM][{}] Bedrock request timed out", language);
        }
        BedrockSdkError::DispatchFailure(d) => {
            error!("[LLM][{}] Bedrock dispatch failure: {:?}", language, d);
        }
        BedrockSdkError::ResponseError(r) => {
            error!(
                "[LLM][{}] Bedrock response error (HTTP {})",
                language,
                r.raw().status(),
            );
        }
        _ => {
            error!("[LLM][{}] Bedrock unknown error: {}", language, sdk_err);
        }
    }
}

// ---------------------------------------------------------------------------
// JSON extraction from LLM response
// ---------------------------------------------------------------------------

/// Extract the first JSON object that looks like an IAM policy from the LLM
/// response text.  The model typically wraps the JSON in a markdown code fence.
pub fn extract_policy_from_response(response: &str) -> Option<Value> {
    // Strategy 1: look for a ```json ... ``` fence.
    if let Some(start) = response.find("```json") {
        let after = &response[start + 7..];
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                if is_iam_policy(&v) {
                    return Some(v);
                }
            }
        }
    }

    // Strategy 2: look for a plain ``` ... ``` fence.
    if let Some(start) = response.find("```") {
        let after = &response[start + 3..];
        // Skip a possible language tag on the same line.
        let after = after.trim_start_matches(|c: char| c.is_alphabetic());
        if let Some(end) = after.find("```") {
            let candidate = after[..end].trim();
            if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                if is_iam_policy(&v) {
                    return Some(v);
                }
            }
        }
    }

    // Strategy 3: scan for the first `{` and try to parse from there.
    if let Some(start) = response.find('{') {
        // Find the matching closing brace by counting depth.
        let bytes = response.as_bytes();
        let mut depth = 0usize;
        let mut end = None;
        for (i, &b) in bytes[start..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(end) = end {
            let candidate = &response[start..end];
            if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                if is_iam_policy(&v) {
                    return Some(v);
                }
            }
        }
    }

    None
}

/// Return `true` if *v* looks like an IAM policy document (has `Statement` key).
fn is_iam_policy(v: &Value) -> bool {
    v.get("Statement").is_some()
}

// ---------------------------------------------------------------------------
// Access Analyzer validation
// ---------------------------------------------------------------------------

/// Run IAM Access Analyzer `ValidatePolicy` on *policy_doc* and return the
/// list of findings.  Logs the full AWS service error on failure.
pub async fn validate_policy_with_access_analyzer(
    aa: &AaClient,
    policy_doc: &Value,
) -> Result<Vec<AccessAnalyzerFinding>> {
    let policy_str = serde_json::to_string(policy_doc)
        .context("Failed to serialise policy document for Access Analyzer")?;

    let resp = aa
        .validate_policy()
        .policy_document(&policy_str)
        .policy_type(PolicyType::IdentityPolicy)
        .send()
        .await
        .map_err(|sdk_err| {
            match &sdk_err {
                AaSdkError::ServiceError(svc) => {
                    error!(
                        "[AA] AccessAnalyzer ServiceError: {} | meta: {:?}",
                        svc.err(),
                        svc.raw().status()
                    );
                }
                _ => {
                    error!("[AA] AccessAnalyzer error: {:#}", sdk_err);
                }
            }
            anyhow::anyhow!("AccessAnalyzer:ValidatePolicy call failed: {:#}", sdk_err)
        })?;

    let findings: Vec<AccessAnalyzerFinding> = resp
        .findings
        .into_iter()
        .map(|f: ValidatePolicyFinding| {
            // Location doesn't implement Serialize; format all locations with Debug
            // and join them so no location is silently dropped.
            let details = f
                .locations
                .iter()
                .map(|loc| format!("{:?}", loc))
                .collect::<Vec<_>>()
                .join("; ");
            AccessAnalyzerFinding {
                finding_type: f.finding_type.as_str().to_string(),
                issue_code: f.issue_code,
                details,
                // learn_more_link is String in the SDK but our struct holds Option<String>.
                learn_more_link: Some(f.learn_more_link),
            }
        })
        .collect();

    Ok(findings)
}

// ---------------------------------------------------------------------------
// Concrete-action counting
// ---------------------------------------------------------------------------

/// Count the concrete IAM actions allowed by *policy_doc* using the service
/// catalogue for wildcard expansion.
pub fn count_concrete_actions(policy_doc: &Value, catalogue: &ServiceCatalogue) -> u32 {
    let mut concrete: HashSet<String> = HashSet::new();
    for stmt in extract_allow_statements(policy_doc) {
        for pattern in &stmt.action_patterns {
            let prefix = match pattern.find(':') {
                Some(pos) => pattern[..pos].to_lowercase(),
                None => continue,
            };
            if let Some(actions) = catalogue.get(&prefix) {
                for action in actions {
                    let full = format!("{}:{}", prefix, action);
                    if action_covered_by(pattern, &full) {
                        concrete.insert(full);
                    }
                }
            } else {
                concrete.insert(pattern.clone());
            }
        }
    }
    concrete.len() as u32
}

// ---------------------------------------------------------------------------
// Top-level per-language orchestration
// ---------------------------------------------------------------------------

/// Generate an LLM policy for *language* in *run_dir*, validate it, and return
/// the full [`LlmPolicyOutcome`].
///
/// * `bedrock`  — Bedrock Runtime client.
/// * `aa`       — Access Analyzer client.
/// * `catalogue` — service catalogue for wildcard expansion.
/// * `script_path` — path to the source file for this language.
/// * `model_id` — Bedrock model/inference-profile ID.
/// * `prior_messages` — optional context messages prepended to the conversation
///   (used for the context-filling experiment).  Pass `vec![]` for the simple prompt.
/// * `cdk_stack_path` — optional path to the CDK stack file (`cdk/lib/stack.ts`).
///   When provided, the stack definition is included in the prompt so the LLM
///   can reason about infrastructure details (e.g. KMS-encrypted S3 buckets).
/// * `minimal_concrete_actions` — baseline action count from the minimal policy.
/// * `managed_policy_concrete_actions` — baseline action count from set-cover.
/// * `strategy` — which resource-handling instruction to append to the prompt.
pub async fn generate_llm_policy_for_language(
    bedrock: &BedrockClient,
    aa: &AaClient,
    catalogue: &ServiceCatalogue,
    language: &str,
    script_path: &std::path::Path,
    model_id: &str,
    prior_messages: Vec<Message>,
    cdk_stack_path: Option<&std::path::Path>,
    minimal_concrete_actions: u32,
    managed_policy_concrete_actions: u32,
    strategy: ResourcePromptStrategy,
) -> LlmPolicyOutcome {
    // Read source code.
    let source_code = match std::fs::read_to_string(script_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[LLM][{}] Cannot read script {:?}: {}", language, script_path, e);
            return LlmPolicyOutcome {
                language: language.to_string(),
                policy_generated: false,
                llm_raw_response: None,
                policy_document: None,
                concrete_actions: 0,
                minimal_concrete_actions,
                managed_policy_concrete_actions,
                over_permission_ratio_vs_minimal: 0.0,
                over_permission_ratio_vs_managed: 0.0,
                validation_success: false,
                access_analyzer_findings: vec![],
                access_analyzer_error_count: 0,
                access_analyzer_warning_count: 0,
                access_analyzer_suggestion_count: 0,
                token_usage: BedrockTokenUsage::default(),
            };
        }
    };

    // Read CDK stack code (if provided).
    let cdk_stack_code = cdk_stack_path.and_then(|p| {
        match std::fs::read_to_string(p) {
            Ok(s) => {
                info!("[LLM][{}] Including CDK stack from {:?} ({} chars)", language, p, s.len());
                Some(s)
            }
            Err(e) => {
                warn!("[LLM][{}] Cannot read CDK stack {:?}: {} — proceeding without it", language, p, e);
                None
            }
        }
    });

    // Call Bedrock.
    let (raw_response, token_usage) = match call_bedrock_for_policy(bedrock, &source_code, language, model_id, prior_messages, cdk_stack_code.as_deref(), strategy).await {
        Ok(r) => r,
        Err(e) => {
            warn!("[LLM][{}] Bedrock call failed: {:#}", language, e);
            return LlmPolicyOutcome {
                language: language.to_string(),
                policy_generated: false,
                llm_raw_response: None,
                policy_document: None,
                concrete_actions: 0,
                minimal_concrete_actions,
                managed_policy_concrete_actions,
                over_permission_ratio_vs_minimal: 0.0,
                over_permission_ratio_vs_managed: 0.0,
                validation_success: false,
                access_analyzer_findings: vec![],
                access_analyzer_error_count: 0,
                access_analyzer_warning_count: 0,
                access_analyzer_suggestion_count: 0,
                token_usage: BedrockTokenUsage::default(),
            };
        }
    };

    // Extract JSON policy.
    let policy_doc = match extract_policy_from_response(&raw_response) {
        Some(p) => p,
        None => {
            warn!(
                "[LLM][{}] Could not extract IAM policy JSON from LLM response",
                language
            );
            return LlmPolicyOutcome {
                language: language.to_string(),
                policy_generated: false,
                llm_raw_response: Some(raw_response),
                policy_document: None,
                concrete_actions: 0,
                minimal_concrete_actions,
                managed_policy_concrete_actions,
                over_permission_ratio_vs_minimal: 0.0,
                over_permission_ratio_vs_managed: 0.0,
                validation_success: false,
                access_analyzer_findings: vec![],
                access_analyzer_error_count: 0,
                access_analyzer_warning_count: 0,
                access_analyzer_suggestion_count: 0,
                token_usage,
            };
        }
    };

    // Count concrete actions.
    let concrete_actions = count_concrete_actions(&policy_doc, catalogue);
    let ratio_vs_minimal = concrete_actions as f64 / minimal_concrete_actions.max(1) as f64;
    let ratio_vs_managed =
        concrete_actions as f64 / managed_policy_concrete_actions.max(1) as f64;

    info!(
        "[LLM][{}] Policy extracted: {} concrete actions (vs minimal {}, vs managed {})",
        language, concrete_actions, minimal_concrete_actions, managed_policy_concrete_actions
    );

    // Run Access Analyzer validation.
    let aa_findings = match validate_policy_with_access_analyzer(aa, &policy_doc).await {
        Ok(f) => f,
        Err(e) => {
            warn!("[LLM][{}] Access Analyzer validation failed: {:#}", language, e);
            vec![]
        }
    };

    let aa_error_count = aa_findings
        .iter()
        .filter(|f| f.finding_type.to_uppercase() == "ERROR")
        .count();
    let aa_warning_count = aa_findings
        .iter()
        .filter(|f| f.finding_type.to_uppercase() == "WARNING")
        .count();
    let aa_suggestion_count = aa_findings
        .iter()
        .filter(|f| f.finding_type.to_uppercase() == "SUGGESTION")
        .count();

    info!(
        "[LLM][{}] Access Analyzer: {} errors, {} warnings, {} suggestions",
        language, aa_error_count, aa_warning_count, aa_suggestion_count
    );

    LlmPolicyOutcome {
        language: language.to_string(),
        policy_generated: true,
        llm_raw_response: Some(raw_response),
        policy_document: Some(policy_doc),
        concrete_actions,
        minimal_concrete_actions,
        managed_policy_concrete_actions,
        over_permission_ratio_vs_minimal: ratio_vs_minimal,
        over_permission_ratio_vs_managed: ratio_vs_managed,
        // validation_success is filled in by the caller after live execution.
        validation_success: false,
        access_analyzer_findings: aa_findings,
        access_analyzer_error_count: aa_error_count,
        access_analyzer_warning_count: aa_warning_count,
        access_analyzer_suggestion_count: aa_suggestion_count,
        token_usage,
    }
}
