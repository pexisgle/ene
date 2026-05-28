use crate::todo;
use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use std::sync::{Arc, Mutex};

/// Built-in utility tool provider.
///
/// Exposes four tools: `question`, `todo`, `get_current_time`, and `get_system_info`.
/// The `todo` store is session-scoped via a `DashMap`.
pub struct UtilityToolProvider {
    todo_store: Arc<todo::TodoStore>,
    session_id: Arc<Mutex<String>>,
}

impl UtilityToolProvider {
    /// Creates a new `UtilityToolProvider` with an empty todo store.
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(todo::TodoStore::new()),
            session_id: Arc::new(Mutex::new("default".to_string())),
        }
    }

    fn current_session_id(&self) -> String {
        self.session_id.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolProvider for UtilityToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            crate::question::tool_definition(),
            crate::todo::tool_definition(),
            ToolDefinition {
                name: "get_current_time".to_string(),
                description: "Get the current date and time on the user's system.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some(ene_tool_proto::ToolCategory::Utility),
                keywords: vec!["time".to_string(), "date".to_string()],
            },
            ToolDefinition {
                name: "get_system_info".to_string(),
                description: "Get basic information about the user's system.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some(ene_tool_proto::ToolCategory::Utility),
                keywords: vec![
                    "system".to_string(),
                    "os".to_string(),
                    "platform".to_string(),
                ],
            },
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "question" => {
                let args: QuestionArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for question: {e}"),
                    })?;
                crate::question::question(args.questions)
            }
            "todo" => {
                let args: TodoArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for todo: {e}"),
                    })?;
                Ok(crate::todo::update_todos(
                    &self.todo_store,
                    &self.current_session_id(),
                    args.todos,
                ))
            }
            "get_current_time" => {
                let now = chrono::Local::now();
                Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
            }
            "get_system_info" => {
                let os = std::env::consts::OS;
                let arch = std::env::consts::ARCH;
                Ok(format!("OS: {}, Architecture: {}", os, arch))
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, session_id: &str) {
        if let Ok(mut id) = self.session_id.lock() {
            *id = session_id.to_string();
        }
    }
}

#[derive(serde::Deserialize)]
struct QuestionArgs {
    questions: Vec<String>,
}

#[derive(serde::Deserialize)]
struct TodoArgs {
    todos: Vec<todo::TodoItem>,
}
