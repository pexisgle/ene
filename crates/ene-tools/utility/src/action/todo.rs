use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use std::sync::Arc;

use crate::db::{TodoDb, TodoError, TodoItem};

const KEYWORDS_BASE: &[&str] = &["todo", "task", "track", "plan", "checklist"];

fn category() -> Option<ToolCategory> {
    Some(ToolCategory::Utility)
}

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

fn parse<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, ToolError> {
    serde_json::from_str(s).map_err(|e| ToolError::InvalidArguments {
        message: format!("invalid arguments: {e}"),
    })
}

// ───────────────────────── todo_list ─────────────────────────

/// Returns the current session's todo tree.
pub struct TodoList {
    db: Arc<TodoDb>,
}

impl TodoList {
    /// Creates a new `TodoList` action backed by the given `TodoDb`.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolAction for TodoList {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_list".to_string(),
            description: concat!(
                "Returns the current todo list for this session as a flat array. ",
                "Each item has id, parent_id (null for top-level todos), content, ",
                "status (pending|in_progress|completed|cancelled), priority (high|medium|low), ",
                "and timestamps. Items are ordered as (parent_id ASC, id ASC) so children ",
                "appear immediately after their parents. Use parent_id to reconstruct the tree."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: category(),
            keywords: KEYWORDS_BASE.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
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

#[derive(Deserialize)]
struct AddArgs {
    content: String,
    priority: String,
    #[serde(default)]
    parent_id: Option<i64>,
}

/// Adds a new todo. Optionally nests it under an existing todo via `parent_id`.
pub struct TodoAdd {
    db: Arc<TodoDb>,
}

impl TodoAdd {
    /// Creates a new `TodoAdd` action backed by the given `TodoDb`.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolAction for TodoAdd {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_add".to_string(),
            description: concat!(
                "Adds a new todo. Set `parent_id` to make this a sub-task of an existing todo; ",
                "this lets you break a large task into smaller sub-tasks at any depth. ",
                "New todos start with status='pending'."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Brief description of the task."
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["high", "medium", "low"],
                        "description": "Priority level."
                    },
                    "parent_id": {
                        "type": "integer",
                        "description": "Optional id of an existing todo to nest this under. Omit for a top-level todo."
                    }
                },
                "required": ["content", "priority"]
            }),
            category: category(),
            keywords: KEYWORDS_BASE.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: AddArgs = parse(arguments)?;
        let item = self
            .db
            .add(args.parent_id, &args.content, &args.priority)
            .map_err(err)?;
        ok_json(&item)
    }
}

// ───────────────────────── todo_update ─────────────────────────

#[derive(Deserialize)]
struct UpdateArgs {
    id: i64,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    /// Tri-state: field absent = don't touch, null = detach (make top-level),
    /// integer = reparent under that id.
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

/// Partial update of a todo. Fields not present are left unchanged.
pub struct TodoUpdate {
    db: Arc<TodoDb>,
}

impl TodoUpdate {
    /// Creates a new `TodoUpdate` action backed by the given `TodoDb`.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolAction for TodoUpdate {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_update".to_string(),
            description: concat!(
                "Updates fields of an existing todo. Any field omitted is left unchanged. ",
                "To reparent a todo, set parent_id to a new integer id; to detach it (make it ",
                "a top-level todo), set parent_id to null. Repurposing cannot create a cycle ",
                "(a todo cannot become a descendant of itself)."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Id of the todo to update." },
                    "content": { "type": "string", "description": "New content (omit to keep)." },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed", "cancelled"],
                        "description": "New status (omit to keep)."
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["high", "medium", "low"],
                        "description": "New priority (omit to keep)."
                    },
                    "parent_id": {
                        "description": "New parent id (integer) to reparent under, or null to detach to a top-level todo. Omit to leave unchanged.",
                        "anyOf": [
                            { "type": "integer" },
                            { "type": "null" }
                        ]
                    }
                },
                "required": ["id"]
            }),
            category: category(),
            keywords: KEYWORDS_BASE.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: UpdateArgs = parse(arguments)?;
        let updated = self
            .db
            .update(
                args.id,
                args.content.as_deref(),
                args.status.as_deref(),
                args.priority.as_deref(),
                args.parent_id,
            )
            .map_err(err)?;
        ok_json(&updated)
    }
}

// ───────────────────────── todo_complete ─────────────────────────

#[derive(Deserialize)]
struct CompleteArgs {
    id: i64,
}

/// Marks a todo and all its descendants as `completed`.
pub struct TodoComplete {
    db: Arc<TodoDb>,
}

impl TodoComplete {
    /// Creates a new `TodoComplete` action backed by the given `TodoDb`.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolAction for TodoComplete {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_complete".to_string(),
            description: concat!(
                "Marks a todo (and all of its descendants) as completed. ",
                "Use this when a large task and all of its sub-tasks are finished."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Id of the todo to complete (cascades to descendants)." }
                },
                "required": ["id"]
            }),
            category: category(),
            keywords: KEYWORDS_BASE.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: CompleteArgs = parse(arguments)?;
        let cascaded = self.db.complete(args.id).map_err(err)?;
        ok_json(&serde_json::json!({
            "id": args.id,
            "status": "completed",
            "cascaded": cascaded,
        }))
    }
}

// ───────────────────────── todo_delete ─────────────────────────

#[derive(Deserialize)]
struct DeleteArgs {
    id: i64,
}

/// Soft-deletes a todo by setting `status='cancelled'`.
pub struct TodoDelete {
    db: Arc<TodoDb>,
}

impl TodoDelete {
    /// Creates a new `TodoDelete` action backed by the given `TodoDb`.
    pub fn new(db: Arc<TodoDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolAction for TodoDelete {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "todo_delete".to_string(),
            description: concat!(
                "Soft-deletes a todo by setting status='cancelled'. ",
                "The row is kept for history. Descendants are NOT cascaded — ",
                "to cancel a whole sub-tree, delete each item individually."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Id of the todo to cancel." }
                },
                "required": ["id"]
            }),
            category: category(),
            keywords: KEYWORDS_BASE.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: DeleteArgs = parse(arguments)?;
        let updated = self.db.delete(args.id).map_err(err)?;
        ok_json(&updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fresh() -> (
        Arc<TodoDb>,
        TodoList,
        TodoAdd,
        TodoUpdate,
        TodoComplete,
        TodoDelete,
    ) {
        let db = Arc::new(TodoDb::new());
        db.set_db_path(Path::new(":memory:")).unwrap();
        db.set_session_id("sess");
        (
            db.clone(),
            TodoList::new(db.clone()),
            TodoAdd::new(db.clone()),
            TodoUpdate::new(db.clone()),
            TodoComplete::new(db.clone()),
            TodoDelete::new(db.clone()),
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
