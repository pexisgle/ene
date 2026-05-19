use super::definition::{ToolDefinition, ToolRegistry};
use super::todo::TodoItem;
use super::undo_manager::UndoManager;
use crate::sandbox::SandboxConfig;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

pub struct OpencodeToolRegistry {
    sandbox: SandboxConfig,
    undo_manager: UndoManager,
    session_id: Arc<Mutex<String>>,
    todo_store: super::todo::TodoStore,
    browser_store: super::browser::BrowserSessionStore,
}

impl OpencodeToolRegistry {
    pub fn new(sandbox: SandboxConfig) -> Self {
        Self {
            sandbox,
            undo_manager: UndoManager::new(),
            session_id: Arc::new(Mutex::new("default".to_string())),
            todo_store: super::todo::TodoStore::new(),
            browser_store: super::browser::BrowserSessionStore::new(),
        }
    }

    pub fn undo_manager(&self) -> &UndoManager {
        &self.undo_manager
    }

    fn current_session_id(&self) -> String {
        self.session_id.lock().unwrap().clone()
    }
}

#[derive(Deserialize)]
struct FilesystemArgs {
    action: String,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Deserialize)]
struct ShellArgs {
    command: String,
    description: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    workdir: Option<String>,
}

#[derive(Deserialize)]
struct BrowserArgs {
    action: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    scroll_x: Option<i32>,
    #[serde(default)]
    scroll_y: Option<i32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    extract: Option<String>,
    #[serde(default)]
    trim: Option<bool>,
}

#[derive(Deserialize)]
struct AppArgs {
    action: String,
    #[serde(default)]
    window_title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    button: Option<String>,
}

#[derive(Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct TodoArgs {
    todos: Vec<TodoItem>,
}

#[derive(Deserialize)]
struct QuestionArgs {
    questions: Vec<String>,
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
            super::filesystem::tool_definition(),
            super::undo_tool::tool_definition(),
            super::shell::tool_definition(),
            super::browser::tool_definition(),
            super::app::tool_definition(),
            super::webfetch::tool_definition(),
            super::websearch::tool_definition(),
            super::todo::tool_definition(),
            super::question::tool_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        match name {
            "filesystem" => {
                let args: FilesystemArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for filesystem: {e}"))?;
                super::filesystem::execute(
                    &args.action,
                    &args.extra,
                    &self.sandbox,
                    &self.undo_manager,
                    &self.current_session_id(),
                )
                .await
            }
            "undo" => super::undo_tool::undo(&self.undo_manager, &self.current_session_id())
                .await
                .map_err(|e| e.to_string()),
            "shell" => {
                let args: ShellArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for shell: {e}"))?;
                super::shell::shell_exec(
                    &args.command,
                    &args.description,
                    args.timeout,
                    args.workdir.as_deref(),
                    &self.sandbox,
                )
                .await
                .map_err(|e| e.to_string())
            }
            "browser" => {
                let args: BrowserArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for browser: {e}"))?;
                super::browser::browser_exec(
                    &self.browser_store,
                    &self.current_session_id(),
                    &args.action,
                    args.url.as_deref(),
                    args.selector.as_deref(),
                    args.text.as_deref(),
                    args.wait_ms,
                    args.scroll_x,
                    args.scroll_y,
                    args.format.as_deref(),
                    args.extract.as_deref(),
                    args.trim,
                )
                .await
                .map_err(|e| e.to_string())
            }
            "app" => {
                let args: AppArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for app: {e}"))?;
                super::app::app_exec(
                    &args.action,
                    args.window_title.as_deref(),
                    args.text.as_deref(),
                    args.key.as_deref(),
                    args.x,
                    args.y,
                    args.button.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())
            }
            "webfetch" => {
                let args: WebFetchArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for webfetch: {e}"))?;
                super::webfetch::webfetch(&args.url, args.format.as_deref(), args.timeout)
                    .await
                    .map_err(|e| e.to_string())
            }
            "websearch" => {
                let args: WebSearchArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for websearch: {e}"))?;
                super::websearch::websearch(&args.query, args.backend.as_deref(), args.limit)
                    .await
                    .map_err(|e| e.to_string())
            }
            "todo" => {
                let args: TodoArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for todo: {e}"))?;
                Ok(super::todo::update_todos(
                    &self.todo_store,
                    &self.current_session_id(),
                    args.todos,
                ))
            }
            "question" => {
                let args: QuestionArgs =
                    serde_json::from_str(arguments).map_err(|e| format!("Invalid arguments for question: {e}"))?;
                super::question::question(args.questions).map_err(|e| e.to_string())
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
