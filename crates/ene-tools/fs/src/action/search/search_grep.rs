use super::MAX_RESULTS;
use crate::sandbox::SandboxConfig;
use ene_tool_proto::ToolError;
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

        if let Some(inc) = include {
            let file_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !glob::Pattern::new(inc)
                .map(|p| p.matches(file_name))
                .unwrap_or(false)
            {
                continue;
            }
        }

        let metadata = match tokio::fs::metadata(file_path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if metadata.len() > 1024 * 1024 {
            continue;
        }

        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(_) => continue,
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
            format!(" (showing first {})", MAX_RESULTS)
        } else {
            "".to_string()
        }
    )];

    let mut current_file = "";
    for (path, line_num, text) in &matches {
        if current_file != path {
            if !current_file.is_empty() {
                output.push("".to_string());
            }
            current_file = path;
            output.push(format!("{}:", path));
        }
        let truncated_text = if text.chars().count() > 2000 {
            let byte_end = text
                .char_indices()
                .nth(2000)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!("{}...", &text[..byte_end])
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
