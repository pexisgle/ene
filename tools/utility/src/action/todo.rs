use ene_tool_common::prelude::*;
use std::sync::Arc;

use crate::db::{TodoDb, TodoError, TodoItem};

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value).map_err(|e| ToolError::Internal {
        message: format!("json serialization failed: {e}"),
    })
}

fn err(e: TodoError) -> ToolError {
    match e {
        TodoError::NotFound(id) => ToolError::InvalidArguments {
            message: format!("todo {id} not found in this session"),
        },
        TodoError::ParentNotFound(id) => ToolError::InvalidArguments {
            message: format!("parent todo {id} not found in this session"),
        },
        TodoError::Cycle { child, parent } => ToolError::InvalidArguments {
            message: format!("cannot reparent todo {child} under its own descendant {parent}"),
        },
        TodoError::InvalidStatus(s) => ToolError::InvalidArguments {
            message: format!("invalid status: {s}"),
        },
        TodoError::InvalidPriority(s) => ToolError::InvalidArguments {
            message: format!("invalid priority: {s}"),
        },
        TodoError::EmptyContent => ToolError::InvalidArguments {
            message: "content must not be empty".to_string(),
        },
        TodoError::DbNotInitialized => ToolError::Internal {
            message: "todo database is not initialized".to_string(),
        },
        TodoError::InvalidPath(s) => ToolError::Internal {
            message: format!("todo db path: {s}"),
        },
        TodoError::Sqlite(e) => ToolError::Internal {
            message: format!("todo db error: {e}"),
        },
    }
}

fn default_db() -> Arc<TodoDb> {
    Arc::new(TodoDb::new())
}

// ───────────────────────── todo_list ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_list",
    summary = "List all todos in the current session.",
    description = "Returns the full todo tree for the current session, including each item's id, content, status, priority, parent relationship, and a summary of active vs total items.",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist"
)]
/// Action to list all todos in the current session.
pub struct TodoListAction {
    #[tool(skip)]
    #[serde(skip, default = "default_db")]
    db: Arc<TodoDb>,
}

impl TodoListAction {
    /// Creates a new `TodoListAction` with the given database.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let items: Vec<TodoItem> = self.db.list().map_err(err)?;
        let count = items.len();
        let active = items
            .iter()
            .filter(|i| i.status != "completed" && i.status != "cancelled")
            .count();
        let body = serde_json::json!({
            "summary": {
                "total": count,
                "active": active,
                "session_id": self.db.current_session_id(),
            },
            "items": items,
        });
        ok_json(&body)
    }
}

// ───────────────────────── todo_add ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_add",
    summary = "Add a new todo.",
    description = "Adds a new todo. Set `parent_id` to make this a sub-task of an existing todo; this lets you break a large task into smaller sub-tasks at any depth. New todos start with status='pending'.",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist"
)]
/// Action to add a new todo.
pub struct TodoAddAction {
    #[tool(skip)]
    #[serde(skip, default = "default_db")]
    db: Arc<TodoDb>,
    /// Brief description of the task.
    content: String,
    /// Priority level.
    #[arg(enum_values = "high, medium, low")]
    priority: String,
    /// Optional id of an existing todo to nest this under. Omit for a
    /// top-level todo.
    #[serde(default)]
    parent_id: Option<i64>,
}

impl TodoAddAction {
    /// Creates a new `TodoAddAction` with the given database.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self {
            db,
            content: String::new(),
            priority: String::new(),
            parent_id: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let item = self
            .db
            .add(self.parent_id, &self.content, &self.priority)
            .map_err(err)?;
        ok_json(&item)
    }
}

// ───────────────────────── todo_update ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_update",
    summary = "Update an existing todo's fields.",
    description = "Updates fields of an existing todo. Any field omitted is left unchanged. To reparent a todo, set parent_id to a new integer id; to detach it (make it a top-level todo), set parent_id to null. Repurposing cannot create a cycle (a todo cannot become a descendant of itself).",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist"
)]
/// Action to update an existing todo's fields.
pub struct TodoUpdateAction {
    #[tool(skip)]
    #[serde(skip, default = "default_db")]
    db: Arc<TodoDb>,
    /// Id of the todo to update.
    id: i64,
    /// New content (omit to keep).
    #[serde(default)]
    content: Option<String>,
    /// New status (omit to keep).
    #[arg(enum_values = "pending, in_progress, completed, cancelled")]
    #[serde(default)]
    status: Option<String>,
    /// New priority (omit to keep).
    #[arg(enum_values = "high, medium, low")]
    #[serde(default)]
    priority: Option<String>,
    /// New parent id (integer) to reparent under, or null to detach to
    /// a top-level todo. Omit to leave unchanged.
    #[serde(default, deserialize_with = "deserialize_optional_parent_id")]
    parent_id: Option<Option<i64>>,
}

fn deserialize_optional_parent_id<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let v = serde_json::Value::deserialize(d)?;
    if v.is_null() {
        Ok(Some(None))
    } else if let Some(n) = v.as_i64() {
        Ok(Some(Some(n)))
    } else {
        Err(D::Error::custom("parent_id must be an integer or null"))
    }
}

impl TodoUpdateAction {
    /// Creates a new `TodoUpdateAction` with the given database.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self {
            db,
            id: 0,
            content: None,
            status: None,
            priority: None,
            parent_id: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let updated = self
            .db
            .update(
                self.id,
                self.content.as_deref(),
                self.status.as_deref(),
                self.priority.as_deref(),
                self.parent_id,
            )
            .map_err(err)?;
        ok_json(&updated)
    }
}

// ───────────────────────── todo_complete ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_complete",
    summary = "Mark a todo and all its sub-tasks as completed.",
    description = "Marks a todo (and all of its descendants) as completed. Use this when a large task and all of its sub-tasks are finished.",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist"
)]
/// Action to mark a todo and all its sub-tasks as completed.
pub struct TodoCompleteAction {
    #[tool(skip)]
    #[serde(skip, default = "default_db")]
    db: Arc<TodoDb>,
    /// Id of the todo to complete (cascades to descendants).
    id: i64,
}

impl TodoCompleteAction {
    /// Creates a new `TodoCompleteAction` with the given database.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db, id: 0 }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let cascaded = self.db.complete(self.id).map_err(err)?;
        ok_json(&serde_json::json!({
            "id": self.id,
            "status": "completed",
            "cascaded": cascaded,
        }))
    }
}

// ───────────────────────── todo_delete ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_delete",
    summary = "Soft-delete a todo by marking it as cancelled.",
    description = "Soft-deletes a todo by setting status='cancelled'. The row is kept for history. Descendants are NOT cascaded — to cancel a whole sub-tree, delete each item individually.",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist"
)]
/// Action to soft-delete a todo by marking it as cancelled.
pub struct TodoDeleteAction {
    #[tool(skip)]
    #[serde(skip, default = "default_db")]
    db: Arc<TodoDb>,
    /// Id of the todo to cancel.
    id: i64,
}

impl TodoDeleteAction {
    /// Creates a new `TodoDeleteAction` with the given database.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db, id: 0 }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let updated = self.db.delete(self.id).map_err(err)?;
        ok_json(&updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fresh() -> (
        Arc<TodoDb>,
        TodoListAction,
        TodoAddAction,
        TodoUpdateAction,
        TodoCompleteAction,
        TodoDeleteAction,
    ) {
        let db = Arc::new(TodoDb::new());
        db.set_db_path(Path::new(":memory:")).unwrap();
        db.set_session_id("sess");
        (
            db.clone(),
            TodoListAction::new(db.clone()),
            TodoAddAction::new(db.clone()),
            TodoUpdateAction::new(db.clone()),
            TodoCompleteAction::new(db.clone()),
            TodoDeleteAction::new(db.clone()),
        )
    }

    #[tokio::test]
    async fn add_then_list() {
        let (db, list, add, _, _, _) = fresh();
        let r = add
            .execute(r#"{"content":"a","priority":"high"}"#)
            .await
            .unwrap();
        let _: TodoItem = serde_json::from_str(&r).unwrap();

        let r = list.execute("{}").await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["summary"]["total"], 1);
        assert_eq!(v["items"][0]["content"], "a");
        let _ = db;
    }

    #[tokio::test]
    async fn update_partial_keeps_other_fields() {
        let (_db, _, add, update, _, _) = fresh();
        let r = add
            .execute(r#"{"content":"x","priority":"low"}"#)
            .await
            .unwrap();
        let item: TodoItem = serde_json::from_str(&r).unwrap();

        let r = update
            .execute(&format!(r#"{{"id":{},"status":"in_progress"}}"#, item.id))
            .await
            .unwrap();
        let updated: TodoItem = serde_json::from_str(&r).unwrap();
        assert_eq!(updated.status, "in_progress");
        assert_eq!(updated.content, "x");
        assert_eq!(updated.priority, "low");
    }

    #[tokio::test]
    async fn update_parent_id_null_detaches() {
        let (_db, _, add, update, _, _) = fresh();
        let p = add
            .execute(r#"{"content":"p","priority":"high"}"#)
            .await
            .unwrap();
        let p: TodoItem = serde_json::from_str(&p).unwrap();
        let c = add
            .execute(&format!(
                r#"{{"content":"c","priority":"low","parent_id":{}}}"#,
                p.id
            ))
            .await
            .unwrap();
        let c: TodoItem = serde_json::from_str(&c).unwrap();

        let r = update
            .execute(&format!(r#"{{"id":{},"parent_id":null}}"#, c.id))
            .await
            .unwrap();
        let c2: TodoItem = serde_json::from_str(&r).unwrap();
        assert!(c2.parent_id.is_none());
    }

    #[tokio::test]
    async fn complete_cascades() {
        let (_db, _, add, _, complete, _) = fresh();
        let p = add
            .execute(r#"{"content":"p","priority":"high"}"#)
            .await
            .unwrap();
        let p: TodoItem = serde_json::from_str(&p).unwrap();
        add.execute(&format!(
            r#"{{"content":"c","priority":"low","parent_id":{}}}"#,
            p.id
        ))
        .await
        .unwrap();

        let r = complete
            .execute(&format!(r#"{{"id":{}}}"#, p.id))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["status"], "completed");
        let cascaded = v["cascaded"].as_array().unwrap();
        assert!(cascaded.len() >= 2);
    }

    #[tokio::test]
    async fn delete_soft_cancels() {
        let (_db, _, add, _, _, delete) = fresh();
        let i = add
            .execute(r#"{"content":"x","priority":"low"}"#)
            .await
            .unwrap();
        let i: TodoItem = serde_json::from_str(&i).unwrap();
        let r = delete
            .execute(&format!(r#"{{"id":{}}}"#, i.id))
            .await
            .unwrap();
        let v: TodoItem = serde_json::from_str(&r).unwrap();
        assert_eq!(v.status, "cancelled");
    }

    #[tokio::test]
    async fn invalid_priority_rejected() {
        let (_db, _, add, _, _, _) = fresh();
        let err = add
            .execute(r#"{"content":"x","priority":"wrong"}"#)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }
}
