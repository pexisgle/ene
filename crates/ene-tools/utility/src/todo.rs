use dashmap::DashMap;
use ene_tool_proto::{ToolCategory, ToolDefinition};
use serde::{Deserialize, Serialize};

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

/// Returns the `ToolDefinition` for the todo tool.
pub fn tool_definition() -> ToolDefinition {
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

/// Updates the todo list for a session and returns a formatted summary.
pub fn update_todos(store: &TodoStore, session_id: &str, todos: Vec<TodoItem>) -> String {
    let pending = todos.iter().filter(|x| x.status != "completed").count();
    store.update(session_id, todos.clone());
    format!(
        "Updated todo list: {} pending tasks\n{}",
        pending,
        serde_json::to_string_pretty(&todos).unwrap_or_default()
    )
}
