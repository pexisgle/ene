use super::definition::{ToolDefinition, ToolRegistry};
use crate::sandbox::SandboxConfig;
use super::undo_manager::UndoManager;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// OpenCode 準拠のツールレジストリ
/// read, write, edit, glob, grep, shell, delete, undo, patch, todo, webfetch, question
pub struct OpencodeToolRegistry {
    sandbox: SandboxConfig,
    undo_manager: UndoManager,
    session_id: Arc<Mutex<String>>,
    todo_store: super::todo::TodoStore,
}

impl OpencodeToolRegistry {
    pub fn new(sandbox: SandboxConfig) -> Self {
        Self {
            sandbox,
            undo_manager: UndoManager::new(),
            session_id: Arc::new(Mutex::new("default".to_string())),
            todo_store: super::todo::TodoStore::new(),
        }
    }

    pub fn undo_manager(&self) -> &UndoManager {
        &self.undo_manager
    }

    fn current_session_id(&self) -> String {
        self.session_id.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolRegistry for OpencodeToolRegistry {
    fn set_session_id(&self, session_id: &str) {
        if let Ok(mut id) = self.session_id.lock() {
            *id = session_id.to_string();
        }
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            super::read::tool_definition(),
            super::write::tool_definition(),
            super::edit::tool_definition(),
            super::search::glob_tool_definition(),
            super::search::grep_tool_definition(),
            super::shell::tool_definition(),
            super::delete::tool_definition(),
            super::undo_tool::tool_definition(),
            super::patch::tool_definition(),
            super::todo::tool_definition(),
            super::webfetch::tool_definition(),
            super::question::tool_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| format!("Invalid JSON arguments: {e}"))?;

        match name {
            "read" => {
                let file_path = args["filePath"].as_str().ok_or("filePath is required")?;
                let offset = args["offset"].as_u64().map(|v| v as usize);
                let limit = args["limit"].as_u64().map(|v| v as usize);
                super::read::read(std::path::Path::new(file_path), offset, limit, &self.sandbox)
                    .await
                    .map_err(|e| e.to_string())
            }
            "write" => {
                let file_path = args["filePath"].as_str().ok_or("filePath is required")?;
                let content = args["content"].as_str().ok_or("content is required")?;
                super::write::write(
                    std::path::Path::new(file_path),
                    content,
                    &self.sandbox,
                    &self.undo_manager,
                    &self.current_session_id(),
                )
                .await
                .map_err(|e| e.to_string())
            }
            "edit" => {
                let file_path = args["filePath"].as_str().ok_or("filePath is required")?;
                let old_string = args["oldString"].as_str().ok_or("oldString is required")?;
                let new_string = args["newString"].as_str().ok_or("newString is required")?;
                let replace_all = args["replaceAll"].as_bool().unwrap_or(false);
                super::edit::edit(
                    std::path::Path::new(file_path),
                    old_string,
                    new_string,
                    replace_all,
                    &self.sandbox,
                    &self.undo_manager,
                    &self.current_session_id(),
                )
                .await
                .map_err(|e| e.to_string())
            }
            "glob" => {
                let pattern = args["pattern"].as_str().ok_or("pattern is required")?;
                let path = args["path"].as_str();
                super::search::glob_search(pattern, path, &self.sandbox)
                    .await
                    .map_err(|e| e.to_string())
            }
            "grep" => {
                let pattern = args["pattern"].as_str().ok_or("pattern is required")?;
                let path = args["path"].as_str();
                let include = args["include"].as_str();
                super::search::grep_search(pattern, path, include, &self.sandbox)
                    .await
                    .map_err(|e| e.to_string())
            }
            "shell" => {
                let command = args["command"].as_str().ok_or("command is required")?;
                let description = args["description"].as_str().ok_or("description is required")?;
                let timeout = args["timeout"].as_u64();
                let workdir = args["workdir"].as_str();
                super::shell::shell_exec(command, description, timeout, workdir, &self.sandbox)
                    .await
                    .map_err(|e| e.to_string())
            }
            "delete" => {
                let path = args["path"].as_str().ok_or("path is required")?;
                let recursive = args["recursive"].as_bool().unwrap_or(false);
                super::delete::delete(
                    std::path::Path::new(path),
                    recursive,
                    &self.sandbox,
                    &self.undo_manager,
                    &self.current_session_id(),
                )
                .await
                .map_err(|e| e.to_string())
            }
            "undo" => {
                super::undo_tool::undo(&self.undo_manager, &self.current_session_id())
                    .await
                    .map_err(|e| e.to_string())
            }
            "patch" => {
                let patch_text = args["patchText"].as_str().ok_or("patchText is required")?;
                super::patch::apply_patch(
                    patch_text,
                    &self.sandbox,
                    &self.undo_manager,
                    &self.current_session_id(),
                )
                .await
                .map_err(|e| e.to_string())
            }
            "todo" => {
                let todos = args["todos"].as_array().ok_or("todos is required")?;
                let items: Vec<super::todo::TodoItem> = todos.iter()
                    .map(|t| super::todo::TodoItem {
                        content: t["content"].as_str().unwrap_or("").to_string(),
                        status: t["status"].as_str().unwrap_or("pending").to_string(),
                        priority: t["priority"].as_str().unwrap_or("medium").to_string(),
                    })
                    .collect();
                Ok(super::todo::update_todos(&self.todo_store, &self.current_session_id(), items))
            }
            "webfetch" => {
                let url = args["url"].as_str().ok_or("url is required")?;
                let format = args["format"].as_str();
                let timeout = args["timeout"].as_u64();
                super::webfetch::webfetch(url, format, timeout)
                    .await
                    .map_err(|e| e.to_string())
            }
            "question" => {
                let questions = args["questions"].as_array().ok_or("questions is required")?;
                let qs: Vec<String> = questions.iter()
                    .filter_map(|q| q.as_str().map(|s| s.to_string()))
                    .collect();
                super::question::question(qs)
                    .map_err(|e| e.to_string())
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
