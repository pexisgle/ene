use super::definition::ToolDefinition;
use crate::error::AiCoreError;
use crate::sandbox::SandboxConfig;
use std::path::Path;

const MAX_LINE_LENGTH: usize = 2000;
const MAX_LINE_SUFFIX: &str = "... (line truncated)";
const SAMPLE_BYTES: usize = 4096;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read".to_string(),
        description: concat!(
            "Read a file or directory from the local filesystem. ",
            "If the path does not exist, an error is returned. ",
            "Contents are returned with each line prefixed by its line number as `<line>: <content>`. ",
            "For directories, entries are returned one per line with a trailing `/` for subdirectories. ",
            "Any line longer than 2000 characters is truncated. ",
            "Call this tool in parallel when you know there are multiple files you want to read."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "filePath": { "type": "string", "description": "The absolute path to the file or directory to read" },
                "offset": { "type": "integer", "description": "The line number to start reading from (1-indexed)" },
                "limit": { "type": "integer", "description": "The maximum number of lines to read (defaults to 2000)" }
            },
            "required": ["filePath"]
        }),
    }
}

/// ファイルまたはディレクトリを読み取る
pub async fn read(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    sandbox: &SandboxConfig,
) -> Result<String, AiCoreError> {
    let resolved = sandbox.resolve_and_check(path, false)?;

    if !resolved.exists() {
        // 候補を探す
        let dir = resolved.parent().unwrap_or(Path::new("."));
        let base = resolved.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let mut suggestions = Vec::new();
        if let Ok(entries) = tokio::fs::read_dir(dir).await {
            let mut entries = entries;
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(name) = entry.file_name().into_string() {
                    if name.to_lowercase().contains(&base.to_lowercase())
                        || base.to_lowercase().contains(&name.to_lowercase())
                    {
                        suggestions.push(format!("{}", entry.path().display()));
                        if suggestions.len() >= 3 {
                            break;
                        }
                    }
                }
            }
        }
        if suggestions.is_empty() {
            return Err(AiCoreError::FileNotFound(format!(
                "File not found: {}",
                resolved.display()
            )));
        } else {
            return Err(AiCoreError::FileNotFound(format!(
                "File not found: {}\n\nDid you mean one of these?\n{}",
                resolved.display(),
                suggestions.join("\n")
            )));
        }
    }

    let metadata = tokio::fs::metadata(&resolved).await.map_err(|e| {
        AiCoreError::FileNotFound(format!("Cannot stat {}: {}", resolved.display(), e))
    })?;

    if metadata.is_dir() {
        return read_directory(&resolved, offset, limit).await;
    }

    // バイナリチェック
    let sample = tokio::fs::read(&resolved).await.map_err(|e| {
        AiCoreError::FileNotFound(format!("Cannot read {}: {}", resolved.display(), e))
    })?;

    if is_binary_file(&resolved, &sample) {
        return Err(AiCoreError::FileNotFound(format!(
            "Cannot read binary file: {}",
            resolved.display()
        )));
    }

    let text = String::from_utf8_lossy(&sample);

    // サイズチェック
    if sample.len() > sandbox.max_read_bytes {
        // 大きいファイルは行単位で制限
        let lines: Vec<&str> = text.lines().collect();
        let default_limit = 2000usize;
        let start = offset.unwrap_or(1).saturating_sub(1);
        let end = (start + limit.unwrap_or(default_limit)).min(lines.len());
        let sliced = &lines[start..end];

        let mut output = format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n",
            resolved.display()
        );
        for (i, line) in sliced.iter().enumerate() {
            let line_num = start + i + 1;
            let truncated = if line.len() > MAX_LINE_LENGTH {
                format!("{}{}", &line[..MAX_LINE_LENGTH], MAX_LINE_SUFFIX)
            } else {
                line.to_string()
            };
            output.push_str(&format!("{}: {}\n", line_num, truncated));
        }
        output.push_str(&format!(
            "\n(Output capped at {}KB. Showing lines {}-{}. Use offset={} to continue.)\n</content>",
            sandbox.max_read_bytes / 1024,
            start + 1,
            end,
            end + 1
        ));
        return Ok(output);
    }

    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).saturating_sub(1);
    if start > lines.len() && !(lines.is_empty() && start == 0) {
        return Err(AiCoreError::FileNotFound(format!(
            "Offset {} is out of range for this file ({} lines)",
            start + 1,
            lines.len()
        )));
    }

    let default_limit = 2000usize;
    let end = (start + limit.unwrap_or(default_limit)).min(lines.len());
    let sliced = &lines[start..end];

    let mut output = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n",
        resolved.display()
    );
    for (i, line) in sliced.iter().enumerate() {
        let line_num = start + i + 1;
        let truncated = if line.len() > MAX_LINE_LENGTH {
            format!("{}{}", &line[..MAX_LINE_LENGTH], MAX_LINE_SUFFIX)
        } else {
            line.to_string()
        };
        output.push_str(&format!("{}: {}\n", line_num, truncated));
    }

    let last = start + sliced.len();
    let next = last + 1;
    let truncated = end < lines.len();
    if truncated {
        output.push_str(&format!(
            "\n(Showing lines {}-{} of {}. Use offset={} to continue.)\n</content>",
            start + 1,
            last,
            lines.len(),
            next
        ));
    } else {
        output.push_str(&format!(
            "\n(End of file - total {} lines)\n</content>",
            lines.len()
        ));
    }

    Ok(output)
}

async fn read_directory(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, AiCoreError> {
    let mut entries = tokio::fs::read_dir(path).await.map_err(|e| {
        AiCoreError::FileNotFound(format!("Cannot read directory {}: {}", path.display(), e))
    })?;

    let mut items = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().into_string().unwrap_or_default();
        let file_type = entry.file_type().await.ok();
        let suffix = if file_type.map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        items.push(format!("{}{}", name, suffix));
    }

    items.sort();

    let start = offset.unwrap_or(1).saturating_sub(1);
    let default_limit = 2000usize;
    let end = (start + limit.unwrap_or(default_limit)).min(items.len());
    let sliced = &items[start..end];
    let truncated = end < items.len();

    let mut output = format!(
        "<path>{}</path>\n<type>directory</type>\n<entries>\n",
        path.display()
    );
    for item in sliced {
        output.push_str(&format!("{}\n", item));
    }
    if truncated {
        output.push_str(&format!(
            "\n(Showing {} of {} entries. Use 'offset' parameter to read beyond entry {}.)\n</entries>",
            sliced.len(),
            items.len(),
            start + sliced.len() + 1
        ));
    } else {
        output.push_str(&format!("\n({} entries)\n</entries>", items.len()));
    }

    Ok(output)
}

fn is_binary_file(path: &Path, bytes: &[u8]) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "tar" | "gz" | "exe" | "dll" | "so" | "class" | "jar" | "war" | "7z" | "doc"
        | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "bin" | "dat"
        | "obj" | "o" | "a" | "lib" | "wasm" | "pyc" | "pyo" => return true,
        _ => {}
    }

    if bytes.is_empty() {
        return false;
    }

    let mut non_printable = 0usize;
    for &b in bytes.iter().take(SAMPLE_BYTES.min(bytes.len())) {
        if b == 0 {
            return true;
        }
        if b < 9 || (b > 13 && b < 32) {
            non_printable += 1;
        }
    }

    let checked = SAMPLE_BYTES.min(bytes.len());
    non_printable > checked / 3
}
