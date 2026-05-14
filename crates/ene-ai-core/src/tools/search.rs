use std::path::Path;
use crate::sandbox::SandboxConfig;
use crate::error::AiCoreError;
use super::definition::ToolDefinition;

const MAX_RESULTS: usize = 100;

pub fn glob_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "glob".to_string(),
        description: "Fast file pattern matching tool that works with any codebase size. Supports glob patterns like '**/*.rs' or 'src/**/*.ts'. Returns matching file paths sorted by modification time.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "The glob pattern to match files against" },
                "path": { "type": "string", "description": "The directory to search in. Defaults to current working directory." }
            },
            "required": ["pattern"]
        }),
    }
}

pub fn grep_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "grep".to_string(),
        description: "Fast content search tool that works with any codebase size. Searches file contents using regular expressions. Supports full regex syntax. Filter files by pattern with the include parameter.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "The regex pattern to search for in file contents" },
                "path": { "type": "string", "description": "The directory to search in. Defaults to current working directory." },
                "include": { "type": "string", "description": "File pattern to include in the search (e.g. '*.rs', '*.{ts,tsx}')" }
            },
            "required": ["pattern"]
        }),
    }
}

/// glob パターンでファイルを検索
pub async fn glob_search(pattern: &str, path: Option<&str>, sandbox: &SandboxConfig) -> Result<String, AiCoreError> {
    let base = if let Some(p) = path {
        let resolved = sandbox.resolve_and_check(Path::new(p), false)?;
        resolved
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    if !base.is_dir() {
        return Err(AiCoreError::FileNotFound(format!("glob path must be a directory: {}", base.display())));
    }

    let pattern_path = base.join(pattern);
    let glob_pattern = pattern_path.to_string_lossy().to_string();

    let mut files = Vec::new();
    match glob::glob(&glob_pattern) {
        Ok(entries) => {
            for entry in entries.flatten() {
                files.push(entry.to_string_lossy().to_string());
                if files.len() > MAX_RESULTS {
                    break;
                }
            }
        }
        Err(e) => return Err(AiCoreError::ToolExecutionError(format!("Invalid glob pattern: {e}"))),
    }

    let truncated = files.len() > MAX_RESULTS;
    if truncated {
        files.truncate(MAX_RESULTS);
    }

    files.sort();

    let mut output = Vec::new();
    if files.is_empty() {
        output.push("No files found".to_string());
    } else {
        output.extend(files);
        if truncated {
            output.push("".to_string());
            output.push(format!(
                "(Results are truncated: showing first {} results. Consider using a more specific path or pattern.)",
                MAX_RESULTS
            ));
        }
    }

    Ok(output.join("\n"))
}

/// 正規表現でファイル内容を検索
pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, AiCoreError> {
    if pattern.is_empty() {
        return Err(AiCoreError::ToolExecutionError("pattern is required".to_string()));
    }

    let re = regex::Regex::new(pattern)
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Invalid regex pattern: {e}")))?;

    let base = if let Some(p) = path {
        let resolved = sandbox.resolve_and_check(Path::new(p), false)?;
        resolved
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
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();

        // include フィルタ
        if let Some(inc) = include {
            let file_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !glob::Pattern::new(inc).map(|p| p.matches(file_name)).unwrap_or(false) {
                continue;
            }
        }

        // サイズチェック
        let metadata = match tokio::fs::metadata(file_path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if metadata.len() > 1024 * 1024 {
            continue; // Skip files > 1MB
        }

        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(_) => continue, // Binary or unreadable
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

    let mut output = vec![format!("Found {} matches{}", total, if truncated { format!(" (showing first {})", MAX_RESULTS) } else { "".to_string() })];

    let mut current_file = "";
    for (path, line_num, text) in &matches {
        if current_file != path {
            if !current_file.is_empty() {
                output.push("".to_string());
            }
            current_file = path;
            output.push(format!("{}:", path));
        }
        let truncated_text = if text.len() > 2000 {
            format!("{}...", &text[..2000])
        } else {
            text.clone()
        };
        output.push(format!("  Line {}: {}", line_num, truncated_text));
    }

    if truncated {
        output.push("".to_string());
        output.push(format!(
            "(Results truncated: showing {} of {} matches ({} hidden). Consider using a more specific path or pattern.)",
            MAX_RESULTS, total, total - MAX_RESULTS
        ));
    }

    Ok(output.join("\n"))
}
