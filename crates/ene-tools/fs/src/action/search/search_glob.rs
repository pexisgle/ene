use crate::sandbox::SandboxConfig;
use super::MAX_RESULTS;
use ene_tool_proto::ToolError;
use std::path::Path;

pub async fn glob_search(
    pattern: &str,
    path: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    let base = if let Some(p) = path {
        let resolved = sandbox.resolve_and_check(Path::new(p), false)?;
        resolved
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    if !base.is_dir() {
        return Err(ToolError::ExecutionFailed {
            message: format!("glob path must be a directory: {}", base.display()),
        });
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
        Err(e) => {
            return Err(ToolError::ExecutionFailed {
                message: format!("Invalid glob pattern: {e}"),
            });
        }
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
