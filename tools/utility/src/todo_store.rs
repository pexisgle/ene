use crate::schema::utility_db_schema;
use ene_tool_db::{DbClient, DbError, DbFilter, DbOrderBy, DbValue, Row};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

/// Errors from [`TodoStore`] operations.
#[derive(Error, Debug)]
pub enum TodoError {
    /// Content string was empty after trimming.
    #[error("content must not be empty")]
    EmptyContent,
    /// Priority value is not one of the allowed values.
    #[error("invalid priority: {0}")]
    InvalidPriority(String),
    /// Status value is not one of the allowed values.
    #[error("invalid status: {0}")]
    InvalidStatus(String),
    /// Attempt to reparent a todo under itself.
    #[error("cannot reparent todo {id} under itself")]
    SelfReparent {
        /// The todo ID that would be reparented.
        id: i64,
    },
    /// Reparenting would create a cycle in the ancestor chain.
    #[error("reparenting todo {id} under {new_parent} would create a cycle")]
    CycleDetected {
        /// The todo ID being reparented.
        id: i64,
        /// The proposed new parent ID.
        new_parent: i64,
    },
    /// A cycle was detected in the existing parent data.
    #[error("pre-existing cycle in parent chain at id {0}")]
    CorruptChain(i64),
    /// The specified parent todo does not exist.
    #[error("parent todo {0} does not exist")]
    ParentNotFound(i64),
    /// The ancestor chain exceeded the maximum allowed depth.
    #[error("parent chain exceeds MAX_ANCESTOR_DEPTH ({0})")]
    AncestorDepthExceeded(usize),
    /// The inserted row could not be fetched back.
    #[error("inserted row not found after insert")]
    InsertedRowNotFound,
    /// The updated row could not be found.
    #[error("updated row not found after update")]
    UpdatedRowNotFound,
    /// The deleted row could not be found.
    #[error("deleted row not found after delete")]
    DeletedRowNotFound,
    /// A required column was missing or corrupt in a database row.
    #[error("missing or corrupt column '{0}' in database row")]
    CorruptRow(String),
    /// An underlying DB communication error.
    #[error(transparent)]
    Db(#[from] DbError),
}

/// A single todo item returned from the store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TodoItem {
    /// Unique identifier.
    pub id: i64,
    /// Session this item belongs to.
    pub session_id: String,
    /// Parent todo ID for hierarchy.
    pub parent_id: Option<i64>,
    /// Task description.
    pub content: String,
    /// Status: pending, `in_progress`, completed, cancelled.
    pub status: String,
    /// Priority: high, medium, low.
    pub priority: String,
    /// Creation timestamp (RFC3339).
    pub created_at: String,
    /// Last update timestamp (RFC3339).
    pub updated_at: String,
}

impl TodoItem {
    fn from_row(row: &Row) -> Result<Self, TodoError> {
        let id = row
            .get("id")
            .and_then(DbValue::as_i64)
            .ok_or_else(|| TodoError::CorruptRow("id".to_string()))?;
        let session_id = row
            .get("session_id")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("session_id".to_string()))?
            .to_string();
        let content = row
            .get("content")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("content".to_string()))?
            .to_string();
        let status = row
            .get("status")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("status".to_string()))?
            .to_string();
        let priority = row
            .get("priority")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("priority".to_string()))?
            .to_string();
        let created_at = row
            .get("created_at")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("created_at".to_string()))?
            .to_string();
        let updated_at = row
            .get("updated_at")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoError::CorruptRow("updated_at".to_string()))?
            .to_string();
        let parent_id = row.get("parent_id").and_then(DbValue::as_i64);
        Ok(Self {
            id,
            session_id,
            parent_id,
            content,
            status,
            priority,
            created_at,
            updated_at,
        })
    }
}

/// DB-backed todo store using `DbClient` over IPC.
pub struct TodoStore {
    client: Arc<Mutex<DbClient>>,
}

impl TodoStore {
    /// Connects to the DB socket and declares the schema.
    pub async fn new(socket_path: &Path, db_auth_token: Option<&str>) -> Result<Self, TodoError> {
        let mut client = match db_auth_token {
            Some(t) => DbClient::connect_with_token(socket_path, t).await?,
            None => DbClient::connect(socket_path).await?,
        };
        client.declare_schema(utility_db_schema()).await?;
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Lists all todos for the given session.
    pub async fn list(&self, session_id: &str) -> Result<Vec<TodoItem>, TodoError> {
        let mut client = self.client.lock().await;
        let filter = DbFilter::eq("session_id", DbValue::Text(session_id.to_string()));
        let rows = client
            .select(
                "utility_todo_items",
                &[],
                filter,
                vec![DbOrderBy::asc("id")],
                None,
            )
            .await?;
        drop(client);
        rows.iter().map(TodoItem::from_row).collect()
    }

    /// Adds a new todo item.
    pub async fn add(
        &self,
        session_id: &str,
        parent_id: Option<i64>,
        content: &str,
        priority: &str,
    ) -> Result<TodoItem, TodoError> {
        if content.trim().is_empty() {
            return Err(TodoError::EmptyContent);
        }
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(TodoError::InvalidPriority(priority.to_string()));
        }

        let now = chrono::Utc::now().to_rfc3339();
        let mut row: Row = BTreeMap::new();
        row.insert(
            "session_id".to_string(),
            DbValue::Text(session_id.to_string()),
        );
        row.insert(
            "parent_id".to_string(),
            parent_id.map_or(DbValue::Null, DbValue::Int),
        );
        row.insert("content".to_string(), DbValue::Text(content.to_string()));
        row.insert("status".to_string(), DbValue::Text("pending".to_string()));
        row.insert("priority".to_string(), DbValue::Text(priority.to_string()));
        row.insert("created_at".to_string(), DbValue::Text(now.clone()));
        row.insert("updated_at".to_string(), DbValue::Text(now));

        let mut client = self.client.lock().await;
        let rowid = client.insert("utility_todo_items", row).await?;

        let filter = DbFilter::eq("id", DbValue::Int(rowid));
        let rows = client
            .select("utility_todo_items", &[], filter, vec![], Some(1))
            .await?;
        drop(client);
        rows.first()
            .map(TodoItem::from_row)
            .ok_or(TodoError::InsertedRowNotFound)?
    }

    /// Updates fields of an existing todo.
    pub async fn update(
        &self,
        session_id: &str,
        id: i64,
        content: Option<&str>,
        status: Option<&str>,
        priority: Option<&str>,
        parent_id: Option<Option<i64>>,
    ) -> Result<TodoItem, TodoError> {
        if let Some(s) = status
            && !matches!(s, "pending" | "in_progress" | "completed" | "cancelled")
        {
            return Err(TodoError::InvalidStatus(s.to_string()));
        }
        if let Some(p) = priority
            && !matches!(p, "high" | "medium" | "low")
        {
            return Err(TodoError::InvalidPriority(p.to_string()));
        }

        if let Some(Some(new_parent)) = parent_id {
            if new_parent == id {
                return Err(TodoError::SelfReparent { id });
            }
            let mut client = self.client.lock().await;
            Self::check_ancestor_chain(&mut client, session_id, id, new_parent).await?;
            drop(client);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let mut set: Row = BTreeMap::new();
        if let Some(c) = content {
            set.insert("content".to_string(), DbValue::Text(c.to_string()));
        }
        if let Some(s) = status {
            set.insert("status".to_string(), DbValue::Text(s.to_string()));
        }
        if let Some(p) = priority {
            set.insert("priority".to_string(), DbValue::Text(p.to_string()));
        }
        if let Some(pid) = parent_id {
            set.insert(
                "parent_id".to_string(),
                pid.map_or(DbValue::Null, DbValue::Int),
            );
        }
        set.insert("updated_at".to_string(), DbValue::Text(now));

        let filter = DbFilter::And(vec![
            DbFilter::eq("id", DbValue::Int(id)),
            DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
        ]);

        let mut client = self.client.lock().await;
        client.update("utility_todo_items", set, filter).await?;

        let filter = DbFilter::eq("id", DbValue::Int(id));
        let rows = client
            .select("utility_todo_items", &[], filter, vec![], Some(1))
            .await?;
        drop(client);
        rows.first()
            .map(TodoItem::from_row)
            .ok_or(TodoError::UpdatedRowNotFound)?
    }

    /// Marks a todo and all its descendants as completed.
    pub async fn complete(&self, session_id: &str, id: i64) -> Result<Vec<TodoItem>, TodoError> {
        let mut client = self.client.lock().await;
        let mut visited: HashSet<i64> = HashSet::new();
        let descendants = self
            .collect_descendants(&mut client, session_id, id, &mut visited)
            .await?;

        let mut ids_to_complete = vec![id];
        ids_to_complete.extend(descendants.iter().map(|item| item.id));

        let now = chrono::Utc::now().to_rfc3339();
        for item_id in &ids_to_complete {
            let mut set: Row = BTreeMap::new();
            set.insert("status".to_string(), DbValue::Text("completed".to_string()));
            set.insert("updated_at".to_string(), DbValue::Text(now.clone()));

            let filter = DbFilter::And(vec![
                DbFilter::eq("id", DbValue::Int(*item_id)),
                DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
            ]);
            client.update("utility_todo_items", set, filter).await?;
        }

        let filter = DbFilter::In {
            column: "id".to_string(),
            values: ids_to_complete.iter().map(|id| DbValue::Int(*id)).collect(),
        };
        let rows = client
            .select(
                "utility_todo_items",
                &[],
                filter,
                vec![DbOrderBy::asc("id")],
                None,
            )
            .await?;
        drop(client);
        rows.iter().map(TodoItem::from_row).collect()
    }

    /// Soft-deletes a todo by marking it as cancelled.
    pub async fn delete(&self, session_id: &str, id: i64) -> Result<TodoItem, TodoError> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut set: Row = BTreeMap::new();
        set.insert("status".to_string(), DbValue::Text("cancelled".to_string()));
        set.insert("updated_at".to_string(), DbValue::Text(now));

        let filter = DbFilter::And(vec![
            DbFilter::eq("id", DbValue::Int(id)),
            DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
        ]);

        let mut client = self.client.lock().await;
        client.update("utility_todo_items", set, filter).await?;

        let filter = DbFilter::eq("id", DbValue::Int(id));
        let rows = client
            .select("utility_todo_items", &[], filter, vec![], Some(1))
            .await?;
        drop(client);
        rows.first()
            .map(TodoItem::from_row)
            .ok_or(TodoError::DeletedRowNotFound)?
    }

    const MAX_ANCESTOR_DEPTH: usize = 1000;

    async fn check_ancestor_chain(
        client: &mut DbClient,
        session_id: &str,
        id: i64,
        new_parent: i64,
    ) -> Result<(), TodoError> {
        let mut current = new_parent;
        let mut visited: HashSet<i64> = HashSet::new();
        for _ in 0..Self::MAX_ANCESTOR_DEPTH {
            if current == id {
                return Err(TodoError::CycleDetected { id, new_parent });
            }
            if !visited.insert(current) {
                return Err(TodoError::CorruptChain(current));
            }
            let filter = DbFilter::And(vec![
                DbFilter::eq("id", DbValue::Int(current)),
                DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
            ]);
            let rows = client
                .select(
                    "utility_todo_items",
                    &["parent_id"],
                    filter,
                    vec![],
                    Some(1),
                )
                .await?;
            let Some(row) = rows.first() else {
                return Err(TodoError::ParentNotFound(new_parent));
            };
            match row.get("parent_id") {
                Some(DbValue::Int(p)) => current = *p,
                _ => return Ok(()),
            }
        }
        Err(TodoError::AncestorDepthExceeded(Self::MAX_ANCESTOR_DEPTH))
    }

    async fn collect_descendants(
        &self,
        client: &mut DbClient,
        session_id: &str,
        parent_id: i64,
        visited: &mut HashSet<i64>,
    ) -> Result<Vec<TodoItem>, TodoError> {
        if !visited.insert(parent_id) {
            return Ok(Vec::new());
        }

        let filter = DbFilter::And(vec![
            DbFilter::eq("parent_id", DbValue::Int(parent_id)),
            DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
        ]);
        let children = client
            .select(
                "utility_todo_items",
                &[],
                filter,
                vec![DbOrderBy::asc("id")],
                None,
            )
            .await?;

        let items: Vec<TodoItem> = children
            .iter()
            .map(TodoItem::from_row)
            .collect::<Result<_, _>>()?;
        let mut result: Vec<TodoItem> = Vec::new();
        for child in &items {
            let child_id = child.id;
            let mut sub =
                Box::pin(self.collect_descendants(client, session_id, child_id, visited)).await?;
            result.append(&mut sub);
        }
        result.extend(items);
        Ok(result)
    }
}
