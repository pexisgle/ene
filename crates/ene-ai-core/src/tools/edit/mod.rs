use std::path::Path;
use crate::sandbox::SandboxConfig;
use crate::error::AiCoreError;
use super::undo_manager::UndoManager;
use super::definition::ToolDefinition;
use tokio::sync::Semaphore;
use std::sync::Arc;
use std::collections::HashMap;

mod simple;
mod line_trimmed;
mod block_anchor;
mod whitespace_normalized;
mod indentation_flexible;
mod escape_normalized;
mod trimmed_boundary;
mod context_aware;
mod multi_occurrence;

use simple::simple_replace;
use line_trimmed::line_trimmed_replace;
use block_anchor::block_anchor_replace;
use whitespace_normalized::whitespace_normalized_replace;
use indentation_flexible::indentation_flexible_replace;
use escape_normalized::escape_normalized_replace;
use trimmed_boundary::trimmed_boundary_replace;
use context_aware::context_aware_replace;
use multi_occurrence::multi_occurrence_replace;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "edit".to_string(),
        description: concat!(
            "Performs exact string replacements in files. ",
            "You must use your Read tool at least once in the conversation before editing. ",
            "When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. ",
            "The edit will FAIL if oldString is not found in the file. ",
            "The edit will FAIL if oldString is found multiple times in the file. Either provide more surrounding lines in oldString to make it unique or use replaceAll to change every instance. ",
            "Use replaceAll for replacing and renaming strings across the file."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "filePath": { "type": "string", "description": "The absolute path to the file to modify" },
                "oldString": { "type": "string", "description": "The text to replace" },
                "newString": { "type": "string", "description": "The text to replace it with (must be different from oldString)" },
                "replaceAll": { "type": "boolean", "description": "Replace all occurrences of oldString (default false)" }
            },
            "required": ["filePath", "oldString", "newString"]
        }),
    }
}

lazy_static::lazy_static! {
    static ref FILE_LOCKS: std::sync::Mutex<HashMap<String, Arc<Semaphore>>> = std::sync::Mutex::new(HashMap::new());
}

fn get_lock(path: &str) -> Arc<Semaphore> {
    let mut locks = FILE_LOCKS.lock().unwrap();
    locks.entry(path.to_string()).or_insert_with(|| Arc::new(Semaphore::new(1))).clone()
}

pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

pub fn detect_line_ending(text: &str) -> &str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Levenshtein distance (using strsim crate)
pub fn levenshtein(a: &str, b: &str) -> usize {
    strsim::levenshtein(a, b)
}

/// 類似度に基づく最良マッチを見つける
pub fn find_best_match<'a>(needle: &str, haystack: &'a str) -> Option<(usize, &'a str, f64)> {
    if needle.is_empty() {
        return None;
    }
    let needle_len = needle.len();
    let mut best: Option<(usize, &'a str, f64)> = None;

    // スライディングウィンドウで最も類似した部分文字列を探す
    let max_window = (needle_len * 2).max(100);
    let step = needle_len.max(1);

    for start in (0..haystack.len().saturating_sub(needle_len / 2)).step_by(step) {
        let end = (start + max_window).min(haystack.len());
        let window = &haystack[start..end];
        let dist = strsim::levenshtein(needle, window);
        let max_len = needle_len.max(window.len());
        let similarity = if max_len == 0 {
            1.0
        } else {
            1.0 - (dist as f64 / max_len as f64)
        };

        if similarity >= 0.7 {
            if best.map_or(true, |(_, _, b_sim)| similarity > b_sim) {
                best = Some((start, window, similarity));
            }
        }
    }
    best
}

pub async fn edit(
    path: &Path,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, AiCoreError> {
    if old_string == new_string {
        return Err(AiCoreError::ToolExecutionError(
            "No changes to apply: oldString and newString are identical.".to_string()
        ));
    }

    let resolved = sandbox.resolve_and_check(path, true)?;

    if !resolved.exists() {
        return Err(AiCoreError::FileNotFound(format!("File not found: {}", resolved.display())));
    }

    let lock = get_lock(&resolved.to_string_lossy());
    let _permit = lock.acquire().await.map_err(|e| AiCoreError::ToolExecutionError(format!("Lock error: {e}")))?;

    let content = tokio::fs::read_to_string(&resolved).await
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Cannot read file: {e}")))?;

    let original = content.clone();
    let ending = detect_line_ending(&content);
    let normalized_content = normalize_line_endings(&content);
    let normalized_old = normalize_line_endings(old_string);
    let normalized_new = normalize_line_endings(new_string);

    let result = if old_string.is_empty() {
        Some(normalized_new + &normalized_content)
    } else {
        let replacers: Vec<fn(&str, &str, &str, bool) -> Option<String>> = vec![
            simple_replace,
            line_trimmed_replace,
            block_anchor_replace,
            whitespace_normalized_replace,
            indentation_flexible_replace,
            escape_normalized_replace,
            trimmed_boundary_replace,
            context_aware_replace,
            multi_occurrence_replace,
        ];

        let mut found = None;
        for replacer in &replacers {
            if let Some(replaced) = replacer(&normalized_content, &normalized_old, &normalized_new, replace_all) {
                found = Some(replaced);
                break;
            }
        }
        found
    };

    let new_content = match result {
        Some(c) => c,
        None => {
            let simple_matches: Vec<_> = normalized_content.match_indices(&normalized_old).collect();
            if simple_matches.len() > 1 && !replace_all {
                return Err(AiCoreError::ToolExecutionError(
                    "Found multiple matches for oldString. Provide more surrounding context to make the match unique.".to_string()
                ));
            }
            return Err(AiCoreError::ToolExecutionError(
                "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings.".to_string()
            ));
        }
    };

    let final_content = if ending == "\r\n" {
        new_content.replace("\n", "\r\n")
    } else {
        new_content
    };

    tokio::fs::write(&resolved, final_content).await
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Failed to write file: {e}")))?;

    undo_manager.push_restore_file(session_id, "edit", resolved.clone(), Some(original.into_bytes()));

    Ok("Edit applied successfully.".to_string())
}
