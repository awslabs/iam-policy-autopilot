//! Scenario context loading for the context-filling LLM experiment.
//!
//! Reads per-language scenario files from a directory and builds Bedrock
//! `Message` pairs suitable for pre-filling the conversation context.

use std::path::{Path, PathBuf};

use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message as BedrockMessage};
use tracing::{info, warn};

/// Load all files from `<scenarios_dir>/<language>/` and build a sequence of
/// Bedrock `Message` pairs suitable for context-filling:
///
///   User:      "Here is a reference file `<filename>`:\n```\n<content>\n```"
///   Assistant: "Understood. I have read `<filename>`."
///
/// Files are sorted by name for deterministic ordering.  Binary files and
/// files that cannot be read as UTF-8 are silently skipped.
///
/// Returns an empty `Vec` if the language sub-directory does not exist.
pub fn load_scenario_context_messages(
    scenarios_dir: &Path,
    language: &str,
) -> Vec<BedrockMessage> {
    let lang_dir = scenarios_dir.join(language);
    if !lang_dir.is_dir() {
        warn!(
            "[CTX-LLM][{}] Scenario directory {:?} not found — no context messages",
            language, lang_dir
        );
        return vec![];
    }

    // Collect all files recursively, sorted for determinism.
    let mut file_paths: Vec<PathBuf> = Vec::new();
    collect_files_recursive(&lang_dir, &mut file_paths);
    file_paths.sort();

    let mut messages: Vec<BedrockMessage> = Vec::new();

    for path in &file_paths {
        let rel = path.strip_prefix(&lang_dir).unwrap_or(path);
        let filename = rel.to_string_lossy();

        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                // Binary or unreadable file — skip silently.
                continue;
            }
        };

        // User turn: present the file.
        let user_text = format!(
            "Here is a reference file `{}`:\n```\n{}\n```",
            filename, content
        );
        let user_msg = match BedrockMessage::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Text(user_text))
            .build()
        {
            Ok(m) => m,
            Err(e) => {
                warn!("[CTX-LLM][{}] Failed to build user message for {:?}: {}", language, path, e);
                continue;
            }
        };

        // Assistant turn: acknowledge.
        let assistant_text = format!("Understood. I have read `{}`.", filename);
        let assistant_msg = match BedrockMessage::builder()
            .role(ConversationRole::Assistant)
            .content(ContentBlock::Text(assistant_text))
            .build()
        {
            Ok(m) => m,
            Err(e) => {
                warn!("[CTX-LLM][{}] Failed to build assistant message for {:?}: {}", language, path, e);
                continue;
            }
        };

        messages.push(user_msg);
        messages.push(assistant_msg);
    }

    info!(
        "[CTX-LLM][{}] Built {} context messages from {} files in {:?}",
        language,
        messages.len(),
        file_paths.len(),
        lang_dir
    );
    messages
}

/// Recursively collect all regular files under `dir` into `out`.
fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}
