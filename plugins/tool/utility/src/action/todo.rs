use crate::provider::UtilityState;
use crate::todo_store::TodoStoreError;
use ene_tool_sdk::prelude::*;
use std::sync::Arc;

fn ok_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::internal(format!("json serialization failed: {e}")))
}

/// Maps a [`TodoStoreError`] to the appropriate [`ToolError`] variant.
///
/// Validation errors (empty content, invalid priority/status, cycles,
/// missing parent) map to `InvalidArguments`; database/transport errors
/// map to `Internal`.
fn store_err(e: &TodoStoreError) -> ToolError {
    match e {
        TodoStoreError::EmptyContent
        | TodoStoreError::InvalidPriority(_)
        | TodoStoreError::InvalidStatus(_)
        | TodoStoreError::CycleDetected { .. }
        | TodoStoreError::PreExistingCycle(_)
        | TodoStoreError::ParentNotFound(_)
        | TodoStoreError::AncestorDepthExceeded(_) => ToolError::InvalidArguments {
            message: e.to_string(),
        },
        TodoStoreError::Db(_)
        | TodoStoreError::MissingAuthToken
        | TodoStoreError::RowNotFound(_)
        | TodoStoreError::CorruptRow { .. } => ToolError::internal(e.to_string()),
    }
}

fn default_state() -> Arc<UtilityState> {
    Arc::new(UtilityState::new())
}

// ───────────────────────── todo_list ─────────────────────────

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "todo_list",
    summary = "List all todos in the current session.",
    description = "Returns the full todo tree for the current session, including each item's id, content, status, priority, parent relationship, and a summary of active vs total items.",
    category = "Utility",
    keywords_primary = "todo, task, track, plan, checklist",
    side_effects = "Idempotent"
)]
/// Action to list all todos in the current session.
pub struct TodoListAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<UtilityState>,
}

impl TodoListAction {
    /// Creates a new `TodoListAction`.
    #[must_use]
    pub const fn new(state: Arc<UtilityState>) -> Self {
        Self { state }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        let store = self.state.ensure_todo_store().await?;
        let items = store.list(&session_id).await.map_err(|e| store_err(&e))?;
        let count = items.len();
        let active = items
            .iter()
            .filter(|i| i.status != "completed" && i.status != "cancelled")
            .count();
        let body = serde_json::json!({
            "summary": {
                "total": count,
                "active": active,
                "session_id": session_id,
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
    keywords_primary = "todo, task, track, plan, checklist",
    side_effects = "Idempotent"
)]
/// Action to add a new todo.
pub struct TodoAddAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<UtilityState>,
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
    /// Creates a new `TodoAddAction`.
    #[must_use]
    pub const fn new(state: Arc<UtilityState>) -> Self {
        Self {
            state,
            content: String::new(),
            priority: String::new(),
            parent_id: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        let store = self.state.ensure_todo_store().await?;
        let item = store
            .add(&session_id, self.parent_id, &self.content, &self.priority)
            .await
            .map_err(|e| store_err(&e))?;
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
    keywords_primary = "todo, task, track, plan, checklist",
    side_effects = "Idempotent"
)]
/// Action to update an existing todo's fields.
pub struct TodoUpdateAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<UtilityState>,
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
    /// Creates a new `TodoUpdateAction`.
    #[must_use]
    pub const fn new(state: Arc<UtilityState>) -> Self {
        Self {
            state,
            id: 0,
            content: None,
            status: None,
            priority: None,
            parent_id: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        let store = self.state.ensure_todo_store().await?;
        let updated = store
            .update(
                &session_id,
                self.id,
                self.content.as_deref(),
                self.status.as_deref(),
                self.priority.as_deref(),
                self.parent_id,
            )
            .await
            .map_err(|e| store_err(&e))?;
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
    keywords_primary = "todo, task, track, plan, checklist",
    side_effects = "Idempotent"
)]
/// Action to mark a todo and all its sub-tasks as completed.
pub struct TodoCompleteAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<UtilityState>,
    /// Id of the todo to complete (cascades to descendants).
    id: i64,
}

impl TodoCompleteAction {
    /// Creates a new `TodoCompleteAction`.
    #[must_use]
    pub const fn new(state: Arc<UtilityState>) -> Self {
        Self { state, id: 0 }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        let store = self.state.ensure_todo_store().await?;
        let cascaded = store
            .complete(&session_id, self.id)
            .await
            .map_err(|e| store_err(&e))?;
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
    keywords_primary = "todo, task, track, plan, checklist",
    side_effects = "Idempotent"
)]
/// Action to soft-delete a todo by marking it as cancelled.
pub struct TodoDeleteAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<UtilityState>,
    /// Id of the todo to cancel.
    id: i64,
}

impl TodoDeleteAction {
    /// Creates a new `TodoDeleteAction`.
    #[must_use]
    pub const fn new(state: Arc<UtilityState>) -> Self {
        Self { state, id: 0 }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let session_id = self.state.session_id();
        let store = self.state.ensure_todo_store().await?;
        let updated = store
            .delete(&session_id, self.id)
            .await
            .map_err(|e| store_err(&e))?;
        ok_json(&updated)
    }
}
