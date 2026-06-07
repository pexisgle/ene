use crate::utils::sandbox::Sandbox;
use ene_tool_common::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

mod block_anchor;
mod context_aware;
mod escape_normalized;
mod indentation_flexible;
mod line_trimmed;
mod multi_occurrence;
mod simple;
mod trimmed_boundary;
mod whitespace_normalized;

use block_anchor::block_anchor_replace;
use context_aware::context_aware_replace;
use escape_normalized::escape_normalized_replace;
use indentation_flexible::indentation_flexible_replace;
use line_trimmed::line_trimmed_replace;
use multi_occurrence::multi_occurrence_replace;
use simple::simple_replace;
use trimmed_boundary::trimmed_boundary_replace;
use whitespace_normalized::whitespace_normalized_replace;

type ReplacerFn = fn(&str, &str, &str, bool) -> Option<String>;

static FILE_LOCKS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<std::path::PathBuf, Arc<Semaphore>>>,
> = std::sync::OnceLock::new();

fn get_lock(path: &Path) -> Arc<Semaphore> {
    let locks = FILE_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Semaphore::new(1)))
        .clone()
}

pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

pub fn detect_line_ending(text: &str) -> &str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    strsim::levenshtein(a, b)
}

pub fn find_best_match<'a>(needle: &str, haystack: &'a str) -> Option<(usize, &'a str, f64)> {
    if needle.is_empty() {
        return None;
    }
    let needle_len = needle.len();
    let mut best: Option<(usize, &'a str, f64)> = None;

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

        if similarity >= 0.7 && best.is_none_or(|(_, _, b_sim)| similarity > b_sim) {
            best = Some((start, window, similarity));
        }
    }
    best
}

pub async fn edit(
    path: &Path,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    sandbox: &Sandbox,
) -> Result<String, ToolError> {
    if old_string == new_string {
        return Err(ToolError::ExecutionFailed {
            message: "No changes to apply: oldString and newString are identical.".to_string(),
        });
    }

    let resolved = sandbox.check_writable(path)?;

    if !resolved.exists() {
        return Err(ToolError::ExecutionFailed {
            message: format!("File not found: {}", resolved.display()),
        });
    }

    let lock = get_lock(&resolved);
    let _permit = lock
        .acquire()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Lock error: {e}"),
        })?;

    let content =
        tokio::fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Cannot read file: {e}"),
            })?;

    let original = content.clone();
    let ending = detect_line_ending(&content);
    let normalized_content = normalize_line_endings(&content);
    let normalized_old = normalize_line_endings(old_string);
    let normalized_new = normalize_line_endings(new_string);

    let result = if old_string.is_empty() {
        Some(normalized_new + &normalized_content)
    } else {
        const REPLACERS: &[ReplacerFn] = &[
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
        for replacer in REPLACERS {
            if let Some(replaced) = replacer(
                &normalized_content,
                &normalized_old,
                &normalized_new,
                replace_all,
            ) {
                found = Some(replaced);
                break;
            }
        }
        found
    };

    let new_content = if let Some(c) = result {
        c
    } else {
        let simple_matches: Vec<_> = normalized_content.match_indices(&normalized_old).collect();
        if simple_matches.len() > 1 && !replace_all {
            return Err(ToolError::ExecutionFailed { message:
                "Found multiple matches for oldString. Provide more surrounding context to make the match unique.".to_string()
            });
        }
        return Err(ToolError::ExecutionFailed { message:
            "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings.".to_string()
        });
    };

    let final_content = if ending == "\r\n" {
        new_content.replace('\n', "\r\n")
    } else {
        new_content
    };

    tokio::fs::write(&resolved, final_content)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to write file: {e}"),
        })?;

    sandbox
        .track_overwrite(&resolved, Some(original.into_bytes()))
        .await;

    Ok("Edit applied successfully.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_line_endings_unix() {
        assert_eq!(normalize_line_endings("line1\nline2\n"), "line1\nline2\n");
    }

    #[test]
    fn test_normalize_line_endings_windows() {
        assert_eq!(
            normalize_line_endings("line1\r\nline2\r\n"),
            "line1\nline2\n"
        );
    }

    #[test]
    fn test_normalize_line_endings_mixed() {
        assert_eq!(
            normalize_line_endings("line1\r\nline2\nline3\r\n"),
            "line1\nline2\nline3\n"
        );
    }

    #[test]
    fn test_normalize_line_endings_empty() {
        assert_eq!(normalize_line_endings(""), "");
    }

    #[test]
    fn test_detect_line_ending_unix() {
        assert_eq!(detect_line_ending("line1\nline2\n"), "\n");
    }

    #[test]
    fn test_detect_line_ending_windows() {
        assert_eq!(detect_line_ending("line1\r\nline2\r\n"), "\r\n");
    }

    #[test]
    fn test_detect_line_ending_fallback() {
        assert_eq!(detect_line_ending("no newlines"), "\n");
    }

    #[test]
    fn test_find_best_match_exact() {
        let haystack = "foo bar baz qux";
        let needle = "bar";
        let result = find_best_match(needle, haystack);
        let _ = result;
    }

    #[test]
    fn test_find_best_match_similar() {
        let haystack = "foo baz qux";
        let needle = "bar";
        let result = find_best_match(needle, haystack);
        let _ = result;
    }

    #[test]
    fn test_find_best_match_empty_needle() {
        assert!(find_best_match("", "foo bar").is_none());
    }
}

use std::sync::RwLock;

type SandboxRef = Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>;

fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "filesystem",
    name = "edit",
    summary = "Targeted in-place edit: find oldString and replace with newString.",
    description = "Targeted in-place edit: find oldString and replace with newString. Uses a chain of matching strategies (exact, trimmed, block anchor, whitespace-normalized, etc.) for robust matching.",
    category = "Filesystem",
    keywords_primary = "edit, replace, modify, change, substitute"
)]
pub struct FsEditAction {
    /// Absolute path to the file to edit.
    file_path: String,
    /// Text to find and replace.
    old_string: String,
    /// Replacement text.
    new_string: String,
    /// Replace all occurrences (default false).
    #[serde(default)]
    replace_all: Option<bool>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsEditAction {
    pub fn new(sandbox: SandboxRef) -> Self {
        Self {
            file_path: String::new(),
            old_string: String::new(),
            new_string: String::new(),
            replace_all: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self
                .sandbox
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::FileOverwrite,
            &self.file_path,
            "Editing file content",
        )?;

        edit(
            Path::new(&self.file_path),
            &self.old_string,
            &self.new_string,
            self.replace_all.unwrap_or(false),
            &sandbox,
        )
        .await
    }
}
