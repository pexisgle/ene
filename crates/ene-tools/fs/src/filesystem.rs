use crate::sandbox::Sandbox;
use crate::undo_manager::UndoManager;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use std::path::Path;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "filesystem".to_string(),
        description: concat!(
            "Unified filesystem and search tool. Performs read, write, edit, delete, glob, grep, and patch operations based on the 'action' parameter. ",
            "Use this for all file access, modification, and search operations. ",
            "When editing text from Read output, preserve exact indentation (tabs/spaces) as it appears AFTER the line number prefix."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "edit", "delete", "glob", "grep", "patch"],
                    "description": "The filesystem operation to perform"
                },
                "filePath": { "type": "string", "description": "Absolute path to the file (for read, write, edit)" },
                "path": { "type": "string", "description": "Absolute path to file or directory (for delete, glob, grep)" },
                "content": { "type": "string", "description": "Content to write (for write)" },
                "oldString": { "type": "string", "description": "Text to replace (for edit)" },
                "newString": { "type": "string", "description": "Replacement text (for edit)" },
                "replaceAll": { "type": "boolean", "description": "Replace all occurrences (for edit, default false)" },
                "recursive": { "type": "boolean", "description": "Delete directories recursively (for delete)" },
                "pattern": { "type": "string", "description": "Glob or regex pattern (for glob/grep)" },
                "include": { "type": "string", "description": "File pattern filter e.g. '*.rs' (for grep)" },
                "patchText": { "type": "string", "description": "Full patch text (for patch)" },
                "offset": { "type": "integer", "description": "Line number to start reading from (1-indexed, for read)" },
                "limit": { "type": "integer", "description": "Maximum lines to read (for read)" }
            },
            "required": ["action"]
        }),
        category: Some(ToolCategory::Filesystem),
        keywords: vec![
            "file".to_string(), "read".to_string(), "write".to_string(),
            "edit".to_string(), "delete".to_string(), "search".to_string(),
            "glob".to_string(), "grep".to_string(), "patch".to_string(),
            "directory".to_string(), "replace".to_string(),
        ],
    }
}

pub async fn execute(
    action: &str,
    args: &serde_json::Value,
    sandbox: &Sandbox,
    undo_manager: &UndoManager,
    session_id: &str,
) -> Result<String, ToolError> {
    match action {
        "read" => {
            let file_path = args["filePath"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "filePath is required for read".to_string(),
                })?;
            let offset = args["offset"].as_u64().map(|v| v as usize);
            let limit = args["limit"].as_u64().map(|v| v as usize);
            super::read::read(Path::new(file_path), offset, limit, sandbox.config()).await
        }
        "write" => {
            let file_path = args["filePath"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "filePath is required for write".to_string(),
                })?;
            let content = args["content"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "content is required for write".to_string(),
                })?;

            sandbox.check_permission(
                crate::permission::DestructiveAction::FileOverwrite,
                file_path,
                "Writing/Overwriting file",
            )?;

            super::write::write(
                Path::new(file_path),
                content,
                sandbox.config(),
                undo_manager,
                session_id,
            )
            .await
        }
        "edit" => {
            let file_path = args["filePath"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "filePath is required for edit".to_string(),
                })?;
            let old_string = args["oldString"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "oldString is required for edit".to_string(),
                })?;
            let new_string = args["newString"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "newString is required for edit".to_string(),
                })?;
            let replace_all = args["replaceAll"].as_bool().unwrap_or(false);

            sandbox.check_permission(
                crate::permission::DestructiveAction::FileOverwrite,
                file_path,
                "Editing file content",
            )?;

            super::edit::edit(
                Path::new(file_path),
                old_string,
                new_string,
                replace_all,
                sandbox.config(),
                undo_manager,
                session_id,
            )
            .await
        }
        "delete" => {
            let path = args["path"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "path is required for delete".to_string(),
                })?;
            let recursive = args["recursive"].as_bool().unwrap_or(false);

            sandbox.check_permission(
                crate::permission::DestructiveAction::FileDelete,
                path,
                "Deleting file or directory",
            )?;

            super::delete::delete(
                Path::new(path),
                recursive,
                sandbox.config(),
                undo_manager,
                session_id,
            )
            .await
        }
        "glob" => {
            let pattern = args["pattern"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "pattern is required for glob".to_string(),
                })?;
            let path = args["path"].as_str();
            super::search::glob_search(pattern, path, sandbox.config()).await
        }
        "grep" => {
            let pattern = args["pattern"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "pattern is required for grep".to_string(),
                })?;
            let path = args["path"].as_str();
            let include = args["include"].as_str();
            super::search::grep_search(pattern, path, include, sandbox.config()).await
        }
        "patch" => {
            let patch_text = args["patchText"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments {
                    message: "patchText is required for patch".to_string(),
                })?;

            sandbox.check_permission(
                crate::permission::DestructiveAction::FileOverwrite,
                "multiple files (patch)",
                "Applying patch to files",
            )?;

            super::patch::apply_patch(patch_text, sandbox.config(), undo_manager, session_id).await
        }
        _ => Err(ToolError::NotFound {
            tool_name: format!("filesystem action: {}", action),
        }),
    }
}
