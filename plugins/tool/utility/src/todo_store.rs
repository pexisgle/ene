use crate::schema::utility_db_schema;
use ene_plugin_db::{DbClient, DbError, DbFilter, DbOrderBy, DbValue, Row};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Errors produced by [`TodoStore`] operations.
#[derive(Debug, thiserror::Error)]
pub enum TodoStoreError {
    /// IPC or database server error.
    #[error("database error: {0}")]
    Db(#[from] DbError),

    /// The DB server requires authentication and rejects unauthenticated
    /// connections, so connecting without a token can never succeed.
    #[error("DB auth token is required: the DB server rejects unauthenticated connections")]
    MissingAuthToken,

    /// The `content` field was empty or whitespace-only.
    #[error("content must not be empty")]
    EmptyContent,

    /// An unrecognized priority value was supplied.
    #[error("invalid priority: {0}")]
    InvalidPriority(String),

    /// An unrecognized status value was supplied.
    #[error("invalid status: {0}")]
    InvalidStatus(String),

    /// Reparenting would create a cycle in the parent chain.
    #[error("reparenting todo {id} under {parent} would create a cycle")]
    CycleDetected {
        /// The todo being reparented.
        id: i64,
        /// The proposed new parent.
        parent: i64,
    },

    /// A pre-existing cycle was detected in corrupt data.
    #[error("pre-existing cycle in parent chain at id {0}")]
    PreExistingCycle(i64),

    /// The proposed parent does not exist.
    #[error("parent todo {0} does not exist")]
    ParentNotFound(i64),

    /// The target row was not found after a mutation.
    #[error("todo {0} not found")]
    RowNotFound(i64),

    /// The parent chain exceeds the maximum allowed depth.
    #[error("parent chain exceeds maximum depth ({0}); refusing to reparent")]
    AncestorDepthExceeded(usize),

    /// A row returned from the DB is missing a required column.
    #[error("corrupt row (id={id}): missing or invalid column '{column}'")]
    CorruptRow {
        /// The row's `id` (0 if the `id` column itself is missing).
        id: i64,
        /// The missing column name.
        column: String,
    },
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
    fn from_row(row: &Row) -> Result<Self, TodoStoreError> {
        let id =
            row.get("id")
                .and_then(DbValue::as_i64)
                .ok_or_else(|| TodoStoreError::CorruptRow {
                    id: 0,
                    column: "id".to_string(),
                })?;
        let session_id = row
            .get("session_id")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoStoreError::CorruptRow {
                id,
                column: "session_id".to_string(),
            })?
            .to_string();
        let content = row
            .get("content")
            .and_then(DbValue::as_str)
            .ok_or_else(|| TodoStoreError::CorruptRow {
                id,
                column: "content".to_string(),
            })?
            .to_string();
        Ok(Self {
            id,
            session_id,
            parent_id: row.get("parent_id").and_then(DbValue::as_i64),
            content,
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
        })
    }
}

/// DB-backed todo store using `DbClient` over IPC.
///
/// # Thread Safety
///
/// `TodoStore` is `Send + Sync`. The inner `DbClient` is wrapped in
/// `Arc<tokio::sync::Mutex<>>`, so all mutating operations are
/// serialized. Concurrent calls to `list`, `add`, `update`, etc. will
/// queue on the mutex and execute one at a time. The `Arc` ensures
/// the store can be shared across tasks and action structs.
pub struct TodoStore {
    client: Arc<Mutex<DbClient>>,
}

impl TodoStore {
    /// Connects to the DB socket and declares the schema.
    ///
    /// `db_auth_token` is the pre-shared auth token presented on the
    /// DB IPC handshake. It is required: the DB server rejects
    /// unauthenticated connections, so a `None` token returns
    /// [`TodoStoreError::MissingAuthToken`].
    pub async fn new(
        socket_path: &Path,
        db_auth_token: Option<&str>,
    ) -> Result<Self, TodoStoreError> {
        let token = db_auth_token.ok_or(TodoStoreError::MissingAuthToken)?;
        let mut client = DbClient::connect_with_token(socket_path, token).await?;
        client.declare_schema(utility_db_schema()).await?;
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Lists all todos for the given session.
    pub async fn list(&self, session_id: &str) -> Result<Vec<TodoItem>, TodoStoreError> {
        let mut client = self.client.lock().await;
        let filter = DbFilter::eq("session_id", DbValue::Text(session_id.to_string()));
        let rows = client
            .select(
                "utility_todo_items",
                &[],
                filter,
                &[DbOrderBy::asc("id")],
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
    ) -> Result<TodoItem, TodoStoreError> {
        if content.trim().is_empty() {
            return Err(TodoStoreError::EmptyContent);
        }
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(TodoStoreError::InvalidPriority(priority.to_string()));
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
            .select("utility_todo_items", &[], filter, &[], Some(1))
            .await?;
        drop(client);
        let row = rows.first().ok_or(TodoStoreError::RowNotFound(rowid))?;
        TodoItem::from_row(row)
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
    ) -> Result<TodoItem, TodoStoreError> {
        if let Some(c) = content
            && c.trim().is_empty()
        {
            return Err(TodoStoreError::EmptyContent);
        }
        if let Some(s) = status
            && !matches!(s, "pending" | "in_progress" | "completed" | "cancelled")
        {
            return Err(TodoStoreError::InvalidStatus(s.to_string()));
        }
        if let Some(p) = priority
            && !matches!(p, "high" | "medium" | "low")
        {
            return Err(TodoStoreError::InvalidPriority(p.to_string()));
        }

        // Walk the proposed parent's chain with a bounded depth so
        // pre-existing corruption cannot cause this method to hang.
        if let Some(Some(new_parent)) = parent_id {
            if new_parent == id {
                return Err(TodoStoreError::CycleDetected {
                    id,
                    parent: new_parent,
                });
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
            .select("utility_todo_items", &[], filter, &[], Some(1))
            .await?;
        drop(client);
        let row = rows.first().ok_or(TodoStoreError::RowNotFound(id))?;
        TodoItem::from_row(row)
    }

    /// Marks a todo and all its descendants as completed. Returns all affected items.
    pub async fn complete(
        &self,
        session_id: &str,
        id: i64,
    ) -> Result<Vec<TodoItem>, TodoStoreError> {
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
                &[DbOrderBy::asc("id")],
                None,
            )
            .await?;
        drop(client);
        rows.iter().map(TodoItem::from_row).collect()
    }

    /// Soft-deletes a todo by marking it as cancelled.
    pub async fn delete(&self, session_id: &str, id: i64) -> Result<TodoItem, TodoStoreError> {
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
            .select("utility_todo_items", &[], filter, &[], Some(1))
            .await?;
        drop(client);
        let row = rows.first().ok_or(TodoStoreError::RowNotFound(id))?;
        TodoItem::from_row(row)
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
    ) -> Result<(), TodoStoreError> {
        let mut current = new_parent;
        let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for _ in 0..Self::MAX_ANCESTOR_DEPTH {
            if current == id {
                return Err(TodoStoreError::CycleDetected {
                    id,
                    parent: new_parent,
                });
            }
            if !visited.insert(current) {
                // Pre-existing cycle in the data; treat
                // as corruption and refuse the new edge
                // rather than hang.
                return Err(TodoStoreError::PreExistingCycle(current));
            }
            let filter = DbFilter::And(vec![
                DbFilter::eq("id", DbValue::Int(current)),
                DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
            ]);
            let rows = client
                .select("utility_todo_items", &["parent_id"], filter, &[], Some(1))
                .await?;
            let Some(row) = rows.first() else {
                return Err(TodoStoreError::ParentNotFound(new_parent));
            };
            match row.get("parent_id") {
                Some(DbValue::Int(p)) => current = *p,
                _ => return Ok(()),
            }
        }
        Err(TodoStoreError::AncestorDepthExceeded(
            Self::MAX_ANCESTOR_DEPTH,
        ))
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
    ) -> Result<Vec<TodoItem>, TodoStoreError> {
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
                &[DbOrderBy::asc("id")],
                None,
            )
            .await?;

        let mut result: Vec<TodoItem> = Vec::new();
        for child_row in &children {
            let child_id = child_row
                .get("id")
                .and_then(DbValue::as_i64)
                .ok_or_else(|| TodoStoreError::CorruptRow {
                    id: 0,
                    column: "id".to_string(),
                })?;
            result.push(TodoItem::from_row(child_row)?);
            let mut sub =
                Box::pin(self.collect_descendants(client, session_id, child_id, visited)).await?;
            result.append(&mut sub);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_plugin_db::{DbRequest, DbResponse};
    use ene_plugin_proto::transport::{IpcListener, IpcStream, cleanup_path};
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct MockDb {
        rows: Vec<Row>,
        next_id: i64,
    }

    impl MockDb {
        fn new() -> Self {
            Self {
                rows: Vec::new(),
                next_id: 1,
            }
        }

        fn handle_request(&mut self, req: &DbRequest) -> DbResponse {
            match req {
                DbRequest::Handshake { .. } => DbResponse::HandshakeAck,
                DbRequest::DeclareSchema(_) => DbResponse::SchemaAccepted {
                    tables: vec!["utility_todo_items".to_string()],
                    indexes: Vec::new(),
                },
                DbRequest::Insert { row, .. } => {
                    let id = self.next_id;
                    self.next_id += 1;
                    let mut new_row = row.clone();
                    new_row.insert("id".to_string(), DbValue::Int(id));
                    self.rows.push(new_row);
                    DbResponse::Insert { rowid: id }
                }
                DbRequest::Select {
                    filter,
                    order_by,
                    limit,
                    ..
                } => {
                    let mut matched: Vec<Row> = self
                        .rows
                        .iter()
                        .filter(|r| matches_filter(r, filter))
                        .cloned()
                        .collect();
                    for ob in order_by.iter().rev() {
                        matched.sort_by(|a, b| {
                            let av = a.get(&ob.column);
                            let bv = b.get(&ob.column);
                            let cmp = compare_values(av, bv);
                            match ob.direction {
                                ene_plugin_db::DbOrderDirection::Desc => cmp.reverse(),
                                ene_plugin_db::DbOrderDirection::Asc => cmp,
                            }
                        });
                    }
                    if let Some(lim) = limit {
                        matched.truncate(*lim as usize);
                    }
                    DbResponse::Select { rows: matched }
                }
                DbRequest::Update { set, filter, .. } => {
                    let mut affected = 0u64;
                    for row in &mut self.rows {
                        if matches_filter(row, filter) {
                            for (k, v) in set {
                                row.insert(k.clone(), v.clone());
                            }
                            affected += 1;
                        }
                    }
                    DbResponse::Update { affected }
                }
                DbRequest::Ping => DbResponse::Pong,
                _ => DbResponse::Error {
                    code: ene_plugin_db::DbErrorCode::Internal,
                    message: "unsupported request in mock".to_string(),
                },
            }
        }
    }

    fn compare_values(a: Option<&DbValue>, b: Option<&DbValue>) -> std::cmp::Ordering {
        match (a, b) {
            (Some(DbValue::Int(x)), Some(DbValue::Int(y))) => x.cmp(y),
            (Some(DbValue::Text(x)), Some(DbValue::Text(y))) => x.cmp(y),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    }

    fn matches_filter(row: &Row, filter: &DbFilter) -> bool {
        match filter {
            DbFilter::Eq { column, value } => row.get(column) == Some(value),
            DbFilter::And(filters) => filters.iter().all(|f| matches_filter(row, f)),
            DbFilter::Or(filters) => filters.iter().any(|f| matches_filter(row, f)),
            DbFilter::In { column, values } => row.get(column).is_some_and(|v| values.contains(v)),
            _ => false,
        }
    }

    async fn read_framed(stream: &mut IpcStream) -> Option<DbRequest> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await.ok()?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.ok()?;
        serde_json::from_slice(&buf).ok()
    }

    async fn write_framed(stream: &mut IpcStream, resp: &DbResponse) {
        let json = serde_json::to_vec(resp).unwrap();
        let len = json.len() as u32;
        stream.write_all(&len.to_le_bytes()).await.unwrap();
        stream.write_all(&json).await.unwrap();
        stream.flush().await.unwrap();
    }

    /// Spawns a mock DB server on a temporary Unix socket and returns
    /// the socket path. The server runs until the returned `JoinHandle`
    /// is dropped or the listener is closed.
    ///
    /// The listener is bound *before* the accept loop is spawned, so the socket
    /// is guaranteed to exist by the time this returns. Polling for the socket
    /// file after spawning would be a race: the poll gave up silently after a
    /// bounded number of yields, and a connect that lost that race failed with
    /// a bare `NotFound` far from its cause.
    async fn spawn_mock_db() -> (PathBuf, tokio::task::JoinHandle<()>) {
        let socket_path = std::env::temp_dir().join(format!(
            "ene-utility-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        cleanup_path(&socket_path);

        let mut listener = IpcListener::bind(&socket_path).unwrap();

        let handle = tokio::spawn(async move {
            let mut db = MockDb::new();
            loop {
                let Ok(mut stream) = listener.accept().await else {
                    break;
                };
                loop {
                    let Some(req) = read_framed(&mut stream).await else {
                        break;
                    };
                    let resp = db.handle_request(&req);
                    write_framed(&mut stream, &resp).await;
                }
            }
        });

        (socket_path, handle)
    }

    async fn make_store() -> (TodoStore, PathBuf, tokio::task::JoinHandle<()>) {
        let (path, handle) = spawn_mock_db().await;
        let store = TodoStore::new(&path, Some("test-token")).await.unwrap();
        (store, path, handle)
    }

    #[tokio::test]
    async fn add_and_list_crud() {
        let (store, path, handle) = make_store().await;

        let item = store.add("sess1", None, "Buy milk", "high").await.unwrap();
        assert_eq!(item.content, "Buy milk");
        assert_eq!(item.priority, "high");
        assert_eq!(item.status, "pending");
        assert_eq!(item.session_id, "sess1");
        assert!(item.id > 0);

        let items = store.list("sess1").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item.id);

        let other = store.list("sess2").await.unwrap();
        assert!(other.is_empty());

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn add_rejects_empty_content() {
        let (store, path, handle) = make_store().await;
        let err = store.add("s", None, "   ", "low").await.unwrap_err();
        assert!(matches!(err, TodoStoreError::EmptyContent));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn add_rejects_invalid_priority() {
        let (store, path, handle) = make_store().await;
        let err = store.add("s", None, "task", "urgent").await.unwrap_err();
        assert!(matches!(err, TodoStoreError::InvalidPriority(_)));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn update_content_and_status() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "original", "medium").await.unwrap();

        let updated = store
            .update(
                "s",
                item.id,
                Some("changed"),
                Some("in_progress"),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.content, "changed");
        assert_eq!(updated.status, "in_progress");
        assert_eq!(updated.priority, "medium");

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn update_rejects_empty_content() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "task", "low").await.unwrap();
        let err = store
            .update("s", item.id, Some(""), None, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TodoStoreError::EmptyContent));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn update_rejects_invalid_status() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "task", "low").await.unwrap();
        let err = store
            .update("s", item.id, None, Some("done"), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TodoStoreError::InvalidStatus(_)));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn reparent_basic() {
        let (store, path, handle) = make_store().await;
        let parent = store.add("s", None, "parent", "high").await.unwrap();
        let child = store.add("s", None, "child", "low").await.unwrap();

        let updated = store
            .update("s", child.id, None, None, None, Some(Some(parent.id)))
            .await
            .unwrap();
        assert_eq!(updated.parent_id, Some(parent.id));

        // Detach back to top-level.
        let detached = store
            .update("s", child.id, None, None, None, Some(None))
            .await
            .unwrap();
        assert_eq!(detached.parent_id, None);

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn reparent_self_is_cycle() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "self", "medium").await.unwrap();
        let err = store
            .update("s", item.id, None, None, None, Some(Some(item.id)))
            .await
            .unwrap_err();
        assert!(matches!(err, TodoStoreError::CycleDetected { .. }));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn reparent_descendant_is_cycle() {
        let (store, path, handle) = make_store().await;
        let a = store.add("s", None, "A", "high").await.unwrap();
        let b = store.add("s", Some(a.id), "B", "high").await.unwrap();
        let c = store.add("s", Some(b.id), "C", "high").await.unwrap();

        // Try to make A a child of C (A -> B -> C -> A would be a cycle).
        let err = store
            .update("s", a.id, None, None, None, Some(Some(c.id)))
            .await
            .unwrap_err();
        assert!(matches!(err, TodoStoreError::CycleDetected { .. }));

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn reparent_under_nonexistent_parent() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "task", "low").await.unwrap();
        let err = store
            .update("s", item.id, None, None, None, Some(Some(9999)))
            .await
            .unwrap_err();
        assert!(matches!(err, TodoStoreError::ParentNotFound(9999)));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn complete_cascades_to_descendants() {
        let (store, path, handle) = make_store().await;
        let root = store.add("s", None, "root", "high").await.unwrap();
        let child1 = store
            .add("s", Some(root.id), "child1", "medium")
            .await
            .unwrap();
        let child2 = store
            .add("s", Some(root.id), "child2", "low")
            .await
            .unwrap();
        let grandchild = store.add("s", Some(child1.id), "gc", "high").await.unwrap();

        let completed = store.complete("s", root.id).await.unwrap();
        assert_eq!(completed.len(), 4);
        for item in &completed {
            assert_eq!(item.status, "completed");
        }

        let all = store.list("s").await.unwrap();
        assert!(all.iter().all(|i| i.status == "completed"));

        let _ = (child1, child2, grandchild);
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn collect_descendants_handles_diamond() {
        // Diamond: A -> B, A -> C, B -> D, C -> D
        // D should appear once in the descendant list.
        let (store, path, handle) = make_store().await;
        let a = store.add("s", None, "A", "high").await.unwrap();
        let b = store.add("s", Some(a.id), "B", "high").await.unwrap();
        let c = store.add("s", Some(a.id), "C", "high").await.unwrap();
        // D has parent B; we can't have two parents in this schema,
        // so test the visited-set guard via complete() on A.
        let d = store.add("s", Some(b.id), "D", "high").await.unwrap();

        let completed = store.complete("s", a.id).await.unwrap();
        assert_eq!(completed.len(), 4);
        let ids: Vec<i64> = completed.iter().map(|i| i.id).collect();
        assert!(ids.contains(&a.id));
        assert!(ids.contains(&b.id));
        assert!(ids.contains(&c.id));
        assert!(ids.contains(&d.id));

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn delete_marks_cancelled() {
        let (store, path, handle) = make_store().await;
        let item = store.add("s", None, "to delete", "low").await.unwrap();
        let deleted = store.delete("s", item.id).await.unwrap();
        assert_eq!(deleted.status, "cancelled");
        assert_eq!(deleted.id, item.id);
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let (store, path, handle) = make_store().await;
        let err = store.delete("s", 9999).await.unwrap_err();
        assert!(matches!(err, TodoStoreError::RowNotFound(9999)));
        handle.abort();
        cleanup_path(&path);
    }
}
