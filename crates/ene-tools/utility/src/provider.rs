use crate::action;
use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use std::sync::{Arc, Mutex};

/// Built-in utility tool provider.
///
/// Exposes four tools: `question`, `todo`, `get_current_time`, and `get_system_info`.
/// The `todo` store is session-scoped via a `DashMap`.
pub struct UtilityToolProvider {
    todo_store: Arc<action::TodoStore>,
    session_id: Arc<Mutex<String>>,
}

impl UtilityToolProvider {
    /// Creates a new `UtilityToolProvider` with an empty todo store.
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(action::TodoStore::new()),
            session_id: Arc::new(Mutex::new("default".to_string())),
        }
    }

    fn current_session_id(&self) -> String {
        self.session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl ToolProvider for UtilityToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            action::question_definition(),
            action::todo_definition(),
            action::time_definition(),
            action::system_info_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "question" => {
                let args: QuestionArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for question: {e}"),
                    })?;
                action::question(args.questions)
            }
            "todo" => {
                let args: TodoArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for todo: {e}"),
                    })?;
                Ok(action::update_todos(
                    &self.todo_store,
                    &self.current_session_id(),
                    args.todos,
                ))
            }
            "get_current_time" => action::get_current_time(),
            "get_system_info" => action::get_system_info(),
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
    todos: Vec<action::TodoItem>,
}
