use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, ToolProvider};
use ene_tools_common::ToolAction;
use std::sync::{Arc, RwLock};

use crate::utils::sandbox::Sandbox;

/// Returns the `ToolDefinition` for the filesystem tool.
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

/// Monolithic Filesystem executor containing registered sub-actions.
pub struct FilesystemExecutor {
    sub_actions: std::collections::HashMap<String, Box<dyn ToolAction>>,
}

impl FilesystemExecutor {
    /// Creates a new `FilesystemExecutor` and registers all filesystem sub-actions from their modules.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>) -> Self {
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(crate::action::read::FsReadSubAction::new(sandbox.clone())),
            Box::new(crate::action::write::FsWriteSubAction::new(sandbox.clone())),
            Box::new(crate::action::edit::FsEditSubAction::new(sandbox.clone())),
            Box::new(crate::action::delete::FsDeleteSubAction::new(
                sandbox.clone(),
            )),
            Box::new(crate::action::search::search_glob::FsGlobSubAction::new(
                sandbox.clone(),
            )),
            Box::new(crate::action::search::search_grep::FsGrepSubAction::new(
                sandbox.clone(),
            )),
            Box::new(crate::action::patch::FsPatchSubAction::new(sandbox.clone())),
        ];

        let mut sub_actions = std::collections::HashMap::new();
        for act in actions {
            sub_actions.insert(act.definition().name.clone(), act);
        }

        Self { sub_actions }
    }

    /// Dispatches and executes the filesystem sub-action.
    pub async fn execute(&self, action: &str, arguments: &str) -> Result<String, ToolError> {
        if let Some(sub_act) = self.sub_actions.get(action) {
            sub_act.execute(arguments).await
        } else {
            Err(ToolError::NotFound {
                tool_name: format!("filesystem action: {}", action),
            })
        }
    }
}

/// Filesystem tool action.
pub struct FilesystemAction {
    sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>,
    executor: FilesystemExecutor,
}

impl FilesystemAction {
    /// Creates a new `FilesystemAction` with sandbox and executor.
    pub fn new(sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>) -> Self {
        Self {
            sandbox: sandbox.clone(),
            executor: FilesystemExecutor::new(sandbox),
        }
    }
}

#[async_trait]
impl ToolAction for FilesystemAction {
    fn definition(&self) -> ToolDefinition {
        tool_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Failed to parse filesystem arguments: {e}"),
            })?;
        let action = args["action"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "Missing 'action' field in filesystem arguments".to_string(),
            })?;
        self.executor.execute(action, arguments).await
    }
}

/// Filesystem tool provider backed by diesel undo manager and secure sandbox.
pub struct FsToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>,
}

impl FsToolProvider {
    /// Creates a new `FsToolProvider` and registers all filesystem actions.
    pub fn new() -> Self {
        let sandbox = Arc::new(RwLock::new(None));
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(FilesystemAction::new(sandbox.clone())),
            Box::new(crate::action::shell::ShellAction::new(sandbox.clone())),
            Box::new(crate::action::undo::UndoAction::new(sandbox.clone())),
        ];
        Self { actions, sandbox }
    }
}

impl Default for FsToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for FsToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.definition().name == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, session_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.set_session_id(session_id);
        }
    }

    fn set_sandbox(&self, data: &ene_tool_proto::SandboxConfigData) {
        let new_sandbox = Arc::new(Sandbox::new(data.clone().into()));
        let mut guard = self.sandbox.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(new_sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.approve_request(request_id);
        }
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = guard.as_ref() {
            s.allow_pattern(action, target_pattern);
        }
    }
}
