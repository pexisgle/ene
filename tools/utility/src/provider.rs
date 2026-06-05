use crate::action;
use crate::db::TodoDb;
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

/// Built-in utility tool provider.
///
/// Exposes eight tools: `question`, `todo_list`, `todo_add`, `todo_update`,
/// `todo_complete`, `todo_delete`, `get_current_time`, and `get_system_info`.
/// The todo tools share a single [`TodoDb`] for SQLite-backed, session-scoped
/// persistence and parent/child hierarchy.
pub struct UtilityToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    db: Arc<TodoDb>,
}

impl UtilityToolProvider {
    /// Creates a new `UtilityToolProvider` and registers all utility actions.
    pub fn new() -> Self {
        let db = Arc::new(TodoDb::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::AskQuestionAction::default()),
            Box::new(action::TodoListAction::new(db.clone())),
            Box::new(action::TodoAddAction::new(db.clone())),
            Box::new(action::TodoUpdateAction::new(db.clone())),
            Box::new(action::TodoCompleteAction::new(db.clone())),
            Box::new(action::TodoDeleteAction::new(db.clone())),
            Box::new(action::GetCurrentTimeAction::default()),
            Box::new(action::GetSystemInfoAction::default()),
        ];
        Self { actions, db }
    }
}

impl Default for UtilityToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for UtilityToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.actions.iter().map(|a| a.definition()).collect()
    }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, session_id: &str) {
        self.db.set_session_id(session_id);
    }

    fn set_config(&self, config: &serde_json::Value) {
        if let Some(path) = config.get("db_path").and_then(|v| v.as_str())
            && !path.trim().is_empty()
            && let Err(e) = self.db.set_db_path(std::path::Path::new(path))
        {
            tracing::error!("[ene-tool-utility] todo db open failed: {e}");
        }
    }
}
