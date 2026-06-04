mod read_binary;

use self::read_binary::is_binary_file;
use crate::utils::sandbox::SandboxConfig;
use ene_tool_proto::ToolError;
use std::path::Path;

const MAX_LINE_LENGTH: usize = 2000;
const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_SUFFIX: &str = "... (line truncated)";

pub async fn read(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    let resolved = sandbox.resolve_and_check(path, false)?;

    if !resolved.exists() {
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
            return Err(ToolError::ExecutionFailed {
                message: format!("File not found: {}", resolved.display()),
            });
        } else {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "File not found: {}\n\nDid you mean one of these?\n{}",
                    resolved.display(),
                    suggestions.join("\n")
                ),
            });
        }
    }

    let metadata =
        tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Cannot stat {}: {}", resolved.display(), e),
            })?;

    if metadata.is_dir() {
        return read_directory(&resolved, offset, limit).await;
    }

    let sample = tokio::fs::read(&resolved)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Cannot read {}: {}", resolved.display(), e),
        })?;

    if is_binary_file(&resolved, &sample) {
        return Err(ToolError::ExecutionFailed {
            message: format!("Cannot read binary file: {}", resolved.display()),
        });
    }

    let text = String::from_utf8_lossy(&sample);

    if sample.len() > sandbox.max_read_bytes {
        let lines: Vec<&str> = text.lines().collect();
        let default_limit = DEFAULT_LINE_LIMIT;
        let start = offset.unwrap_or(1).saturating_sub(1);
        let end = (start + limit.unwrap_or(default_limit)).min(lines.len());
        let sliced = &lines[start..end];

        let mut output = format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n",
            resolved.display()
        );
        for (i, line) in sliced.iter().enumerate() {
            let line_num = start + i + 1;
            let truncated = if line.chars().count() > MAX_LINE_LENGTH {
                let s: String = line.chars().take(MAX_LINE_LENGTH).collect();
                format!("{}{}", s, MAX_LINE_SUFFIX)
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
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "Offset {} is out of range for this file ({} lines)",
                start + 1,
                lines.len()
            ),
        });
    }

    let default_limit = DEFAULT_LINE_LIMIT;
    let end = (start + limit.unwrap_or(default_limit)).min(lines.len());
    let sliced = &lines[start..end];

    let mut output = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n",
        resolved.display()
    );
    for (i, line) in sliced.iter().enumerate() {
        let line_num = start + i + 1;
        let truncated = if line.chars().count() > MAX_LINE_LENGTH {
            let s: String = line.chars().take(MAX_LINE_LENGTH).collect();
            format!("{}{}", s, MAX_LINE_SUFFIX)
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
) -> Result<String, ToolError> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Cannot read directory {}: {}", path.display(), e),
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
    let default_limit = DEFAULT_LINE_LIMIT;
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

use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use std::sync::{Arc, RwLock};

/// Filesystem sub-action to read file or directory.
pub struct FsReadSubAction {
    sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl FsReadSubAction {
    /// Creates a new `FsReadSubAction` with the shared sandbox reference.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<crate::utils::sandbox::Sandbox>>>>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ToolAction for FsReadSubAction {
    fn tool_name(&self) -> &'static str {
        "filesystem.read"
    }

    fn definition(&self) -> ToolSpec {
        let description = concat!(
            "Read the contents of a file or list a directory. ",
            "When editing text from Read output, preserve exact indentation (tabs/spaces) ",
            "as it appears AFTER the line number prefix."
        );
        ToolSpec {
            name: ToolName::new("filesystem.read"),
            version: ToolVersion::default(),
            display_name: "Read File".to_string(),
            summary: "Read the contents of a file or list a directory.".to_string(),
            description: description.to_string(),
            category: ToolCategory::Filesystem,
            keywords: KeywordSet::primary_only([
                "read",
                "file",
                "open",
                "cat",
                "directory",
                "list",
            ]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "filePath": {
                        "type": "string",
                        "description": "Absolute path to the file or directory"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "1-indexed line number to start reading from"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (default 2000)"
                    }
                },
                "required": ["filePath"]
            }),
            examples: vec![
                ToolExample {
                    description: "Read a text file".to_string(),
                    input: serde_json::json!({"filePath": "/home/user/notes.txt"}),
                    output: Some("file content here".to_string()),
                },
                ToolExample {
                    description: "Read file with line offset and limit".to_string(),
                    input: serde_json::json!({"filePath": "/home/user/large.log", "offset": 1, "limit": 50}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };

        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Failed to parse read arguments: {e}"),
            })?;

        let file_path = args["filePath"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "filePath is required for read".to_string(),
            })?;
        let offset = args["offset"].as_u64().map(|v| v as usize);
        let limit = args["limit"].as_u64().map(|v| v as usize);
        read(Path::new(file_path), offset, limit, sandbox.config()).await
    }
}
