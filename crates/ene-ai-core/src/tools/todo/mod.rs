mod store;

use super::definition::ToolDefinition;
pub use store::{TodoItem, TodoStore};

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
        category: Some(super::ToolCategory::Utility),
        keywords: vec!["todo".to_string(), "task".to_string(), "track".to_string(), "plan".to_string()],
    }
}

pub fn update_todos(store: &TodoStore, session_id: &str, todos: Vec<TodoItem>) -> String {
    let pending = todos.iter().filter(|x| x.status != "completed").count();
    store.update(session_id, todos.clone());
    format!(
        "Updated todo list: {} pending tasks\n{}",
        pending,
        serde_json::to_string_pretty(&todos).unwrap_or_default()
    )
}
