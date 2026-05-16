use super::definition::ToolDefinition;
use super::undo_manager::{UndoEntry, UndoManager};
use crate::error::AiCoreError;
use crate::sandbox::SandboxConfig;
use std::path::Path;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "patch".to_string(),
        description: concat!(
            "Applies a patch to multiple files at once. ",
            "The patch text describes all changes to be made. ",
            "Use this for complex multi-file changes or when you need to add/update/delete multiple files in one operation."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "patchText": { "type": "string", "description": "The full patch text that describes all changes to be made. Format: *** Begin Patch\n*** Update File: path\n```\nold content\n```\n```\nnew content\n```\n*** Add File: path\n```\ncontent\n```\n*** Delete File: path\n*** End Patch" }
            },
            "required": ["patchText"]
        }),
    }
}

/// パッチテキストを解析して複数ファイルに適用
/// パッチフォーマット:
/// *** Begin Patch
/// *** Update File: path/to/file.txt
/// ```text
/// old content
/// ```
/// ```text
/// new content
/// ```
/// *** Add File: path/to/new.txt
/// ```text
/// content
/// ```
/// *** Delete File: path/to/old.txt
/// *** End Patch
pub async fn apply_patch(
    patch_text: &str,
    sandbox: &SandboxConfig,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, AiCoreError> {
    let normalized = patch_text.replace("\r\n", "\n").replace("\r", "\n");
    let trimmed = normalized.trim();

    if trimmed == "*** Begin Patch\n*** End Patch" || trimmed == "*** Begin Patch\n\n*** End Patch"
    {
        return Err(AiCoreError::ToolExecutionError(
            "Patch rejected: empty patch".to_string(),
        ));
    }

    if !trimmed.starts_with("*** Begin Patch") || !trimmed.ends_with("*** End Patch") {
        return Err(AiCoreError::ToolExecutionError(
            "Patch must start with '*** Begin Patch' and end with '*** End Patch'".to_string(),
        ));
    }

    // パッチブロックをパース
    let mut operations = Vec::new();
    let lines: Vec<&str> = trimmed.lines().collect();
    let mut i = 1; // Skip "*** Begin Patch"

    while i < lines.len() - 1 {
        // Skip "*** End Patch"
        let line = lines[i].trim();

        if line.starts_with("*** Update File:") {
            let file_path = line["*** Update File:".len()..].trim();
            let start = i + 1;
            let (old_content, new_content, consumed) = parse_update_block(&lines, start)?;
            operations.push(PatchOperation::Update {
                path: file_path.to_string(),
                old_content,
                new_content,
            });
            i = start + consumed + 1;
        } else if line.starts_with("*** Add File:") {
            let file_path = line["*** Add File:".len()..].trim();
            let start = i + 1;
            let (content, consumed) = parse_code_block(&lines, start)?;
            operations.push(PatchOperation::Add {
                path: file_path.to_string(),
                content,
            });
            i = start + consumed + 1;
        } else if line.starts_with("*** Delete File:") {
            let file_path = line["*** Delete File:".len()..].trim();
            operations.push(PatchOperation::Delete {
                path: file_path.to_string(),
            });
            i += 1;
        } else if line.is_empty() {
            i += 1;
        } else {
            return Err(AiCoreError::ToolExecutionError(format!(
                "Unknown patch directive at line {}: {}",
                i + 1,
                line
            )));
        }
    }

    if operations.is_empty() {
        return Err(AiCoreError::ToolExecutionError(
            "Patch contains no operations".to_string(),
        ));
    }

    // 検証フェーズ - すべての操作が有効か確認
    let mut validated = Vec::new();
    for op in &operations {
        match op {
            PatchOperation::Update {
                path, old_content, ..
            } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                if !resolved.exists() {
                    return Err(AiCoreError::FileNotFound(format!(
                        "Patch verification failed: File to update does not exist: {}",
                        path
                    )));
                }
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    AiCoreError::ToolExecutionError(format!(
                        "Patch verification failed: Cannot read {}: {}",
                        path, e
                    ))
                })?;
                if !current.contains(old_content) {
                    return Err(AiCoreError::ToolExecutionError(format!(
                        "Patch verification failed: old_content not found in {}. The file may have changed.",
                        path
                    )));
                }
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Add { path, .. } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                validated.push((resolved, op.clone()));
            }
            PatchOperation::Delete { path } => {
                let resolved = sandbox.resolve_and_check(Path::new(path), true)?;
                if !resolved.exists() {
                    return Err(AiCoreError::FileNotFound(format!(
                        "Patch verification failed: File to delete does not exist: {}",
                        path
                    )));
                }
                validated.push((resolved, op.clone()));
            }
        }
    }

    // 適用フェーズ - Undoバックアップを取得してから実行
    let mut undo_ops = Vec::new();
    let mut summary = Vec::new();

    for (resolved, op) in validated {
        match op {
            PatchOperation::Update {
                old_content,
                new_content,
                ..
            } => {
                let original = tokio::fs::read(&resolved).await.ok();
                let current = tokio::fs::read_to_string(&resolved).await.map_err(|e| {
                    AiCoreError::ToolExecutionError(format!(
                        "Cannot read {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                // Simple replace (must be unique match)
                let first = current.find(&old_content);
                let last = current.rfind(&old_content);
                if first != last {
                    return Err(AiCoreError::ToolExecutionError(format!(
                        "Patch failed: old_content found multiple times in {}. Patch cannot be applied safely.",
                        resolved.display()
                    )));
                }
                let pos = first.ok_or_else(|| {
                    AiCoreError::ToolExecutionError(format!(
                        "Patch failed: old_content not found in {}",
                        resolved.display()
                    ))
                })?;

                let updated =
                    current[..pos].to_string() + &new_content + &current[pos + old_content.len()..];
                tokio::fs::write(&resolved, updated).await.map_err(|e| {
                    AiCoreError::ToolExecutionError(format!(
                        "Cannot write {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                undo_ops.push(UndoEntry::restore_file(resolved.clone(), original));
                summary.push(format!("M {}", resolved.display()));
            }
            PatchOperation::Add { content, .. } => {
                if let Some(parent) = resolved.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        AiCoreError::ToolExecutionError(format!("Cannot create directory: {e}"))
                    })?;
                }
                tokio::fs::write(&resolved, &content).await.map_err(|e| {
                    AiCoreError::ToolExecutionError(format!(
                        "Cannot write {}: {}",
                        resolved.display(),
                        e
                    ))
                })?;

                undo_ops.push(UndoEntry::delete_created_file(resolved.clone()));
                summary.push(format!("A {}", resolved.display()));
            }
            PatchOperation::Delete { .. } => {
                let original = if resolved.is_file() {
                    tokio::fs::read(&resolved).await.ok()
                } else {
                    None
                };

                if resolved.is_dir() {
                    tokio::fs::remove_dir_all(&resolved).await.map_err(|e| {
                        AiCoreError::ToolExecutionError(format!(
                            "Cannot delete {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;
                } else {
                    tokio::fs::remove_file(&resolved).await.map_err(|e| {
                        AiCoreError::ToolExecutionError(format!(
                            "Cannot delete {}: {}",
                            resolved.display(),
                            e
                        ))
                    })?;
                }

                undo_ops.push(UndoEntry::restore_file(resolved.clone(), original));
                summary.push(format!("D {}", resolved.display()));
            }
        }
    }

    // Undoエントリを1つにまとめる
    undo_manager.push(session_id, UndoEntry::new("patch", undo_ops));

    Ok(format!(
        "Patch applied successfully.\n{}",
        summary.join("\n")
    ))
}

#[derive(Clone)]
enum PatchOperation {
    Update {
        path: String,
        old_content: String,
        new_content: String,
    },
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
}

fn parse_code_block(lines: &[&str], start: usize) -> Result<(String, usize), AiCoreError> {
    if start >= lines.len() || !lines[start].trim().starts_with("```") {
        return Err(AiCoreError::ToolExecutionError(format!(
            "Expected code block starting with ``` at line {}",
            start + 1
        )));
    }

    let mut content = Vec::new();
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "```" {
            return Ok((content.join("\n"), i - start));
        }
        content.push(line);
        i += 1;
    }

    Err(AiCoreError::ToolExecutionError(
        "Unclosed code block in patch".to_string(),
    ))
}

fn parse_update_block(
    lines: &[&str],
    start: usize,
) -> Result<(String, String, usize), AiCoreError> {
    // Parse old content block
    let (old_content, consumed1) = parse_code_block(lines, start)?;
    let next = start + consumed1 + 1;

    // Parse new content block
    if next >= lines.len() || !lines[next].trim().starts_with("```") {
        return Err(AiCoreError::ToolExecutionError(format!(
            "Expected second code block for new content at line {}",
            next + 1
        )));
    }
    let (new_content, consumed2) = parse_code_block(lines, next)?;

    Ok((old_content, new_content, consumed1 + 1 + consumed2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_code_block_simple() {
        let lines = vec![
            "```",
            "content line 1",
            "content line 2",
            "```",
            "next line",
        ];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "content line 1\ncontent line 2");
        assert_eq!(consumed, 3);
    }

    #[test]
    fn test_parse_code_block_with_lang() {
        let lines = vec!["```rust", "fn main() {}", "```", "next"];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "fn main() {}");
        assert_eq!(consumed, 2);
    }

    #[test]
    fn test_parse_code_block_empty() {
        let lines = vec!["```", "```", "next"];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "");
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_parse_code_block_missing_start() {
        let lines = vec!["not a code block", "next"];
        let result = parse_code_block(&lines, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_code_block_unclosed() {
        let lines = vec!["```", "content", "more content"];
        let result = parse_code_block(&lines, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_update_block() {
        let lines = vec!["```", "old content", "```", "```", "new content", "```"];
        let (old, new, consumed) = parse_update_block(&lines, 0).unwrap();
        assert_eq!(old, "old content");
        assert_eq!(new, "new content");
        assert_eq!(consumed, 5);
    }

    #[test]
    fn test_parse_update_block_missing_second_block() {
        let lines = vec!["```", "old content", "```", "not a code block"];
        let result = parse_update_block(&lines, 0);
        assert!(result.is_err());
    }
}
