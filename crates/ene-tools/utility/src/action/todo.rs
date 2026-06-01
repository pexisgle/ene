use async_trait::async_trait;
use dashmap::DashMap;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// A single todo item with content, status, and priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Brief description of the task.
    pub content: String,
    /// Current status: `pending`, `in_progress`, `completed`, or `cancelled`.
    pub status: String,
    /// Priority level: `high`, `medium`, or `low`.
    pub priority: String,
}

/// Session-scoped todo list store backed by a `DashMap`.
#[derive(Default)]
pub struct TodoStore {
    lists: DashMap<String, Vec<TodoItem>>,
}

impl TodoStore {
    /// Creates a new empty `TodoStore`.
    pub fn new() -> Self {
        Self {
            lists: DashMap::new(),
        }
    }

    /// Replaces the todo list for the given session.
    pub fn update(&self, session_id: &str, todos: Vec<TodoItem>) {
        self.lists.insert(session_id.to_string(), todos);
    }

    /// Returns the todo list for the given session, if any.
    pub fn get(&self, session_id: &str) -> Option<Vec<TodoItem>> {
        self.lists.get(session_id).map(|v| v.clone())
    }

    /// Removes the todo list for the given session.
    pub fn clear(&self, session_id: &str) {
        self.lists.remove(session_id);
    }

    /// Formats the todo list for a session into a human-readable string.
    pub fn format_list(&self, session_id: &str) -> String {
        match self.get(session_id) {
            Some(items) => {
                if items.is_empty() {
                    return "No todos.".to_string();
                }
                let mut output = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    let status_icon = match item.status.as_str() {
                        "completed" => "[x]",
                        "cancelled" => "[-]",
                        "in_progress" => "[~]",
                        _ => "[ ]",
                    };
                    output.push(format!(
                        "{} {} {} (priority: {})",
                        status_icon,
                        item.content,
                        i + 1,
                        item.priority
                    ));
                }
                output.join("\n")
            }
            None => "No todos for this session.".to_string(),
        }
    }
}

#[derive(Deserialize)]
struct TodoArgs {
    todos: Vec<TodoItem>,
}

/// Action to update the todo list for tracking tasks.
pub struct UpdateTodo {
    store: Arc<TodoStore>,
    session_id: Arc<Mutex<String>>,
}

impl UpdateTodo {
    /// Creates a new `UpdateTodo` action.
    pub fn new(store: Arc<TodoStore>, session_id: Arc<Mutex<String>>) -> Self {
        Self { store, session_id }
    }

    fn current_session_id(&self) -> String {
        self.session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl ToolAction for UpdateTodo {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo".to_string(),
            description: concat!(
                "Updates the todo list for tracking tasks. ",
                "Provide the complete updated todo list. Each item has content, status (pending/in_progress/completed/cancelled), and priority (high/medium/low)."
            ).to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The updated todo list",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string", "description": "Brief description of the task" },
                                "status": { "type": "string", "description": "Current status: pending, in_progress, completed, cancelled" },
                                "priority": { "type": "string", "description": "Priority level: high, medium, low" }
                            },
                            "required": ["content", "status", "priority"]
                        }
                    }
                },
                "required": ["todos"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec!["todo".to_string(), "task".to_string(), "track".to_string(), "plan".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: TodoArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for todo: {e}"),
            })?;

        let session_id = self.current_session_id();
        let pending = args
            .todos
            .iter()
            .filter(|x| x.status != "completed")
            .count();
        self.store.update(&session_id, args.todos.clone());

        Ok(format!(
            "Updated todo list: {} pending tasks\n{}",
            pending,
            serde_json::to_string_pretty(&args.todos).unwrap_or_default()
        ))
    }
}
