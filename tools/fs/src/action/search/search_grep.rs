use super::MAX_RESULTS;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_tool_common::prelude::*;
use std::path::Path;

pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    if pattern.is_empty() {
        return Err(ToolError::ExecutionFailed {
            message: "pattern is required".to_string(),
        });
    }

    let re = regex::Regex::new(pattern).map_err(|e| ToolError::ExecutionFailed {
        message: format!("Invalid regex pattern: {e}"),
    })?;

    let base = if let Some(p) = path {
        sandbox.resolve_and_check(Path::new(p), false)?
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    let search_dir = if base.is_dir() {
        base.clone()
    } else {
        base.parent().unwrap_or(&base).to_path_buf()
    };

    let mut matches = Vec::new();

    let walker = walkdir::WalkDir::new(&search_dir)
        .follow_links(false)
        .max_depth(10);

    for entry in walker {
        let Ok(entry) = entry else {
            continue;
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();

        if let Some(inc) = include {
            let file_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !glob::Pattern::new(inc).is_ok_and(|p| p.matches(file_name)) {
                continue;
            }
        }

        let Ok(metadata) = std::fs::metadata(file_path) else {
            continue;
        };
        if metadata.len() > 1024 * 1024 {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(file_path) else {
            continue;
        };

        for (line_num, line) in content.lines().enumerate() {
            if re.is_match(line) {
                matches.push((
                    file_path.to_string_lossy().to_string(),
                    line_num + 1,
                    line.to_string(),
                ));
                if matches.len() > MAX_RESULTS {
                    break;
                }
            }
        }

        if matches.len() > MAX_RESULTS {
            break;
        }
    }

    let truncated = matches.len() > MAX_RESULTS;
    let total = matches.len();
    if truncated {
        matches.truncate(MAX_RESULTS);
    }

    if matches.is_empty() {
        return Ok("No files found".to_string());
    }

    let mut output = vec![format!(
        "Found {} matches{}",
        total,
        if truncated {
            format!(" (showing first {MAX_RESULTS})")
        } else {
            String::new()
        }
    )];

    let mut current_file = "";
    for (path, line_num, text) in &matches {
        if current_file != path {
            if !current_file.is_empty() {
                output.push(String::new());
            }
            current_file = path;
            output.push(format!("{path}:"));
        }
        let truncated_text = if text.chars().count() > 2000 {
            let byte_end = text.char_indices().nth(2000).map_or(text.len(), |(i, _)| i);
            format!("{}...", &text[..byte_end])
        } else {
            text.clone()
        };
        output.push(format!("  Line {line_num}: {truncated_text}"));
    }

    if truncated {
        output.push(String::new());
        output.push(format!(
            "(Results truncated: showing {} of {} matches ({} hidden). Consider using a more specific path or pattern.)",
            MAX_RESULTS, total, total - MAX_RESULTS
        ));
    }

    Ok(output.join("\n"))
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "filesystem",
    name = "grep",
    summary = "Search for regex patterns within file contents.",
    description = "Search for regex patterns within file contents.",
    category = "Filesystem",
    keywords_primary = "grep, search, regex, find, pattern, content"
)]
pub struct FsGrepAction {
    /// Regex pattern to search for.
    pattern: String,
    /// Base directory or file to search in (defaults to cwd).
    #[serde(default)]
    path: Option<String>,
    /// File glob filter (e.g. '*.rs', '*.{ts,tsx}').
    #[serde(default)]
    include: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsGrepAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            pattern: String::new(),
            path: None,
            include: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);

        grep_search(
            &self.pattern,
            self.path.as_deref(),
            self.include.as_deref(),
            sandbox.config(),
        )
        .await
    }
}
