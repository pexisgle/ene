use dashmap::DashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Default)]
pub struct TodoStore {
    lists: DashMap<String, Vec<TodoItem>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            lists: DashMap::new(),
        }
    }

    pub fn update(&self, session_id: &str, todos: Vec<TodoItem>) {
        self.lists.insert(session_id.to_string(), todos);
    }

    pub fn get(&self, session_id: &str) -> Option<Vec<TodoItem>> {
        self.lists.get(session_id).map(|v| v.clone())
    }

    pub fn clear(&self, session_id: &str) {
        self.lists.remove(session_id);
    }

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
