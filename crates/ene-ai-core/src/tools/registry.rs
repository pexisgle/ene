use super::definition::{ToolDefinition, ToolRegistry};
use crate::sandbox::SandboxConfig;
use super::undo_manager::UndoManager;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// OpenCode 準拠のツールレジストリ（Cowork Agent 拡張版）
/// 5つのツールカテゴリを統合:
/// 1. filesystem_tools: read, write, edit, glob, grep, delete, undo, patch
/// 2. shell_tools: shell
/// 3. browser_tools: browser
/// 4. app_tools: app
/// 5. websearch_tools: webfetch, websearch
/// 6. utility_tools: todo, question, screenshot
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
            // filesystem_tools
            super::read::tool_definition(),
            super::write::tool_definition(),
            super::edit::tool_definition(),
            super::search::glob_tool_definition(),
            super::search::grep_tool_definition(),
            super::delete::tool_definition(),
            super::undo_tool::tool_definition(),
            super::patch::tool_definition(),
            // shell_tools
            super::shell::tool_definition(),
            // browser_tools
            super::browser::tool_definition(),
            // app_tools
            super::app::tool_definition(),
            // websearch_tools
            super::webfetch::tool_definition(),
            super::websearch::tool_definition(),
            // utility_tools
            super::todo::tool_definition(),
            super::question::tool_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        let args: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| format!("Invalid JSON arguments: {e}"))?;

        match name {
            // filesystem_tools
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
            // shell_tools
            "shell" => {
                let command = args["command"].as_str().ok_or("command is required")?;
                let description = args["description"].as_str().ok_or("description is required")?;
                let timeout = args["timeout"].as_u64();
                let workdir = args["workdir"].as_str();
                super::shell::shell_exec(command, description, timeout, workdir, &self.sandbox)
                    .await
                    .map_err(|e| e.to_string())
            }
            // browser_tools
            "browser" => {
                let action = args["action"].as_str().ok_or("action is required")?;
                let url = args["url"].as_str();
                let selector = args["selector"].as_str();
                let text = args["text"].as_str();
                let wait_ms = args["wait_ms"].as_u64();
                let scroll_x = args["scroll_x"].as_i64().map(|v| v as i32);
                let scroll_y = args["scroll_y"].as_i64().map(|v| v as i32);
                super::browser::browser_exec(action, url, selector, text, wait_ms, scroll_x, scroll_y)
                    .await
                    .map_err(|e| e.to_string())
            }
            // app_tools
            "app" => {
                let action = args["action"].as_str().ok_or("action is required")?;
                let window_title = args["window_title"].as_str();
                let text = args["text"].as_str();
                let key = args["key"].as_str();
                let x = args["x"].as_i64().map(|v| v as i32);
                let y = args["y"].as_i64().map(|v| v as i32);
                let button = args["button"].as_str();
                super::app::app_exec(action, window_title, text, key, x, y, button)
                    .await
                    .map_err(|e| e.to_string())
            }
            // websearch_tools
            "webfetch" => {
                let url = args["url"].as_str().ok_or("url is required")?;
                let format = args["format"].as_str();
                let timeout = args["timeout"].as_u64();
                super::webfetch::webfetch(url, format, timeout)
                    .await
                    .map_err(|e| e.to_string())
            }
            "websearch" => {
                let query = args["query"].as_str().ok_or("query is required")?;
                let backend = args["backend"].as_str();
                let limit = args["limit"].as_u64().map(|v| v as usize);
                super::websearch::websearch(query, backend, limit)
                    .await
                    .map_err(|e| e.to_string())
            }
            // utility_tools
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
