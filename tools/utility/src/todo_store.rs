use crate::schema::utility_db_schema;
use ene_tool_db::{DbClient, DbFilter, DbOrderBy, DbValue, Row};
use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    fn from_row(row: &Row) -> Self {
        Self {
            id: row.get("id").and_then(DbValue::as_i64).unwrap_or(0),
            session_id: row
                .get("session_id")
                .and_then(DbValue::as_str)
                .unwrap_or("")
                .to_string(),
            parent_id: row.get("parent_id").and_then(DbValue::as_i64),
            content: row
                .get("content")
                .and_then(DbValue::as_str)
                .unwrap_or("")
                .to_string(),
            status: row
                .get("status")
                .and_then(DbValue::as_str)
                .unwrap_or("pending")
                .to_string(),
            priority: row
                .get("priority")
                .and_then(DbValue::as_str)
                .unwrap_or("medium")
                .to_string(),
            created_at: row
                .get("created_at")
                .and_then(DbValue::as_str)
                .unwrap_or("")
                .to_string(),
            updated_at: row
                .get("updated_at")
                .and_then(DbValue::as_str)
                .unwrap_or("")
                .to_string(),
        }
    }
}

/// DB-backed todo store using `DbClient` over IPC.
pub struct TodoStore {
    client: Arc<Mutex<DbClient>>,
}

impl TodoStore {
    /// Connects to the DB socket and declares the schema.
    ///
    /// `db_auth_token` is the pre-shared auth token presented on the
    /// DB IPC handshake; pass `None` to use the unauthenticated
    /// connect path (which the server will reject for the DB server).
    pub async fn new(
        socket_path: &Path,
        db_auth_token: Option<&str>,
    ) -> Result<Self, Box<dyn Error>> {
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
    pub async fn list(&self, session_id: &str) -> Result<Vec<TodoItem>, Box<dyn Error>> {
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
        Ok(rows.iter().map(TodoItem::from_row).collect())
    }

    /// Adds a new todo item.
    pub async fn add(
        &self,
        session_id: &str,
        parent_id: Option<i64>,
        content: &str,
        priority: &str,
    ) -> Result<TodoItem, Box<dyn Error>> {
        if content.trim().is_empty() {
            return Err("content must not be empty".into());
        }
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(format!("invalid priority: {priority}").into());
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
            .ok_or_else(|| "inserted row not found".into())
    }

    /// Updates fields of an existing todo.
    ///
    /// Rejects a reparenting (`parent_id`) that would
    /// create a cycle in the parent chain. The action's
    /// public contract ("Repurposing cannot create a
    /// cycle") is enforced here: walking the proposed
    /// parent's ancestors must not reach the todo being
    /// updated.
    pub async fn update(
        &self,
        session_id: &str,
        id: i64,
        content: Option<&str>,
        status: Option<&str>,
        priority: Option<&str>,
        parent_id: Option<Option<i64>>,
    ) -> Result<TodoItem, Box<dyn Error>> {
        if let Some(s) = status
            && !matches!(s, "pending" | "in_progress" | "completed" | "cancelled")
        {
            return Err(format!("invalid status: {s}").into());
        }
        if let Some(p) = priority
            && !matches!(p, "high" | "medium" | "low")
        {
            return Err(format!("invalid priority: {p}").into());
        }

        // Cycle check before the update. We walk up the
        // chain of the proposed new parent; if we ever
        // encounter the todo being updated, the
        // reparenting would create a cycle. We also bound
        // the walk at `MAX_ANCESTOR_DEPTH` so a
        // pre-existing corruption cannot cause this
        // method to hang.
        if let Some(Some(new_parent)) = parent_id {
            if new_parent == id {
                return Err(format!("cannot reparent todo {id} under itself").into());
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
            .ok_or_else(|| "updated row not found".into())
    }

    /// Marks a todo and all its descendants as completed. Returns all affected items.
    pub async fn complete(
        &self,
        session_id: &str,
        id: i64,
    ) -> Result<Vec<TodoItem>, Box<dyn Error>> {
        let mut client = self.client.lock().await;
        // The visited-set guard makes
        // `collect_descendants` safe even if the parent
        // chain is corrupt. Without it, a cycle in the
        // data would cause `todo_complete` to hang.
        let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
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
        Ok(rows.iter().map(TodoItem::from_row).collect())
    }

    /// Soft-deletes a todo by marking it as cancelled.
    pub async fn delete(&self, session_id: &str, id: i64) -> Result<TodoItem, Box<dyn Error>> {
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
            .ok_or_else(|| "deleted row not found".into())
    }

    /// Maximum length of the parent chain we will walk
    /// when checking a reparenting for cycles. Set to
    /// 1000 — a chain longer than that is treated as
    /// corruption rather than a valid user state.
    const MAX_ANCESTOR_DEPTH: usize = 1000;

    /// Verifies that reparenting `id` under `new_parent`
    /// would not create a cycle. Returns an error if
    /// the proposed `new_parent`'s ancestor chain
    /// includes `id` (the cycle case) or if the chain
    /// exceeds [`MAX_ANCESTOR_DEPTH`].
    async fn check_ancestor_chain(
        client: &mut DbClient,
        session_id: &str,
        id: i64,
        new_parent: i64,
    ) -> Result<(), Box<dyn Error>> {
        let mut current = new_parent;
        let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for _ in 0..Self::MAX_ANCESTOR_DEPTH {
            if current == id {
                return Err(format!(
                    "reparenting todo {id} under {new_parent} would create a cycle"
                )
                .into());
            }
            if !visited.insert(current) {
                // Pre-existing cycle in the data; treat
                // as corruption and refuse the new edge
                // rather than hang.
                return Err(format!("pre-existing cycle in parent chain at id {current}").into());
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
                // The proposed parent does not exist.
                return Err(format!("parent todo {new_parent} does not exist").into());
            };
            match row.get("parent_id") {
                Some(DbValue::Int(p)) => current = *p,
                _ => return Ok(()),
            }
        }
        Err(format!(
            "parent chain exceeds MAX_ANCESTOR_DEPTH ({}); refusing to reparent",
            Self::MAX_ANCESTOR_DEPTH
        )
        .into())
    }

    /// Recursively collects every descendant of
    /// `parent_id`. Cycle-robust via a visited-set guard:
    /// even if the underlying data is corrupt and a
    /// parent loop exists, the recursion terminates
    /// rather than hanging the caller's task. The visited
    /// set is keyed on todo id and shared across all
    /// recursive calls in one top-level invocation.
    async fn collect_descendants(
        &self,
        client: &mut DbClient,
        session_id: &str,
        parent_id: i64,
        visited: &mut std::collections::HashSet<i64>,
    ) -> Result<Vec<TodoItem>, Box<dyn Error>> {
        // Cycle guard: if we have already visited this
        // node on the current DFS path (or in the
        // general visited set), do not recurse further.
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

        let mut result: Vec<TodoItem> = children.iter().map(TodoItem::from_row).collect();
        for child in &children {
            let child_id = child.get("id").and_then(DbValue::as_i64).unwrap_or(0);
            let mut sub =
                Box::pin(self.collect_descendants(client, session_id, child_id, visited)).await?;
            result.append(&mut sub);
        }
        Ok(result)
    }
}
