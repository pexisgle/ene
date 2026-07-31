//! Per-tool DB IPC server using SeaORM.
//!
//! Listens on a Unix socket, accepts tool connections, and dispatches
//! [`DbRequest`] messages against the shared `memory.db`.
//!
//! ## Security model
//!
//! - Each tool declares its schema (tables + columns) via `DeclareSchema`.
//! - The server records the declaration in `__tool_schemas` and enforces that
//!   all subsequent requests only reference tables/columns in the declaration.
//! - Table names must start with the tool's prefix (e.g. `fs_`, `utility_`),
//!   and index names must carry the prefix too (`SQLite` index names live in a
//!   single database-wide namespace, so an unprefixed index could squat a name
//!   a core migration later needs).
//! - DDL is **not** exposed to tools as a direct operation. `CREATE TABLE` and
//!   `CREATE INDEX` — plus the additive `ALTER TABLE ... ADD COLUMN` used to
//!   grow an existing table — run only as a consequence of `DeclareSchema`,
//!   under prefix isolation and identifier validation. The server never drops
//!   tables or columns and rejects destructive or conflicting schema changes.
//! - `sqlite_master` and other internal tables are blocked.
//!
//! ## Schema evolution
//!
//! On every `DeclareSchema` the server hashes the declaration (BLAKE3) and
//! compares it against the `fingerprint` stored in `__tool_schemas`:
//!
//! - **Unchanged** — the stored row is left untouched (no needless rewrite) and
//!   the existing tables are reused as-is.
//! - **Additive change** (new table, or a new column on an existing table) —
//!   the new column is applied with `ALTER TABLE ... ADD COLUMN`, the stored
//!   declaration is refreshed, and the tool proceeds. `SQLite` fills existing
//!   rows with the column's `DEFAULT` (or `NULL`).
//! - **Conflicting change** (a column type change, a previously declared
//!   table or column dropped from the declaration, or a new column that
//!   carries a `PRIMARY KEY`/`UNIQUE`/`AUTOINCREMENT` constraint) — rejected
//!   with [`DbServerError::SchemaConflict`]. `SQLite` cannot alter or drop
//!   columns in place, nor add a constrained column, so the host refuses to
//!   let the validation layer and the actual tables silently diverge; the
//!   plugin author must reconcile the difference.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::entities::tool_schemas;
use ene_plugin_db::{
    DbBatchOpResult, DbColumn, DbErrorCode, DbFilter, DbOrderBy, DbRequest, DbResponse, DbSchema,
    DbTable, DbType, DbValue, DbWriteOp, Row,
};
use sea_orm::sea_query::{Alias, Condition, Expr, Query, SqliteQueryBuilder};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, ExprTrait, Statement,
    TransactionTrait,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info, warn};

/// Maximum number of operations accepted in a single [`DbRequest::Batch`].
///
/// The server holds the `SQLite` write lock for the whole batch, and each
/// operation carries a full [`Row`](crate::Row) of values, so an unbounded
/// batch would let a plugin pin the write lock and memory for an arbitrarily
/// long time. Requests above this limit are rejected up front.
const MAX_BATCH_OPS: usize = 10_000;

/// Errors from the DB IPC server.
#[derive(Debug, thiserror::Error)]
pub enum DbServerError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// `SeaORM` database error.
    #[error("Database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    /// The tool does not have permission to access the resource.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    /// The specified table is unknown or not declared.
    #[error("Unknown table: {0}")]
    UnknownTable(String),
    /// The specified column is unknown.
    #[error("Unknown column: {table}.{column}")]
    UnknownColumn {
        /// Table name.
        table: String,
        /// Column name.
        column: String,
    },
    /// An operation inside a [`DbRequest::Batch`] failed at `index`.
    ///
    /// Wraps the underlying error so the failing operation index can be
    /// reported in the *message* while the wire error code of the underlying
    /// error is preserved — clients that match on the typed error they would
    /// get from a standalone request still catch it.
    #[error("batch op {index}: {source}")]
    BatchOp {
        /// Index of the failing operation within the batch.
        index: usize,
        /// The underlying error.
        source: Box<DbServerError>,
    },
    /// The declared schema conflicts with the one already stored for this
    /// prefix in a way that cannot be applied automatically (e.g. a column
    /// type change or a removed table). The plugin must reconcile the
    /// difference explicitly rather than having the host silently diverge.
    #[error("Schema conflict: {0}")]
    SchemaConflict(String),
    /// An internal server error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl DbServerError {
    fn to_error_response(&self) -> DbResponse {
        let (code, message) = self.error_parts();
        DbResponse::Error { code, message }
    }

    /// Splits this error into its wire [`DbErrorCode`] and human-readable
    /// message, recursing through [`Self::BatchOp`] so the batch operation
    /// index is prefixed to the message without corrupting any structured
    /// fields (e.g. `UnknownColumn`'s `table`).
    fn error_parts(&self) -> (DbErrorCode, String) {
        match self {
            Self::PermissionDenied(msg) => (DbErrorCode::PermissionDenied, msg.clone()),
            Self::UnknownTable(msg) => (DbErrorCode::UnknownTable, msg.clone()),
            Self::UnknownColumn { table, column } => (
                DbErrorCode::UnknownColumn,
                format!("Unknown column: {table}.{column}"),
            ),
            Self::SchemaConflict(msg) => (DbErrorCode::SchemaConflict, msg.clone()),
            Self::Io(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Json(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Db(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Internal(msg) => (DbErrorCode::Internal, msg.clone()),
            Self::BatchOp { index, source } => {
                let (code, message) = source.error_parts();
                (code, format!("batch op {index}: {message}"))
            }
        }
    }
}

/// Per-tool DB IPC server.
pub struct DbIpcServer {
    db: DatabaseConnection,
    socket_path: PathBuf,
    tool_name: String,
    prefix: String,
    /// Pre-shared auth token required on the very first request of
    /// every connection. Generated by the parent and passed to the
    /// tool binary via the `ENE_DB_AUTH_TOKEN` env var.
    auth_token: String,
}

impl DbIpcServer {
    /// Creates a new server for the given tool.
    ///
    /// `db` must be a sea-orm database connection that is already
    /// open, has had its PRAGMAs applied, and has had migrations run.
    /// The caller is responsible for providing a connection from
    /// the same pool that `MemoryStore` uses.
    pub const fn new(
        db: DatabaseConnection,
        socket_path: PathBuf,
        tool_name: String,
        prefix: String,
        auth_token: String,
    ) -> Self {
        Self {
            db,
            socket_path,
            tool_name,
            prefix,
            auth_token,
        }
    }

    /// Runs the server, listening for connections and handling requests.
    pub async fn run(self) -> Result<(), DbServerError> {
        // On Unix, remove any stale socket file from a previous run
        // before binding. Named pipes are kernel objects, not
        // filesystem objects, so this step is a no-op on Windows.
        #[cfg(unix)]
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        let mut listener = ene_plugin_proto::transport::IpcListener::bind(&self.socket_path)?;

        // Restrict the unix socket to the owning user. Without this
        // chmod the socket in /tmp is world-connectable, allowing any
        // local process to connect and issue DeclareSchema on the tool's
        // behalf (combined with the SQL-injection vector fixed in
        // #23 this is a privilege escalation). Named pipes use a
        // per-handle ACL instead, which the kernel sets when we bind.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&self.socket_path, perms) {
                return Err(DbServerError::Internal(format!(
                    "failed to chmod DB socket to 0o600: {e}"
                )));
            }
        }

        info!(
            tool = %self.tool_name,
            socket = %self.socket_path.display(),
            "DB IPC server listening"
        );

        #[expect(
            clippy::infinite_loop,
            reason = "DB IPC accept loop runs until the server task is cancelled"
        )]
        loop {
            // Continue the accept loop on transient accept
            // errors (EMFILE, ENFILE, EINTR, ...). A
            // permanent kill of the DB server on a single
            // resource-exhaustion event would leave the
            // tool offline until the next reconfigure.
            // The errors that *do* terminate the server
            // (broken listener, socket removed, etc.) are
            // surfaced via the original `?` on the
            // `accept()` call only after we have logged
            // and backed off, so a single transient
            // failure does not silently break the tool.
            let stream = match listener.accept().await {
                Ok(stream) => stream,
                Err(e) => {
                    error!(
                        tool = %self.tool_name,
                        error = %e,
                        "DB IPC accept failed; backing off and continuing"
                    );
                    // Exponential backoff capped at 5s.
                    // The previous behavior was `?`-propagate,
                    // which killed the server task on a
                    // single accept failure.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
            };
            debug!(tool = %self.tool_name, "Accepted DB IPC connection");

            let db = self.db.clone();
            let tool_name = self.tool_name.clone();
            let prefix = self.prefix.clone();
            let auth_token = self.auth_token.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    Self::handle_connection(stream, db, tool_name, prefix, auth_token).await
                {
                    error!(error = %e, "DB IPC connection error");
                }
            });
        }
    }

    async fn handle_connection(
        stream: ene_plugin_proto::transport::IpcStream,
        db: DatabaseConnection,
        tool_name: String,
        prefix: String,
        auth_token: String,
    ) -> Result<(), DbServerError> {
        let mut stream = stream;

        // Track the rowid of the most recent Insert so that
        // a subsequent `LastInsertRowId` request can return
        // the correct value. SQLite's `last_insert_rowid()`
        // is per-connection; with SeaORM's pool, the
        // connection serving a query may not be the one
        // that performed the prior insert, so a raw
        // `SELECT last_insert_rowid()` is racy. Tracking
        // it here (one cell per logical `handle_connection`)
        // matches the tool's own view: a tool client
        // always pairs Insert and LastInsertRowId on the
        // same connection.
        let last_rowid: Arc<std::sync::Mutex<Option<i64>>> = Arc::new(std::sync::Mutex::new(None));

        let mut declared_tables: HashMap<String, DbTable> = HashMap::new();
        let mut declared_columns: HashMap<String, HashSet<String>> = HashMap::new();

        let mut authenticated = false;

        loop {
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    debug!(tool = %tool_name, "DB IPC connection closed");
                    break;
                }
                Err(e) => return Err(DbServerError::Io(e)),
            }
            let Ok(msg_len) = usize::try_from(u32::from_le_bytes(len_buf)) else {
                return Err(DbServerError::Internal(
                    "message length overflow on this platform".to_string(),
                ));
            };
            if msg_len > 64 * 1024 * 1024 {
                return Err(DbServerError::Internal(format!(
                    "message too large: {msg_len}"
                )));
            }

            let mut msg_buf = vec![0u8; msg_len];
            stream.read_exact(&mut msg_buf).await?;

            let request: DbRequest = match serde_json::from_slice(&msg_buf) {
                Ok(req) => req,
                Err(e) => {
                    warn!(tool = %tool_name, error = %e, "Invalid JSON in DB request");
                    let response = DbResponse::Error {
                        code: DbErrorCode::Internal,
                        message: format!("Invalid JSON: {e}"),
                    };
                    Self::send_response(&mut stream, &response).await?;
                    continue;
                }
            };

            // Enforce handshake: the very first request of any new
            // connection must carry the pre-shared auth token. Any
            // other request is rejected and the connection is closed.
            if !authenticated {
                let response = match &request {
                    DbRequest::Handshake { token } if token == &auth_token => {
                        authenticated = true;
                        DbResponse::HandshakeAck
                    }
                    DbRequest::Handshake { .. } => {
                        warn!(
                            tool = %tool_name,
                            "DB IPC handshake failed: bad token"
                        );
                        DbResponse::Error {
                            code: DbErrorCode::PermissionDenied,
                            message: "Invalid auth token".to_string(),
                        }
                    }
                    _ => {
                        warn!(
                            tool = %tool_name,
                            "DB IPC request before handshake; closing"
                        );
                        DbResponse::Error {
                            code: DbErrorCode::PermissionDenied,
                            message: "Handshake required".to_string(),
                        }
                    }
                };
                Self::send_response(&mut stream, &response).await?;
                if !authenticated {
                    break;
                }
                continue;
            }

            // Match the actual `Shutdown` request rather
            // than the response. The previous form closed
            // the connection on any `DbResponse::Ack`, which
            // is fragile: if any other request ever returns
            // `Ack` (or a future response is reused), the
            // connection would terminate spuriously.
            let was_shutdown = matches!(request, DbRequest::Shutdown);

            let response = Self::handle_request(
                &db,
                &tool_name,
                &prefix,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid,
                request,
            )
            .await;

            Self::send_response(&mut stream, &response).await?;

            if was_shutdown {
                debug!(tool = %tool_name, "Received shutdown request");
                break;
            }
        }

        Ok(())
    }

    async fn send_response(
        stream: &mut ene_plugin_proto::transport::IpcStream,
        response: &DbResponse,
    ) -> Result<(), DbServerError> {
        let json = serde_json::to_vec(response)?;
        let len = json.len() as u32;
        stream.write_all(&len.to_le_bytes()).await?;
        stream.write_all(&json).await?;
        stream.flush().await?;
        Ok(())
    }

    async fn handle_request(
        db: &DatabaseConnection,
        tool_name: &str,
        prefix: &str,
        declared_tables: &mut HashMap<String, DbTable>,
        declared_columns: &mut HashMap<String, HashSet<String>>,
        last_rowid: &Arc<std::sync::Mutex<Option<i64>>>,
        request: DbRequest,
    ) -> DbResponse {
        match request {
            DbRequest::DeclareSchema(schema) => Self::handle_declare_schema(
                db,
                tool_name,
                prefix,
                declared_tables,
                declared_columns,
                schema,
            )
            .await
            .unwrap_or_else(|e| e.to_error_response()),
            DbRequest::Insert { table, row } => {
                let res = Self::validate_table_access(declared_tables, &table)
                    .and_then(|()| Self::validate_row_columns(declared_columns, &table, &row));
                match res {
                    Ok(()) => match Self::handle_insert(db, &table, row).await {
                        Ok(id) => {
                            // Record the rowid for the
                            // connection-scoped
                            // `LastInsertRowId` lookup.
                            if let Ok(mut guard) = last_rowid.lock() {
                                *guard = Some(id);
                            }
                            DbResponse::Insert { rowid: id }
                        }
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Upsert {
                table,
                row,
                conflict_columns,
            } => {
                let res = Self::validate_table_access(declared_tables, &table)
                    .and_then(|()| Self::validate_row_columns(declared_columns, &table, &row))
                    .and_then(|()| {
                        Self::validate_conflict_columns(declared_columns, &table, &conflict_columns)
                    });
                match res {
                    Ok(()) => match Self::handle_upsert(db, &table, row, conflict_columns).await {
                        Ok(rowid) => DbResponse::Upsert { rowid },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Select {
                table,
                columns,
                filter,
                order_by,
                limit,
            } => {
                match Self::handle_select(
                    db,
                    declared_tables,
                    declared_columns,
                    &table,
                    columns,
                    filter,
                    order_by,
                    limit,
                )
                .await
                {
                    Ok(rows) => DbResponse::Select { rows },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Update { table, set, filter } => {
                let res = Self::validate_table_access(declared_tables, &table)
                    .and_then(|()| Self::validate_row_columns(declared_columns, &table, &set))
                    .and_then(|()| {
                        Self::validate_filter_columns(declared_columns, &table, &filter)
                    });
                match res {
                    Ok(()) => match Self::handle_update(db, &table, set, filter).await {
                        Ok(affected) => DbResponse::Update { affected },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Delete { table, filter } => {
                let res = Self::validate_table_access(declared_tables, &table).and_then(|()| {
                    Self::validate_filter_columns(declared_columns, &table, &filter)
                });
                match res {
                    Ok(()) => match Self::handle_delete(db, &table, filter).await {
                        Ok(affected) => DbResponse::Delete { affected },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Count { table, filter } => {
                let res = Self::validate_table_access(declared_tables, &table).and_then(|()| {
                    Self::validate_filter_columns(declared_columns, &table, &filter)
                });
                match res {
                    Ok(()) => match Self::handle_count(db, &table, filter).await {
                        Ok(count) => DbResponse::Count { count },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Batch { ops } => {
                match Self::handle_batch(db, declared_tables, declared_columns, last_rowid, ops)
                    .await
                {
                    Ok(results) => DbResponse::Batch { results },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::LastInsertRowId => {
                // Return the rowid of the most recent
                // Insert on this logical connection. SQLite's
                // `last_insert_rowid()` is per-connection, so
                // a raw `SELECT last_insert_rowid()` against
                // a SeaORM pool can return a stale or
                // different value than the prior insert.
                let rowid = last_rowid.lock().ok().and_then(|g| *g).unwrap_or(0);
                DbResponse::LastInsertRowId { rowid }
            }
            DbRequest::Ping => DbResponse::Pong,
            DbRequest::Shutdown => DbResponse::Ack,
            // The handshake is intercepted in `handle_connection`
            // before reaching this dispatch; reaching it here means
            // a second handshake was sent, which we reject.
            DbRequest::Handshake { .. } => DbResponse::Error {
                code: DbErrorCode::PermissionDenied,
                message: "Handshake already completed".to_string(),
            },
        }
    }

    /// Validates a single [`DbWriteOp`] against the declared schema without
    /// touching the database. Mirrors the per-request validation performed in
    /// [`handle_request`](Self::handle_request) for the standalone
    /// `Insert`/`Upsert`/`Update`/`Delete` requests, so a batch is rejected
    /// before any statement runs.
    fn validate_write_op(
        declared_tables: &HashMap<String, DbTable>,
        declared_columns: &HashMap<String, HashSet<String>>,
        op: &DbWriteOp,
    ) -> Result<(), DbServerError> {
        match op {
            DbWriteOp::Insert { table, row } => Self::validate_table_access(declared_tables, table)
                .and_then(|()| Self::validate_row_columns(declared_columns, table, row)),
            DbWriteOp::Upsert {
                table,
                row,
                conflict_columns,
            } => Self::validate_table_access(declared_tables, table)
                .and_then(|()| Self::validate_row_columns(declared_columns, table, row))
                .and_then(|()| {
                    Self::validate_conflict_columns(declared_columns, table, conflict_columns)
                }),
            DbWriteOp::Update { table, set, filter } => {
                Self::validate_table_access(declared_tables, table)
                    .and_then(|()| Self::validate_row_columns(declared_columns, table, set))
                    .and_then(|()| Self::validate_filter_columns(declared_columns, table, filter))
            }
            DbWriteOp::Delete { table, filter } => {
                Self::validate_table_access(declared_tables, table)
                    .and_then(|()| Self::validate_filter_columns(declared_columns, table, filter))
            }
        }
    }

    /// Executes a [`DbRequest::Batch`] atomically.
    ///
    /// Every operation is validated against the declared schema up front, then
    /// applied inside a single `SQLite` transaction opened on `db`. If all
    /// operations succeed the transaction commits and one [`DbBatchOpResult`]
    /// per operation is returned in request order. If any operation fails, the
    /// transaction is rolled back and an error naming the failing operation is
    /// returned — nothing from the batch is persisted.
    ///
    /// The transaction is scoped entirely to this call: it is never held across
    /// IPC round-trips, so a plugin cannot pin the `SQLite` write lock open, and
    /// a dropped connection cannot leave a half-applied batch (the transaction
    /// either commits before the response is sent or rolls back on drop).
    async fn handle_batch(
        db: &DatabaseConnection,
        declared_tables: &HashMap<String, DbTable>,
        declared_columns: &HashMap<String, HashSet<String>>,
        last_rowid: &Arc<std::sync::Mutex<Option<i64>>>,
        ops: Vec<DbWriteOp>,
    ) -> Result<Vec<DbBatchOpResult>, DbServerError> {
        if ops.len() > MAX_BATCH_OPS {
            return Err(DbServerError::Internal(format!(
                "batch too large: {} operations exceeds the limit of {MAX_BATCH_OPS}",
                ops.len()
            )));
        }

        for (idx, op) in ops.iter().enumerate() {
            Self::validate_write_op(declared_tables, declared_columns, op).map_err(|e| {
                DbServerError::BatchOp {
                    index: idx,
                    source: Box::new(e),
                }
            })?;
        }

        let txn = db.begin().await.map_err(DbServerError::Db)?;

        let mut results = Vec::with_capacity(ops.len());
        for (idx, op) in ops.into_iter().enumerate() {
            let result = match op {
                DbWriteOp::Insert { table, row } => Self::handle_insert(&txn, &table, row)
                    .await
                    .map(|rowid| DbBatchOpResult::Insert { rowid }),
                DbWriteOp::Upsert {
                    table,
                    row,
                    conflict_columns,
                } => Self::handle_upsert(&txn, &table, row, conflict_columns)
                    .await
                    .map(|rowid| DbBatchOpResult::Upsert { rowid }),
                DbWriteOp::Update { table, set, filter } => {
                    Self::handle_update(&txn, &table, set, filter)
                        .await
                        .map(|affected| DbBatchOpResult::Update { affected })
                }
                DbWriteOp::Delete { table, filter } => Self::handle_delete(&txn, &table, filter)
                    .await
                    .map(|affected| DbBatchOpResult::Delete { affected }),
            };
            match result {
                Ok(res) => results.push(res),
                Err(e) => {
                    // Explicit rollback for a prompt, deterministic release of
                    // the write lock; dropping `txn` would roll back too.
                    if let Err(rollback_err) = txn.rollback().await {
                        // Surface the rollback failure but keep the original
                        // statement error as the primary one.
                        error!(
                            error = %rollback_err,
                            "failed to roll back batch transaction after statement error"
                        );
                    }
                    return Err(DbServerError::BatchOp {
                        index: idx,
                        source: Box::new(e),
                    });
                }
            }
        }

        txn.commit().await.map_err(DbServerError::Db)?;

        // Mirror the standalone `Insert` behavior: only inserts update the
        // connection-scoped `LastInsertRowId` cell (standalone `Upsert`
        // deliberately does not — an upsert that resolves via the
        // conflict-update path performs no insert, so its `last_insert_id()`
        // would be stale). Record the batch's final insert rowid so the tool's
        // view stays consistent.
        {
            // A poisoned mutex means a panic happened while holding the lock;
            // recover the value rather than silently skipping the update.
            let mut guard = last_rowid
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for res in results.iter().rev() {
                if let DbBatchOpResult::Insert { rowid } = res {
                    *guard = Some(*rowid);
                    break;
                }
            }
        }

        Ok(results)
    }

    fn validate_table_access(
        declared_tables: &HashMap<String, DbTable>,
        table: &str,
    ) -> Result<(), DbServerError> {
        if table.starts_with("sqlite_") || table == "__tool_schemas" {
            return Err(DbServerError::PermissionDenied(format!(
                "Access to internal table '{table}' is not allowed"
            )));
        }

        if !declared_tables.contains_key(table) {
            return Err(DbServerError::UnknownTable(table.to_string()));
        }

        Ok(())
    }

    fn validate_row_columns(
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        row: &Row,
    ) -> Result<(), DbServerError> {
        let table_columns = declared_columns
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        for col in row.keys() {
            if !table_columns.contains(col) {
                return Err(DbServerError::UnknownColumn {
                    table: table.to_string(),
                    column: col.clone(),
                });
            }
        }

        Ok(())
    }

    /// Validates that every column default in a schema declaration can be
    /// rendered as a `SQLite` literal (#427).
    ///
    /// Defaults are the one value path that bypasses `sea-query`'s parameter
    /// binding (they are embedded in the DDL), so a value with no valid
    /// literal — a non-finite float — must be rejected up front. Running this
    /// before any database write keeps a rejected declaration from leaving a
    /// stored `__tool_schemas` row ahead of the physical tables.
    fn validate_column_defaults(schema: &DbSchema) -> Result<(), DbServerError> {
        for table in &schema.tables {
            for col in &table.columns {
                if let Some(default) = &col.default {
                    Self::db_value_to_sql(default)?;
                }
            }
        }
        Ok(())
    }

    /// Validates that every upsert conflict column is declared for `table`.
    ///
    /// The other five dispatch paths all cross-check the identifiers they
    /// reference against the declared schema; an upsert's `conflict_columns`
    /// was the one identifier set that skipped this check (#427). The values
    /// are quoted by `sea-query`'s `Alias`, so this is not an injection hole,
    /// but it breaks the "only declared columns may be referenced" invariant
    /// the rest of the server enforces — an undeclared conflict column would
    /// reach the database and fail there with an opaque error instead of a
    /// structured `UnknownColumn`.
    fn validate_conflict_columns(
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        conflict_columns: &[String],
    ) -> Result<(), DbServerError> {
        let table_columns = declared_columns
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        for col in conflict_columns {
            if !table_columns.contains(col) {
                return Err(DbServerError::UnknownColumn {
                    table: table.to_string(),
                    column: col.clone(),
                });
            }
        }

        Ok(())
    }

    fn validate_select_columns(
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        columns: &[String],
    ) -> Result<(), DbServerError> {
        if columns.is_empty() {
            return Ok(());
        }

        let table_columns = declared_columns
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        for col in columns {
            if !table_columns.contains(col) {
                return Err(DbServerError::UnknownColumn {
                    table: table.to_string(),
                    column: col.clone(),
                });
            }
        }

        Ok(())
    }

    fn validate_filter_columns(
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        filter: &DbFilter,
    ) -> Result<(), DbServerError> {
        let table_columns = declared_columns
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        for col in filter.columns_referenced() {
            if !table_columns.contains(col) {
                return Err(DbServerError::UnknownColumn {
                    table: table.to_string(),
                    column: col.to_string(),
                });
            }
        }

        Ok(())
    }

    fn validate_order_by_columns(
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        order_by: &[DbOrderBy],
    ) -> Result<(), DbServerError> {
        let table_columns = declared_columns
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        for order in order_by {
            if !table_columns.contains(&order.column) {
                return Err(DbServerError::UnknownColumn {
                    table: table.to_string(),
                    column: order.column.clone(),
                });
            }
        }

        Ok(())
    }

    async fn handle_declare_schema(
        db: &DatabaseConnection,
        tool_name: &str,
        prefix: &str,
        declared_tables: &mut HashMap<String, DbTable>,
        declared_columns: &mut HashMap<String, HashSet<String>>,
        schema: DbSchema,
    ) -> Result<DbResponse, DbServerError> {
        if schema.prefix != prefix {
            return Err(DbServerError::PermissionDenied(format!(
                "Schema prefix '{}' does not match tool prefix '{}'",
                schema.prefix, prefix
            )));
        }

        for table in &schema.tables {
            if !table.name.starts_with(prefix) {
                return Err(DbServerError::PermissionDenied(format!(
                    "Table '{}' does not start with prefix '{}'",
                    table.name, prefix
                )));
            }
            Self::validate_identifier(&table.name)?;
            for col in &table.columns {
                Self::validate_identifier(&col.name)?;
            }
        }

        // Reject column defaults that cannot be rendered as a SQLite literal
        // (currently only non-finite floats) before any database write, so a
        // bad declaration never leaves a stored `__tool_schemas` row ahead of
        // the physical tables (#427).
        Self::validate_column_defaults(&schema)?;

        let known_table_names: HashSet<&str> =
            schema.tables.iter().map(|t| t.name.as_str()).collect();
        for index in &schema.indexes {
            Self::validate_identifier(&index.name)?;
            Self::validate_identifier(&index.table)?;
            // SQLite index names share a single database-wide namespace, so an
            // unprefixed index could squat a name a core migration later needs
            // (e.g. `idx_typed_mem_kind`). Requiring the tool's prefix to
            // appear in the index name keeps each tool's indexes inside its
            // own namespace. The prefix may appear anywhere in the name — the
            // established convention is `idx_<prefix>...` — so this stays
            // compatible with existing declarations while still rejecting
            // names that carry no prefix at all (#427).
            if !index.name.contains(prefix) {
                return Err(DbServerError::PermissionDenied(format!(
                    "Index '{}' does not carry prefix '{}'",
                    index.name, prefix
                )));
            }
            if !index.table.starts_with(prefix) {
                return Err(DbServerError::PermissionDenied(format!(
                    "Index '{}' references table '{}' outside prefix '{}'",
                    index.name, index.table, prefix
                )));
            }
            if !known_table_names.contains(index.table.as_str()) {
                return Err(DbServerError::UnknownTable(format!(
                    "Index '{}' references undeclared table '{}'",
                    index.name, index.table
                )));
            }
            for col in &index.columns {
                Self::validate_identifier(col)?;
            }
        }

        let schema_json = serde_json::to_string(&schema)
            .map_err(|e| DbServerError::Internal(format!("Failed to serialize schema: {e}")))?;

        let fingerprint = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

        // Compare against the previously stored declaration, if any. The
        // fingerprint was historically written but never read (#423); it now
        // gates whether the stored row is rewritten and whether the physical
        // tables need additive migration.
        let stored = tool_schemas::Entity::find_by_id(prefix)
            .one(db)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let mut created_indexes = Vec::new();

        match &stored {
            // Unchanged schema: reuse the existing tables verbatim and leave
            // the stored row untouched so an unchanged declaration does not
            // rewrite it needlessly.
            Some(row) if row.fingerprint == fingerprint => {
                debug!(
                    tool = %tool_name,
                    prefix = %prefix,
                    "Schema unchanged; reusing stored declaration"
                );
            }
            // First declaration, or the schema changed: reconcile the
            // physical tables with the new declaration, then refresh the
            // stored row. Both steps run in a single transaction so a
            // failure part-way through cannot leave the physical tables
            // ahead of the stored fingerprint (which would make the next
            // DeclareSchema re-apply — and fail on — the same ALTERs).
            _ => {
                let txn = db
                    .begin()
                    .await
                    .map_err(|e| DbServerError::Internal(e.to_string()))?;

                let result = async {
                    let previous = stored
                        .as_ref()
                        .map(|row| {
                            serde_json::from_str::<DbSchema>(&row.schema_json).map_err(|e| {
                                DbServerError::Internal(format!(
                                    "stored schema for prefix '{prefix}' is corrupt: {e}"
                                ))
                            })
                        })
                        .transpose()?;

                    Self::apply_schema_change(&txn, tool_name, prefix, previous.as_ref(), &schema)
                        .await?;

                    let active_model = tool_schemas::ActiveModel {
                        prefix: sea_orm::ActiveValue::Set(prefix.to_string()),
                        schema_json: sea_orm::ActiveValue::Set(schema_json.clone()),
                        fingerprint: sea_orm::ActiveValue::Set(fingerprint.clone()),
                        created_at: sea_orm::ActiveValue::Set(chrono::Utc::now()),
                    };

                    tool_schemas::Entity::insert(active_model)
                        .on_conflict(
                            sea_orm::sea_query::OnConflict::column(tool_schemas::Column::Prefix)
                                .update_columns([
                                    tool_schemas::Column::SchemaJson,
                                    tool_schemas::Column::Fingerprint,
                                    tool_schemas::Column::CreatedAt,
                                ])
                                .to_owned(),
                        )
                        .exec(&txn)
                        .await
                        .map_err(|e| DbServerError::Internal(e.to_string()))?;

                    Ok::<_, DbServerError>(())
                }
                .await;

                match result {
                    Ok(()) => txn
                        .commit()
                        .await
                        .map_err(|e| DbServerError::Internal(e.to_string()))?,
                    Err(e) => {
                        txn.rollback().await.map_err(|rb| {
                            DbServerError::Internal(format!("{e}; rollback: {rb}"))
                        })?;
                        return Err(e);
                    }
                }
            }
        }

        // Idempotent regardless of the branch above: `CREATE TABLE IF NOT
        // EXISTS` / `CREATE INDEX IF NOT EXISTS` are no-ops on existing
        // objects and create any that are missing.
        for table in &schema.tables {
            let create_sql = Self::build_create_table_sql(table)?;
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, create_sql))
                .await
                .map_err(|e| DbServerError::Internal(e.to_string()))?;

            for index in &schema.indexes {
                if index.table == table.name {
                    let create_index_sql = Self::build_create_index_sql(index);
                    db.execute_raw(Statement::from_string(
                        DatabaseBackend::Sqlite,
                        create_index_sql,
                    ))
                    .await
                    .map_err(|e| DbServerError::Internal(e.to_string()))?;
                    created_indexes.push(index.name.clone());
                }
            }
        }

        for table in &schema.tables {
            let columns: HashSet<String> = table.columns.iter().map(|c| c.name.clone()).collect();
            declared_columns.insert(table.name.clone(), columns);
            declared_tables.insert(table.name.clone(), table.clone());
        }

        info!(
            tool = %tool_name,
            prefix = %prefix,
            tables = schema.tables.len(),
            "Schema declared"
        );

        Ok(DbResponse::SchemaAccepted {
            tables: schema.tables.iter().map(|t| t.name.clone()).collect(),
            indexes: created_indexes,
        })
    }

    /// Reconciles the physical tables with a changed schema declaration.
    ///
    /// `previous` is the last stored declaration for this prefix, if any.
    /// Additive changes (new tables, new columns) are applied in place;
    /// changes `SQLite` cannot perform in place (a column type change, or a
    /// previously declared table removed from the declaration) are rejected
    /// with [`DbServerError::SchemaConflict`] so the validation layer and the
    /// actual tables never silently diverge.
    ///
    /// `db` is generic over [`ConnectionTrait`] so the caller can run this
    /// inside a transaction: the ALTERs and the stored-declaration upsert
    /// must commit atomically, otherwise a failure between them would leave
    /// the physical table ahead of the stored fingerprint and every
    /// subsequent `DeclareSchema` would re-apply (and fail on) the same
    /// ALTER.
    async fn apply_schema_change(
        db: &impl ConnectionTrait,
        tool_name: &str,
        prefix: &str,
        previous: Option<&DbSchema>,
        new: &DbSchema,
    ) -> Result<(), DbServerError> {
        let Some(previous) = previous else {
            // First declaration for this prefix: nothing to migrate. The
            // caller's `CREATE TABLE IF NOT EXISTS` pass creates the tables.
            return Ok(());
        };

        let diff = Self::diff_schemas(previous, new)?;

        if diff.added_columns.is_empty() && diff.added_tables.is_empty() {
            // The fingerprint differs but there is nothing additive to apply
            // (e.g. only an index changed). Indexes are reconciled by the
            // caller's `CREATE INDEX IF NOT EXISTS` pass.
            return Ok(());
        }

        for (table, column) in &diff.added_columns {
            let alter_sql = Self::build_alter_add_column_sql(table, column)?;
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, alter_sql))
                .await
                .map_err(|e| {
                    DbServerError::Internal(format!(
                        "failed to add column '{}.{}': {e}",
                        table.name, column.name
                    ))
                })?;
            info!(
                tool = %tool_name,
                prefix = %prefix,
                table = %table.name,
                column = %column.name,
                "Applied additive schema change (ALTER TABLE ADD COLUMN)"
            );
        }

        for table in &diff.added_tables {
            info!(
                tool = %tool_name,
                prefix = %prefix,
                table = %table.name,
                "New table declared; will be created"
            );
        }

        Ok(())
    }

    /// Computes the additive diff between a stored schema and a new one.
    ///
    /// Returns the tables and columns that were added. Returns
    /// [`DbServerError::SchemaConflict`] if the new declaration removes a
    /// previously declared table or changes an existing column's type, since
    /// neither can be applied to the live `SQLite` tables in place.
    fn diff_schemas(previous: &DbSchema, new: &DbSchema) -> Result<SchemaDiff, DbServerError> {
        let previous_tables: HashMap<&str, &DbTable> = previous
            .tables
            .iter()
            .map(|t| (t.name.as_str(), t))
            .collect();

        let mut diff = SchemaDiff::default();

        for table in &new.tables {
            let Some(old_table) = previous_tables.get(table.name.as_str()) else {
                diff.added_tables.push(table.clone());
                continue;
            };

            let old_columns: HashMap<&str, &DbColumn> = old_table
                .columns
                .iter()
                .map(|c| (c.name.as_str(), c))
                .collect();

            for column in &table.columns {
                match old_columns.get(column.name.as_str()) {
                    // Brand-new column. `ALTER TABLE ADD COLUMN` can only add
                    // a plain column: SQLite forbids adding one that carries a
                    // PRIMARY KEY, UNIQUE, or AUTOINCREMENT constraint, so such
                    // an addition is rejected explicitly rather than failing
                    // with a cryptic error at execution time.
                    None => {
                        if column.primary_key || column.auto_increment || column.unique {
                            return Err(DbServerError::SchemaConflict(format!(
                                "column '{}.{}' was added with a PRIMARY KEY/UNIQUE/AUTOINCREMENT constraint; SQLite cannot add such a column via ALTER TABLE, so the plugin must reconcile this change explicitly",
                                table.name, column.name
                            )));
                        }
                        // A NOT NULL column needs a DEFAULT: SQLite requires a
                        // default to populate pre-existing rows when a
                        // non-nullable column is added via `ALTER TABLE`.
                        // Silently dropping the constraint (as the previous
                        // emission logic did) would let validation and storage
                        // diverge — refuse instead so the author reconciles
                        // this explicitly.
                        if !column.nullable && column.default.is_none() {
                            return Err(DbServerError::SchemaConflict(format!(
                                "column '{}.{}' was added NOT NULL without a DEFAULT; SQLite cannot add such a column via ALTER TABLE (existing rows would violate the constraint), so the plugin must either add a DEFAULT or make the column nullable",
                                table.name, column.name
                            )));
                        }
                        diff.added_columns.push((table.clone(), column.clone()));
                    }
                    // Existing column with a changed type: SQLite cannot
                    // change a column type in place, so refuse rather than
                    // let validation and storage diverge.
                    Some(old_column) if old_column.ty != column.ty => {
                        return Err(DbServerError::SchemaConflict(format!(
                            "column '{}.{}' changed type from {:?} to {:?}; SQLite cannot alter column types in place, so the plugin must reconcile this change explicitly",
                            table.name, column.name, old_column.ty, column.ty
                        )));
                    }
                    Some(_) => {}
                }
            }

            // A column present in the stored table but absent from the new
            // declaration would leave the physical column behind while
            // validation stops referencing it. Refuse so the author handles
            // the removal explicitly.
            let new_column_names: HashSet<&str> =
                table.columns.iter().map(|c| c.name.as_str()).collect();
            for old_column in &old_table.columns {
                if !new_column_names.contains(old_column.name.as_str()) {
                    return Err(DbServerError::SchemaConflict(format!(
                        "column '{}.{}' was removed from the schema declaration; SQLite cannot drop columns in place, so the plugin must reconcile this change explicitly",
                        table.name, old_column.name
                    )));
                }
            }
        }

        // A table present in the stored declaration but absent from the new
        // one would leave the physical table behind while validation stops
        // referencing it. Refuse so the author handles the removal explicitly.
        let new_table_names: HashSet<&str> = new.tables.iter().map(|t| t.name.as_str()).collect();
        for old_table in &previous.tables {
            if !new_table_names.contains(old_table.name.as_str()) {
                return Err(DbServerError::SchemaConflict(format!(
                    "table '{}' was removed from the schema declaration; SQLite cannot drop tables in place, so the plugin must reconcile this change explicitly",
                    old_table.name
                )));
            }
        }

        Ok(diff)
    }

    /// Builds an `ALTER TABLE ... ADD COLUMN` statement for an added column.
    ///
    /// `SQLite` forbids adding a column that carries a `PRIMARY KEY`,
    /// `UNIQUE`, or `AUTOINCREMENT` constraint, so none are emitted here;
    /// [`Self::diff_schemas`] rejects such additions before this is reached.
    /// `NOT NULL` is only emitted alongside a `DEFAULT`, matching `SQLite`'s
    /// `ADD COLUMN` requirement that a non-nullable added column have a
    /// default to populate existing rows.
    fn build_alter_add_column_sql(
        table: &DbTable,
        column: &DbColumn,
    ) -> Result<String, DbServerError> {
        let mut sql = format!("ALTER TABLE {} ADD COLUMN {} ", table.name, column.name);

        sql.push_str(match column.ty {
            DbType::Integer | DbType::Boolean => "INTEGER",
            DbType::Real => "REAL",
            DbType::Text => "TEXT",
            DbType::Blob => "BLOB",
        });

        // SQLite requires a DEFAULT for a NOT NULL column added via ALTER.
        if !column.nullable && column.default.is_some() {
            sql.push_str(" NOT NULL");
        }

        if let Some(default) = &column.default {
            sql.push_str(" DEFAULT ");
            sql.push_str(&Self::db_value_to_sql(default)?);
        }

        Ok(sql)
    }

    /// Validates a SQL identifier (table, column, or index name).
    ///
    /// Rejects empty strings, names longer than 64 characters, and any
    /// character outside `[A-Za-z0-9_]` with the first character in
    /// `[A-Za-z_]`. This prevents SQL injection through identifier
    /// interpolation in DDL construction.
    #[expect(
        clippy::indexing_slicing,
        reason = "indexes into validated identifier bytes after length check"
    )]
    fn validate_identifier(name: &str) -> Result<(), DbServerError> {
        if name.is_empty() {
            return Err(DbServerError::PermissionDenied(
                "Identifier must not be empty".to_string(),
            ));
        }
        if name.len() > 64 {
            return Err(DbServerError::PermissionDenied(format!(
                "Identifier '{name}' exceeds 64-character limit"
            )));
        }
        let bytes = name.as_bytes();
        if !matches!(bytes[0], b'A'..=b'Z' | b'a'..=b'z' | b'_') {
            return Err(DbServerError::PermissionDenied(format!(
                "Identifier '{name}' must start with a letter or underscore"
            )));
        }
        for &b in &bytes[1..] {
            if !matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_') {
                return Err(DbServerError::PermissionDenied(format!(
                    "Identifier '{name}' contains invalid character"
                )));
            }
        }
        Ok(())
    }

    fn build_create_table_sql(table: &DbTable) -> Result<String, DbServerError> {
        let mut sql = format!("CREATE TABLE IF NOT EXISTS {} (", table.name);

        let mut first = true;
        for col in &table.columns {
            if !first {
                sql.push_str(", ");
            }
            first = false;

            sql.push_str(&col.name);
            sql.push(' ');
            sql.push_str(match col.ty {
                ene_plugin_db::DbType::Integer | ene_plugin_db::DbType::Boolean => "INTEGER",
                ene_plugin_db::DbType::Real => "REAL",
                ene_plugin_db::DbType::Text => "TEXT",
                ene_plugin_db::DbType::Blob => "BLOB",
            });

            if !col.nullable {
                sql.push_str(" NOT NULL");
            }

            if col.primary_key {
                sql.push_str(" PRIMARY KEY");
            }

            if col.auto_increment {
                sql.push_str(" AUTOINCREMENT");
            }

            if col.unique {
                sql.push_str(" UNIQUE");
            }

            if let Some(default) = &col.default {
                sql.push_str(" DEFAULT ");
                sql.push_str(&Self::db_value_to_sql(default)?);
            }
        }

        sql.push(')');
        Ok(sql)
    }

    /// Renders a [`DbValue`] as a `SQLite` literal for embedding in hand-built
    /// DDL (a column `DEFAULT` clause).
    ///
    /// DDL defaults are the one place values bypass `sea-query`'s parameter
    /// binding, so each variant is rendered as a literal. A non-finite float
    /// (`NaN` or infinity) has no valid `SQLite` literal — `f64::to_string`
    /// yields `"NaN"` / `"inf"`, which is a syntax error — so it is rejected
    /// here with [`DbServerError::PermissionDenied`] rather than producing a
    /// `DeclareSchema` failure with an opaque database error (#427).
    fn db_value_to_sql(value: &DbValue) -> Result<String, DbServerError> {
        match value {
            DbValue::Null => Ok("NULL".to_string()),
            DbValue::Bool(b) => Ok(if *b { "1" } else { "0" }.to_string()),
            DbValue::Int(i) => Ok(i.to_string()),
            DbValue::Float(f) => {
                if f.is_finite() {
                    Ok(f.to_string())
                } else {
                    Err(DbServerError::PermissionDenied(format!(
                        "column DEFAULT value {f} is not finite; SQLite has no literal for NaN or infinity"
                    )))
                }
            }
            DbValue::Text(s) => Ok(format!("'{}'", s.replace('\'', "''"))),
            DbValue::Blob(b) => {
                use std::fmt::Write;
                let mut hex = String::with_capacity(b.len().saturating_mul(2).saturating_add(3));
                hex.push_str("X'");
                for byte in b {
                    // `fmt::Error` is `Copy`, so `drop()` would itself trip
                    // `clippy::dropping_copy_types`; writing into a `String`
                    // via `fmt::Write` never actually fails.
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "fmt::Write to a String is infallible in practice"
                    )]
                    let _ = write!(hex, "{byte:02x}");
                }
                hex.push('\'');
                Ok(hex)
            }
        }
    }

    fn build_create_index_sql(index: &ene_plugin_db::DbIndex) -> String {
        let columns = index.columns.join(", ");
        let unique = if index.unique { "UNIQUE " } else { "" };
        format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({})",
            index.name, index.table, columns
        )
    }

    fn db_value_to_sea_value(val: &DbValue) -> sea_orm::Value {
        match val {
            DbValue::Null => sea_orm::Value::from(None::<i32>),
            DbValue::Bool(b) => sea_orm::Value::from(*b),
            DbValue::Int(i) => sea_orm::Value::from(*i),
            DbValue::Float(f) => sea_orm::Value::from(*f),
            DbValue::Text(s) => sea_orm::Value::from(s.clone()),
            DbValue::Blob(b) => sea_orm::Value::from(b.clone()),
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "indexes into request-owned row map with validated column keys"
    )]
    async fn handle_insert(
        db: &impl ConnectionTrait,
        table: &str,
        row: Row,
    ) -> Result<i64, DbServerError> {
        let mut insert = Query::insert();
        insert.into_table(Alias::new(table));

        let columns: Vec<String> = row.keys().cloned().collect();
        insert.columns(columns.iter().map(Alias::new));

        let values: Vec<sea_orm::sea_query::SimpleExpr> = columns
            .iter()
            .map(|k| Self::db_value_to_sea_value(&row[k]).into())
            .collect();

        insert
            .values(values)
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let (sql, params) = insert.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let exec_res = db
            .execute_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.last_insert_id() as i64)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "indexes into request-owned row map with validated column keys"
    )]
    async fn handle_upsert(
        db: &impl ConnectionTrait,
        table: &str,
        row: Row,
        conflict_columns: Vec<String>,
    ) -> Result<i64, DbServerError> {
        let mut insert = Query::insert();
        insert.into_table(Alias::new(table));

        let columns: Vec<String> = row.keys().cloned().collect();
        insert.columns(columns.iter().map(Alias::new));

        let values: Vec<sea_orm::sea_query::SimpleExpr> = columns
            .iter()
            .map(|k| Self::db_value_to_sea_value(&row[k]).into())
            .collect();

        insert
            .values(values)
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let mut on_conflict =
            sea_orm::sea_query::OnConflict::columns(conflict_columns.iter().map(Alias::new));

        let update_cols: Vec<Alias> = columns
            .iter()
            .filter(|c| !conflict_columns.contains(c))
            .map(Alias::new)
            .collect();

        if !update_cols.is_empty() {
            on_conflict.update_columns(update_cols);
            insert.on_conflict(on_conflict);
        }

        let (sql, params) = insert.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let exec_res = db
            .execute_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.last_insert_id() as i64)
    }

    async fn handle_select(
        db: &DatabaseConnection,
        declared_tables: &HashMap<String, DbTable>,
        declared_columns: &HashMap<String, HashSet<String>>,
        table: &str,
        columns: Vec<String>,
        filter: DbFilter,
        order_by: Vec<DbOrderBy>,
        limit: Option<u64>,
    ) -> Result<Vec<Row>, DbServerError> {
        Self::validate_table_access(declared_tables, table)?;
        Self::validate_select_columns(declared_columns, table, &columns)?;
        Self::validate_filter_columns(declared_columns, table, &filter)?;
        Self::validate_order_by_columns(declared_columns, table, &order_by)?;

        let mut select = Query::select();
        select.from(Alias::new(table));

        if columns.is_empty() {
            select.column(sea_orm::sea_query::Asterisk);
        } else {
            for col in &columns {
                select.column(Alias::new(col));
            }
        }

        let cond = Self::build_sea_query_filter(&filter)?;
        select.cond_where(cond);

        for o in &order_by {
            let order = if matches!(o.direction, ene_plugin_db::DbOrderDirection::Desc) {
                sea_orm::sea_query::Order::Desc
            } else {
                sea_orm::sea_query::Order::Asc
            };
            select.order_by(Alias::new(&o.column), order);
        }

        if let Some(limit) = limit {
            select.limit(limit);
        }

        let (sql, params) = select.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let query_results = db
            .query_all_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let cols_to_fetch = if columns.is_empty() {
            declared_columns.get(table).cloned().unwrap_or_default()
        } else {
            columns.into_iter().collect::<HashSet<_>>()
        };

        let table_def = declared_tables
            .get(table)
            .ok_or_else(|| DbServerError::UnknownTable(table.to_string()))?;

        let col_def_map: HashMap<&str, &ene_plugin_db::DbColumn> = table_def
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        let mut rows = Vec::new();
        for result in query_results {
            let mut row = Row::new();
            for col in &cols_to_fetch {
                let col_def = col_def_map.get(col.as_str());
                let val = if let Some(def) = col_def {
                    match def.ty {
                        ene_plugin_db::DbType::Integer => {
                            match result.try_get::<Option<i64>>("", col) {
                                Ok(Some(v)) => DbValue::Int(v),
                                Ok(None) => DbValue::Null,
                                Err(e) => {
                                    log_select_type_mismatch(table, col, "INTEGER", &e);
                                    DbValue::Null
                                }
                            }
                        }
                        ene_plugin_db::DbType::Real => match result.try_get::<Option<f64>>("", col)
                        {
                            Ok(Some(v)) => DbValue::Float(v),
                            Ok(None) => DbValue::Null,
                            Err(e) => {
                                log_select_type_mismatch(table, col, "REAL", &e);
                                DbValue::Null
                            }
                        },
                        ene_plugin_db::DbType::Text => {
                            match result.try_get::<Option<String>>("", col) {
                                Ok(Some(v)) => DbValue::Text(v),
                                Ok(None) => DbValue::Null,
                                Err(e) => {
                                    log_select_type_mismatch(table, col, "TEXT", &e);
                                    DbValue::Null
                                }
                            }
                        }
                        ene_plugin_db::DbType::Blob => {
                            match result.try_get::<Option<Vec<u8>>>("", col) {
                                Ok(Some(v)) => DbValue::Blob(v),
                                Ok(None) => DbValue::Null,
                                Err(e) => {
                                    log_select_type_mismatch(table, col, "BLOB", &e);
                                    DbValue::Null
                                }
                            }
                        }
                        ene_plugin_db::DbType::Boolean => {
                            // SQLite stores booleans as integers, so a failed
                            // `bool` decode is expected and retried as `i64`
                            // without logging; only a failure of both decodes
                            // indicates a genuine type mismatch.
                            match result.try_get::<Option<bool>>("", col) {
                                Ok(Some(v)) => DbValue::Bool(v),
                                Ok(None) => DbValue::Null,
                                Err(_) => match result.try_get::<Option<i64>>("", col) {
                                    Ok(Some(v)) => DbValue::Bool(v != 0),
                                    Ok(None) => DbValue::Null,
                                    Err(e) => {
                                        log_select_type_mismatch(table, col, "BOOLEAN", &e);
                                        DbValue::Null
                                    }
                                },
                            }
                        }
                    }
                } else {
                    DbValue::Null
                };
                row.insert(col.clone(), val);
            }
            rows.push(row);
        }

        Ok(rows)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "indexes into request-owned set map with validated column keys"
    )]
    async fn handle_update(
        db: &impl ConnectionTrait,
        table: &str,
        set: Row,
        filter: DbFilter,
    ) -> Result<u64, DbServerError> {
        let mut update = Query::update();
        update.table(Alias::new(table));

        let columns: Vec<String> = set.keys().cloned().collect();
        let values: Vec<sea_orm::Value> = columns
            .iter()
            .map(|k| Self::db_value_to_sea_value(&set[k]))
            .collect();

        for (col, val) in columns.iter().zip(values) {
            update.value(Alias::new(col), Expr::val(val));
        }

        let cond = Self::build_sea_query_filter(&filter)?;
        update.cond_where(cond);

        let (sql, params) = update.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let exec_res = db
            .execute_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.rows_affected())
    }

    async fn handle_delete(
        db: &impl ConnectionTrait,
        table: &str,
        filter: DbFilter,
    ) -> Result<u64, DbServerError> {
        let mut delete = Query::delete();
        delete.from_table(Alias::new(table));

        let cond = Self::build_sea_query_filter(&filter)?;
        delete.cond_where(cond);

        let (sql, params) = delete.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let exec_res = db
            .execute_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.rows_affected())
    }

    async fn handle_count(
        db: &DatabaseConnection,
        table: &str,
        filter: DbFilter,
    ) -> Result<i64, DbServerError> {
        let mut select = Query::select();
        select.from(Alias::new(table));
        select.expr_as(Expr::cust("COUNT(*)"), Alias::new("count"));

        let cond = Self::build_sea_query_filter(&filter)?;
        select.cond_where(cond);

        let (sql, params) = select.build(SqliteQueryBuilder);
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, params);

        let res = db
            .query_one_raw(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let count: i64 = match res {
            Some(row) => row.try_get("", "count").unwrap_or(0),
            None => 0,
        };

        Ok(count)
    }

    fn build_sea_query_filter(filter: &DbFilter) -> Result<Condition, DbServerError> {
        match filter {
            DbFilter::Always => Ok(Condition::all()),
            DbFilter::And(filters) => {
                let mut cond = Condition::all();
                for f in filters {
                    cond = cond.add(Self::build_sea_query_filter(f)?);
                }
                Ok(cond)
            }
            DbFilter::Or(filters) => {
                let mut cond = Condition::any();
                for f in filters {
                    cond = cond.add(Self::build_sea_query_filter(f)?);
                }
                Ok(cond)
            }
            DbFilter::Not(f) => {
                let inner = Self::build_sea_query_filter(f)?;
                Ok(Condition::all().not().add(inner))
            }
            DbFilter::Eq { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).eq(Self::db_value_to_sea_value(value)))),
            DbFilter::Ne { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).ne(Self::db_value_to_sea_value(value)))),
            DbFilter::Lt { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).lt(Self::db_value_to_sea_value(value)))),
            DbFilter::Le { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).lte(Self::db_value_to_sea_value(value)))),
            DbFilter::Gt { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).gt(Self::db_value_to_sea_value(value)))),
            DbFilter::Ge { column, value } => Ok(Condition::all()
                .add(Expr::col(Alias::new(column)).gte(Self::db_value_to_sea_value(value)))),
            DbFilter::In { column, values } => {
                let sea_vals: Vec<sea_orm::Value> =
                    values.iter().map(Self::db_value_to_sea_value).collect();
                Ok(Condition::all().add(Expr::col(Alias::new(column)).is_in(sea_vals)))
            }
            DbFilter::Like { column, pattern } => {
                Ok(Condition::all().add(Expr::col(Alias::new(column)).like(pattern.clone())))
            }
            DbFilter::IsNull { column } => {
                Ok(Condition::all().add(Expr::col(Alias::new(column)).is_null()))
            }
            DbFilter::IsNotNull { column } => {
                Ok(Condition::all().add(Expr::col(Alias::new(column)).is_not_null()))
            }
        }
    }
}

/// Logs a column whose stored value could not be decoded as the type declared
/// in the tool's schema (#427).
///
/// `SQLite` is dynamically typed, so a column declared `INTEGER` can hold a
/// string; `handle_select` reads by the declared type and falls back to
/// [`DbValue::Null`] when the decode fails. Returning `Null` for data that is
/// actually present is confusing to debug from a plugin, so the mismatch is
/// surfaced here as a structured warning rather than swallowed silently.
fn log_select_type_mismatch(table: &str, column: &str, declared: &str, err: &sea_orm::DbErr) {
    warn!(
        component = "db_server",
        table = %table,
        column = %column,
        declared_type = %declared,
        error = %err,
        "Select column value did not decode as its declared type; returning NULL"
    );
}

/// Additive differences between a stored schema declaration and a new one.
///
/// Produced by [`DbIpcServer::diff_schemas`]; consumed by
/// [`DbIpcServer::apply_schema_change`] to migrate the physical tables in
/// place. Only additions are representable — removals and type changes are
/// reported as errors before a `SchemaDiff` is built.
#[derive(Debug, Default)]
struct SchemaDiff {
    /// Tables present in the new declaration but absent from the stored one.
    added_tables: Vec<DbTable>,
    /// Columns added to already-existing tables, paired with their table.
    added_columns: Vec<(DbTable, DbColumn)>,
}

#[cfg(test)]
#[expect(
    clippy::similar_names,
    clippy::iter_on_single_items,
    reason = "IPC test fixtures use paired request/response names and single-item iterators"
)]
mod tests {
    use super::*;

    #[test]
    fn validate_identifier_accepts_valid_names() {
        for name in [
            "fs_data",
            "_internal",
            "Table1",
            "a",
            "A_B_C_123",
            "x_1234567890_1234567890_1234567890_1234567890_1234567890_abcdefg",
        ] {
            assert!(
                DbIpcServer::validate_identifier(name).is_ok(),
                "expected '{name}' to be valid"
            );
        }
    }

    #[test]
    fn validate_identifier_rejects_injection_payloads() {
        // Each of these would be a valid SQL injection vector if interpolated
        // unescaped into a CREATE TABLE / CREATE INDEX statement.
        let bad_inputs = [
            "x) ; DROP TABLE fs_data; --",
            "evil\" TEXT --",
            "x'; DROP TABLE foo; --",
            "a; DROP TABLE users",
            "1col",          // starts with digit
            "with space",    // contains space
            "with-dash",     // contains dash
            "with`backtick", // contains backtick
            "",              // empty
            "table.name",    // contains dot
            "(select 1)",    // contains parens
            "x_1234567890_1234567890_1234567890_1234567890_1234567890_abcdefX1", // > 64
        ];
        for bad in bad_inputs {
            assert!(
                DbIpcServer::validate_identifier(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn build_create_table_sql_emits_valid_ddl() {
        let table = ene_plugin_db::DbTable {
            name: "fs_notes".to_string(),
            columns: vec![
                ene_plugin_db::DbColumn {
                    name: "id".to_string(),
                    ty: ene_plugin_db::DbType::Integer,
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    unique: false,
                    default: None,
                },
                ene_plugin_db::DbColumn {
                    name: "body".to_string(),
                    ty: ene_plugin_db::DbType::Text,
                    nullable: true,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
            ],
        };
        let sql = DbIpcServer::build_create_table_sql(&table).unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS fs_notes ("));
        assert!(sql.contains("id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT"));
        assert!(sql.contains("body TEXT"));
    }

    /// Regression test for #42: after an Insert, a
    /// `LastInsertRowId` request must return the rowid of
    /// that Insert — not 0 (the previous "no prior insert"
    /// sentinel) and not the rowid of some other insert
    /// served by a different pooled connection. The
    /// connection-scoped `last_rowid` cell records the
    /// rowid of the most recent Insert on this logical
    /// `handle_connection` and the dispatch returns it
    /// verbatim.
    #[tokio::test]
    async fn last_insert_rowid_returns_most_recent_insert() {
        use sea_orm::ConnectionTrait;
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE fs_test (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
        )
        .await
        .unwrap();

        let mut declared_tables: HashMap<String, ene_plugin_db::DbTable> = HashMap::new();
        declared_tables.insert(
            "fs_test".to_string(),
            ene_plugin_db::DbTable {
                name: "fs_test".to_string(),
                columns: vec![ene_plugin_db::DbColumn {
                    name: "name".to_string(),
                    ty: ene_plugin_db::DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                }],
            },
        );
        let mut declared_columns: HashMap<String, HashSet<String>> = HashMap::new();
        declared_columns.insert(
            "fs_test".to_string(),
            ["name".to_string()].into_iter().collect(),
        );

        let last_rowid: Arc<std::sync::Mutex<Option<i64>>> = Arc::new(std::sync::Mutex::new(None));

        // Insert a row.
        let insert_resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("first".to_string()),
                )]),
            },
        )
        .await;
        let first_id = match insert_resp {
            ene_plugin_db::DbResponse::Insert { rowid } => rowid,
            other => panic!("expected Insert, got {other:?}"),
        };
        assert!(first_id > 0);

        // LastInsertRowId must return the rowid we just got.
        let lookup = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::LastInsertRowId,
        )
        .await;
        let lookup_id = match lookup {
            ene_plugin_db::DbResponse::LastInsertRowId { rowid } => rowid,
            other => panic!("expected LastInsertRowId, got {other:?}"),
        };
        assert_eq!(lookup_id, first_id);

        // Insert a second row; LastInsertRowId now points
        // at the new row.
        let insert_resp2 = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("second".to_string()),
                )]),
            },
        )
        .await;
        let second_id = match insert_resp2 {
            ene_plugin_db::DbResponse::Insert { rowid } => rowid,
            other => panic!("expected Insert, got {other:?}"),
        };
        assert!(second_id > first_id);

        let lookup2 = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::LastInsertRowId,
        )
        .await;
        let lookup2_id = match lookup2 {
            ene_plugin_db::DbResponse::LastInsertRowId { rowid } => rowid,
            other => panic!("expected LastInsertRowId, got {other:?}"),
        };
        assert_eq!(lookup2_id, second_id);
    }

    /// Sets up an in-memory DB with a single pooled connection (mirroring the
    /// store's in-memory setup, so reads and writes hit the same connection),
    /// a declared `fs_test` table, and the connection-scoped state that
    /// [`DbIpcServer::handle_request`] threads through.
    async fn setup_batch_fixture() -> (
        sea_orm::DatabaseConnection,
        HashMap<String, ene_plugin_db::DbTable>,
        HashMap<String, HashSet<String>>,
        Arc<std::sync::Mutex<Option<i64>>>,
    ) {
        use sea_orm::ConnectionTrait;
        let mut opt = sea_orm::ConnectOptions::new("sqlite::memory:");
        opt.max_connections(1);
        let db = sea_orm::Database::connect(opt).await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE fs_test (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL UNIQUE, qty INTEGER)",
        )
        .await
        .unwrap();

        let mut declared_tables: HashMap<String, ene_plugin_db::DbTable> = HashMap::new();
        declared_tables.insert(
            "fs_test".to_string(),
            ene_plugin_db::DbTable {
                name: "fs_test".to_string(),
                columns: vec![
                    ene_plugin_db::DbColumn {
                        name: "id".to_string(),
                        ty: ene_plugin_db::DbType::Integer,
                        nullable: false,
                        primary_key: true,
                        auto_increment: true,
                        unique: false,
                        default: None,
                    },
                    ene_plugin_db::DbColumn {
                        name: "name".to_string(),
                        ty: ene_plugin_db::DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    ene_plugin_db::DbColumn {
                        name: "qty".to_string(),
                        ty: ene_plugin_db::DbType::Integer,
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                ],
            },
        );
        let mut declared_columns: HashMap<String, HashSet<String>> = HashMap::new();
        declared_columns.insert(
            "fs_test".to_string(),
            ["id".to_string(), "name".to_string(), "qty".to_string()]
                .into_iter()
                .collect(),
        );
        let last_rowid: Arc<std::sync::Mutex<Option<i64>>> = Arc::new(std::sync::Mutex::new(None));
        (db, declared_tables, declared_columns, last_rowid)
    }

    /// Counts rows in `fs_test` via the request dispatcher.
    async fn count_test_rows(
        db: &sea_orm::DatabaseConnection,
        declared_tables: &mut HashMap<String, ene_plugin_db::DbTable>,
        declared_columns: &mut HashMap<String, HashSet<String>>,
        last_rowid: &Arc<std::sync::Mutex<Option<i64>>>,
    ) -> i64 {
        match DbIpcServer::handle_request(
            db,
            "fs",
            "fs_",
            declared_tables,
            declared_columns,
            last_rowid,
            ene_plugin_db::DbRequest::Count {
                table: "fs_test".to_string(),
                filter: ene_plugin_db::DbFilter::Always,
            },
        )
        .await
        {
            ene_plugin_db::DbResponse::Count { count } => count,
            other => panic!("expected Count, got {other:?}"),
        }
    }

    /// A batch of valid writes commits atomically: every operation is applied
    /// and one [`ene_plugin_db::DbBatchOpResult`] is returned per operation, in
    /// request order.
    #[tokio::test]
    async fn batch_commits_all_ops_atomically() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([
                    (
                        "name".to_string(),
                        ene_plugin_db::DbValue::Text("alpha".to_string()),
                    ),
                    ("qty".to_string(), ene_plugin_db::DbValue::Int(1)),
                ]),
            },
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([
                    (
                        "name".to_string(),
                        ene_plugin_db::DbValue::Text("beta".to_string()),
                    ),
                    ("qty".to_string(), ene_plugin_db::DbValue::Int(2)),
                ]),
            },
            ene_plugin_db::DbWriteOp::Update {
                table: "fs_test".to_string(),
                set: std::collections::BTreeMap::from([(
                    "qty".to_string(),
                    ene_plugin_db::DbValue::Int(99),
                )]),
                filter: ene_plugin_db::DbFilter::eq("name", "alpha"),
            },
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;

        let results = match resp {
            ene_plugin_db::DbResponse::Batch { results } => results,
            other => panic!("expected Batch, got {other:?}"),
        };
        assert_eq!(results.len(), 3);
        assert!(
            matches!(results[0], ene_plugin_db::DbBatchOpResult::Insert { rowid } if rowid > 0)
        );
        assert!(
            matches!(results[1], ene_plugin_db::DbBatchOpResult::Insert { rowid } if rowid > 0)
        );
        assert_eq!(
            results[2],
            ene_plugin_db::DbBatchOpResult::Update { affected: 1 }
        );

        // Both inserts committed.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            2
        );
    }

    /// A failing statement rolls the whole batch back: the valid insert that
    /// preceded the NOT NULL violation is not persisted, and the error names
    /// the failing operation index.
    #[tokio::test]
    async fn batch_rolls_back_on_failing_statement() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            // Valid insert.
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("ok".to_string()),
                )]),
            },
            // Violates `name TEXT NOT NULL`; the whole batch must roll back.
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Null,
                )]),
            },
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;

        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::Internal);
                assert!(
                    message.contains("batch op 1"),
                    "error should name the failing op: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // The first (valid) insert must have been rolled back.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            0
        );
    }

    /// Schema validation runs before any statement executes, so a batch that
    /// references an undeclared table is rejected wholesale and nothing — not
    /// even the valid operations before it — is applied.
    #[tokio::test]
    async fn batch_rejects_undeclared_table_before_executing() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("ok".to_string()),
                )]),
            },
            ene_plugin_db::DbWriteOp::Delete {
                table: "fs_undeclared".to_string(),
                filter: ene_plugin_db::DbFilter::Always,
            },
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;

        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::UnknownTable);
                assert!(
                    message.contains("batch op 1"),
                    "error should name the failing op: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Validation precedes execution, so the valid insert never ran.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            0
        );
    }

    /// A committed batch updates the connection-scoped `LastInsertRowId` cell
    /// to the batch's final insert rowid, matching standalone `Insert` behavior.
    #[tokio::test]
    async fn batch_updates_last_insert_rowid() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("one".to_string()),
                )]),
            },
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("two".to_string()),
                )]),
            },
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;
        let results = match resp {
            ene_plugin_db::DbResponse::Batch { results } => results,
            other => panic!("expected Batch, got {other:?}"),
        };
        let second_rowid = match results[1] {
            ene_plugin_db::DbBatchOpResult::Insert { rowid } => rowid,
            ref other => panic!("expected Insert result, got {other:?}"),
        };

        let lookup = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::LastInsertRowId,
        )
        .await;
        let lookup_id = match lookup {
            ene_plugin_db::DbResponse::LastInsertRowId { rowid } => rowid,
            other => panic!("expected LastInsertRowId, got {other:?}"),
        };
        assert_eq!(lookup_id, second_rowid);
    }

    /// An empty batch is a no-op that commits successfully with no results.
    #[tokio::test]
    async fn empty_batch_commits_nothing() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops: vec![] },
        )
        .await;
        match resp {
            ene_plugin_db::DbResponse::Batch { results } => assert!(results.is_empty()),
            other => panic!("expected Batch, got {other:?}"),
        }
    }

    /// A batch larger than [`MAX_BATCH_OPS`] is rejected up front so a plugin
    /// cannot pin the write lock and memory for an arbitrarily long time.
    #[tokio::test]
    async fn oversized_batch_is_rejected() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("x".to_string()),
                )]),
            };
            MAX_BATCH_OPS + 1
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;
        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::Internal);
                assert!(
                    message.contains("batch too large"),
                    "expected size-limit error, got: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Nothing was persisted.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            0
        );
    }

    /// A batch op referencing an undeclared column preserves the structured
    /// `UnknownColumn` wire code (rather than being downgraded to `Internal`),
    /// so clients matching on the typed error still catch it, while the message
    /// names the failing operation index.
    #[tokio::test]
    async fn batch_unknown_column_preserves_error_code() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![ene_plugin_db::DbWriteOp::Insert {
            table: "fs_test".to_string(),
            row: std::collections::BTreeMap::from([(
                "no_such_column".to_string(),
                ene_plugin_db::DbValue::Text("x".to_string()),
            )]),
        }];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;

        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::UnknownColumn);
                assert!(
                    message.contains("batch op 0"),
                    "error should name the failing op: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// A committed batch whose final write is an upsert does **not** overwrite
    /// the connection-scoped `LastInsertRowId` cell: only inserts do, matching
    /// the standalone `Upsert` path (which never records a rowid, since an
    /// upsert resolving via conflict-update performs no insert and its
    /// `last_insert_id()` would be stale).
    #[tokio::test]
    async fn batch_upsert_does_not_update_last_insert_rowid() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![
            ene_plugin_db::DbWriteOp::Insert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("seed".to_string()),
                )]),
            },
            // Conflict-update on the existing `name = "seed"` row: no insert.
            ene_plugin_db::DbWriteOp::Upsert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([
                    (
                        "name".to_string(),
                        ene_plugin_db::DbValue::Text("seed".to_string()),
                    ),
                    ("qty".to_string(), ene_plugin_db::DbValue::Int(5)),
                ]),
                conflict_columns: vec!["name".to_string()],
            },
        ];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;
        let results = match resp {
            ene_plugin_db::DbResponse::Batch { results } => results,
            other => panic!("expected Batch, got {other:?}"),
        };
        let insert_rowid = match results[0] {
            ene_plugin_db::DbBatchOpResult::Insert { rowid } => rowid,
            ref other => panic!("expected Insert result, got {other:?}"),
        };

        // The upsert must not have clobbered the insert's rowid.
        let lookup = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::LastInsertRowId,
        )
        .await;
        let lookup_id = match lookup {
            ene_plugin_db::DbResponse::LastInsertRowId { rowid } => rowid,
            other => panic!("expected LastInsertRowId, got {other:?}"),
        };
        assert_eq!(lookup_id, insert_rowid);
    }

    // --- Schema fingerprint / evolution tests (#423) ---

    fn test_column(name: &str, ty: ene_plugin_db::DbType) -> ene_plugin_db::DbColumn {
        ene_plugin_db::DbColumn {
            name: name.to_string(),
            ty,
            nullable: true,
            primary_key: false,
            auto_increment: false,
            unique: false,
            default: None,
        }
    }

    fn test_schema_v1() -> ene_plugin_db::DbSchema {
        ene_plugin_db::DbSchema {
            prefix: "fs_".to_string(),
            tables: vec![ene_plugin_db::DbTable {
                name: "fs_undo".to_string(),
                columns: vec![
                    test_column("id", ene_plugin_db::DbType::Integer),
                    test_column("path", ene_plugin_db::DbType::Text),
                ],
            }],
            indexes: Vec::new(),
        }
    }

    /// Creates an in-memory DB with the `__tool_schemas` bookkeeping table.
    async fn test_db_with_tool_schemas() -> sea_orm::DatabaseConnection {
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE __tool_schemas (prefix TEXT PRIMARY KEY NOT NULL, schema_json TEXT NOT NULL, fingerprint TEXT NOT NULL, created_at TEXT NOT NULL)",
        )
        .await
        .unwrap();
        db
    }

    async fn declare(
        db: &sea_orm::DatabaseConnection,
        schema: ene_plugin_db::DbSchema,
    ) -> DbResponse {
        let mut declared_tables = HashMap::new();
        let mut declared_columns = HashMap::new();
        DbIpcServer::handle_declare_schema(
            db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            schema,
        )
        .await
        .unwrap_or_else(|e| e.to_error_response())
    }

    async fn stored_row(
        db: &sea_orm::DatabaseConnection,
        prefix: &str,
    ) -> Option<tool_schemas::Model> {
        tool_schemas::Entity::find_by_id(prefix)
            .one(db)
            .await
            .unwrap()
    }

    async fn table_columns(db: &sea_orm::DatabaseConnection, table: &str) -> Vec<String> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("PRAGMA table_info({table})"),
            ))
            .await
            .unwrap();
        rows.iter()
            .map(|r| r.try_get::<String>("", "name").unwrap())
            .collect()
    }

    async fn insert_row(
        db: &sea_orm::DatabaseConnection,
        table: &str,
        row: Row,
        declared_tables: &HashMap<String, ene_plugin_db::DbTable>,
        declared_columns: &HashMap<String, HashSet<String>>,
    ) -> DbResponse {
        let last_rowid: Arc<std::sync::Mutex<Option<i64>>> = Arc::new(std::sync::Mutex::new(None));
        let mut declared_tables = declared_tables.clone();
        let mut declared_columns = declared_columns.clone();
        DbIpcServer::handle_request(
            db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Insert {
                table: table.to_string(),
                row,
            },
        )
        .await
    }

    /// Registering a changed schema refreshes the stored row (fingerprint +
    /// `schema_json`) and applies the new column to the existing table.
    #[tokio::test]
    async fn changed_schema_updates_stored_row_and_applies_column() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));
        let row_v1 = stored_row(&db, "fs_").await.expect("row stored after v1");
        assert_eq!(
            table_columns(&db, "fs_undo").await,
            ["id".to_string(), "path".to_string()]
        );

        // v2 adds a `checksum` column to the existing `fs_undo` table.
        let mut schema_v2 = test_schema_v1();
        schema_v2.tables[0]
            .columns
            .push(test_column("checksum", ene_plugin_db::DbType::Text));

        let resp = declare(&db, schema_v2.clone()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // The stored declaration was refreshed to the new fingerprint/json.
        let row_v2 = stored_row(&db, "fs_").await.expect("row stored after v2");
        assert_ne!(row_v1.fingerprint, row_v2.fingerprint);
        assert_eq!(
            row_v2.schema_json,
            serde_json::to_string(&schema_v2).unwrap()
        );

        // The physical table gained the column, and existing rows survived.
        assert_eq!(
            table_columns(&db, "fs_undo").await,
            ["id".to_string(), "path".to_string(), "checksum".to_string()]
        );
    }

    /// Re-declaring an identical schema must not rewrite the stored row.
    #[tokio::test]
    async fn unchanged_schema_does_not_rewrite_stored_row() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));
        let first = stored_row(&db, "fs_").await.expect("row stored");

        // Ensure a rewrite would produce a different `created_at`.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));
        let second = stored_row(&db, "fs_").await.expect("row still stored");

        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.schema_json, second.schema_json);
        assert_eq!(first.created_at, second.created_at);
    }

    /// A column type change cannot be applied in place and is rejected.
    #[tokio::test]
    async fn type_change_is_rejected_as_conflict() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));
        let before = stored_row(&db, "fs_").await.expect("row stored");

        // v2 changes `path` from TEXT to INTEGER.
        let mut schema_v2 = test_schema_v1();
        schema_v2.tables[0].columns[1].ty = ene_plugin_db::DbType::Integer;

        let resp = declare(&db, schema_v2).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::SchemaConflict,
                    ..
                }
            ),
            "expected SchemaConflict, got {resp:?}"
        );

        // The stored declaration is untouched after the rejected change.
        let after = stored_row(&db, "fs_").await.expect("row still stored");
        assert_eq!(before.fingerprint, after.fingerprint);
        assert_eq!(before.schema_json, after.schema_json);
    }

    /// Adding a NOT NULL column without a DEFAULT is rejected: `SQLite` cannot
    /// add such a column via `ALTER TABLE` (existing rows would violate the
    /// constraint), and silently dropping the constraint would let validation
    /// and storage diverge.
    #[tokio::test]
    async fn not_null_column_without_default_is_rejected_as_conflict() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));
        let before = stored_row(&db, "fs_").await.expect("row stored");

        // v2 adds a NOT NULL column without a DEFAULT.
        let mut schema_v2 = test_schema_v1();
        let mut col = test_column("checksum", ene_plugin_db::DbType::Text);
        col.nullable = false;
        schema_v2.tables[0].columns.push(col);

        let resp = declare(&db, schema_v2).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::SchemaConflict,
                    ..
                }
            ),
            "expected SchemaConflict, got {resp:?}"
        );

        // The stored declaration is untouched after the rejected change.
        let after = stored_row(&db, "fs_").await.expect("row still stored");
        assert_eq!(before.fingerprint, after.fingerprint);
    }

    /// Removing a previously declared table is rejected rather than silently
    /// leaving the physical table orphaned.
    #[tokio::test]
    async fn removed_table_is_rejected_as_conflict() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // v2 drops the `fs_undo` table entirely.
        let schema_v2 = ene_plugin_db::DbSchema {
            prefix: "fs_".to_string(),
            tables: Vec::new(),
            indexes: Vec::new(),
        };

        let resp = declare(&db, schema_v2).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::SchemaConflict,
                    ..
                }
            ),
            "expected SchemaConflict, got {resp:?}"
        );
    }

    /// Removing a column from an existing table is rejected rather than
    /// leaving the physical column orphaned.
    #[tokio::test]
    async fn removed_column_is_rejected_as_conflict() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // v2 drops the `path` column from `fs_undo`.
        let mut schema_v2 = test_schema_v1();
        schema_v2.tables[0].columns.pop();

        let resp = declare(&db, schema_v2).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::SchemaConflict,
                    ..
                }
            ),
            "expected SchemaConflict, got {resp:?}"
        );
    }

    /// Adding a column that carries a `UNIQUE` constraint is rejected, since
    /// `SQLite` cannot add a constrained column via `ALTER TABLE`.
    #[tokio::test]
    async fn added_unique_column_is_rejected_as_conflict() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // v2 adds a UNIQUE column, which ALTER TABLE ADD COLUMN forbids.
        let mut schema_v2 = test_schema_v1();
        let mut unique_col = test_column("slug", ene_plugin_db::DbType::Text);
        unique_col.unique = true;
        schema_v2.tables[0].columns.push(unique_col);

        let resp = declare(&db, schema_v2).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::SchemaConflict,
                    ..
                }
            ),
            "expected SchemaConflict, got {resp:?}"
        );
    }

    /// An additive migration preserves existing rows and accepts new data in
    /// the added column — the exact scenario from #423.
    #[tokio::test]
    async fn additive_migration_preserves_existing_rows() {
        let db = test_db_with_tool_schemas().await;

        let resp = declare(&db, test_schema_v1()).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // Insert a row under the v1 schema.
        let declared_tables: HashMap<String, ene_plugin_db::DbTable> = test_schema_v1()
            .tables
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        let declared_columns: HashMap<String, HashSet<String>> = declared_tables
            .iter()
            .map(|(name, t)| {
                (
                    name.clone(),
                    t.columns.iter().map(|c| c.name.clone()).collect(),
                )
            })
            .collect();
        let resp = insert_row(
            &db,
            "fs_undo",
            std::collections::BTreeMap::from([(
                "path".to_string(),
                ene_plugin_db::DbValue::Text("/tmp/a".to_string()),
            )]),
            &declared_tables,
            &declared_columns,
        )
        .await;
        assert!(matches!(resp, DbResponse::Insert { .. }));

        // Migrate to v2 (adds `checksum`).
        let mut schema_v2 = test_schema_v1();
        schema_v2.tables[0]
            .columns
            .push(test_column("checksum", ene_plugin_db::DbType::Text));
        let resp = declare(&db, schema_v2).await;
        assert!(matches!(resp, DbResponse::SchemaAccepted { .. }));

        // The existing row survived; the new column reads back as NULL.
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT path, checksum FROM fs_undo".to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].try_get::<String>("", "path").unwrap(),
            "/tmp/a".to_string()
        );
        assert!(
            rows[0]
                .try_get::<Option<String>>("", "checksum")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn build_alter_add_column_sql_renders_type_and_default() {
        let table = ene_plugin_db::DbTable {
            name: "fs_undo".to_string(),
            columns: Vec::new(),
        };

        let nullable = test_column("checksum", ene_plugin_db::DbType::Text);
        assert_eq!(
            DbIpcServer::build_alter_add_column_sql(&table, &nullable).unwrap(),
            "ALTER TABLE fs_undo ADD COLUMN checksum TEXT"
        );

        let with_default = ene_plugin_db::DbColumn {
            name: "count".to_string(),
            ty: ene_plugin_db::DbType::Integer,
            nullable: false,
            primary_key: false,
            auto_increment: false,
            unique: false,
            default: Some(ene_plugin_db::DbValue::Int(0)),
        };
        assert_eq!(
            DbIpcServer::build_alter_add_column_sql(&table, &with_default).unwrap(),
            "ALTER TABLE fs_undo ADD COLUMN count INTEGER NOT NULL DEFAULT 0"
        );
    }

    // --- #427 cleanup regression tests ---

    /// A standalone upsert whose `conflict_columns` reference an undeclared
    /// column is rejected with a structured `UnknownColumn` error before
    /// anything reaches the database (#427 item 1).
    #[tokio::test]
    async fn upsert_with_undeclared_conflict_column_is_rejected() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Upsert {
                table: "fs_test".to_string(),
                row: std::collections::BTreeMap::from([(
                    "name".to_string(),
                    ene_plugin_db::DbValue::Text("alpha".to_string()),
                )]),
                conflict_columns: vec!["not_a_column".to_string()],
            },
        )
        .await;

        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::UnknownColumn);
                assert!(
                    message.contains("not_a_column"),
                    "error should name the bad conflict column: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Nothing was persisted.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            0
        );
    }

    /// A batch upsert with an undeclared conflict column is rejected up front
    /// (before the transaction runs) and names the failing operation (#427
    /// item 1).
    #[tokio::test]
    async fn batch_upsert_with_undeclared_conflict_column_is_rejected() {
        let (db, mut declared_tables, mut declared_columns, last_rowid) =
            setup_batch_fixture().await;

        let ops = vec![ene_plugin_db::DbWriteOp::Upsert {
            table: "fs_test".to_string(),
            row: std::collections::BTreeMap::from([(
                "name".to_string(),
                ene_plugin_db::DbValue::Text("alpha".to_string()),
            )]),
            conflict_columns: vec!["not_a_column".to_string()],
        }];

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Batch { ops },
        )
        .await;

        match resp {
            ene_plugin_db::DbResponse::Error { code, message } => {
                assert_eq!(code, ene_plugin_db::DbErrorCode::UnknownColumn);
                assert!(
                    message.contains("batch op 0"),
                    "error should name the failing op: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Nothing was persisted.
        assert_eq!(
            count_test_rows(
                &db,
                &mut declared_tables,
                &mut declared_columns,
                &last_rowid
            )
            .await,
            0
        );
    }

    /// A declared column whose `DEFAULT` is a non-finite float is rejected
    /// explicitly (`NaN`/`inf` have no valid `SQLite` literal) rather than
    /// producing an opaque database syntax error (#427 item 3).
    #[tokio::test]
    async fn declare_schema_rejects_non_finite_default() {
        let db = test_db_with_tool_schemas().await;

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut col = test_column("ratio", ene_plugin_db::DbType::Real);
            col.default = Some(ene_plugin_db::DbValue::Float(bad));
            let schema = ene_plugin_db::DbSchema {
                prefix: "fs_".to_string(),
                tables: vec![ene_plugin_db::DbTable {
                    name: "fs_thing".to_string(),
                    columns: vec![col],
                }],
                indexes: Vec::new(),
            };

            let resp = declare(&db, schema).await;
            assert!(
                matches!(
                    resp,
                    DbResponse::Error {
                        code: DbErrorCode::PermissionDenied,
                        ..
                    }
                ),
                "expected PermissionDenied for default {bad}, got {resp:?}"
            );
        }
    }

    /// Finite float defaults render valid DDL; non-finite ones are refused
    /// (#427 item 3).
    #[test]
    fn db_value_to_sql_rejects_non_finite_floats() {
        assert!(DbIpcServer::db_value_to_sql(&ene_plugin_db::DbValue::Float(1.5)).is_ok());
        assert!(DbIpcServer::db_value_to_sql(&ene_plugin_db::DbValue::Float(f64::NAN)).is_err());
        assert!(
            DbIpcServer::db_value_to_sql(&ene_plugin_db::DbValue::Float(f64::INFINITY)).is_err()
        );
        assert!(
            DbIpcServer::db_value_to_sql(&ene_plugin_db::DbValue::Float(f64::NEG_INFINITY))
                .is_err()
        );
    }

    /// An index name that does not carry the tool's prefix is rejected, so a
    /// plugin cannot squat a database-global index name a core migration may
    /// later need (#427 item 4).
    #[tokio::test]
    async fn declare_schema_rejects_index_without_prefix() {
        let db = test_db_with_tool_schemas().await;

        let schema = ene_plugin_db::DbSchema {
            prefix: "fs_".to_string(),
            tables: vec![ene_plugin_db::DbTable {
                name: "fs_thing".to_string(),
                columns: vec![test_column("id", ene_plugin_db::DbType::Integer)],
            }],
            indexes: vec![ene_plugin_db::DbIndex {
                // A plausible core index name with no `fs_` prefix.
                name: "idx_typed_mem_kind".to_string(),
                table: "fs_thing".to_string(),
                columns: vec!["id".to_string()],
                unique: false,
            }],
        };

        let resp = declare(&db, schema).await;
        assert!(
            matches!(
                resp,
                DbResponse::Error {
                    code: DbErrorCode::PermissionDenied,
                    ..
                }
            ),
            "expected PermissionDenied for unprefixed index, got {resp:?}"
        );
    }

    /// An index name carrying the tool's prefix (the established
    /// `idx_<prefix>...` convention) is accepted (#427 item 4).
    #[tokio::test]
    async fn declare_schema_accepts_prefixed_index() {
        let db = test_db_with_tool_schemas().await;

        let schema = ene_plugin_db::DbSchema {
            prefix: "fs_".to_string(),
            tables: vec![ene_plugin_db::DbTable {
                name: "fs_thing".to_string(),
                columns: vec![test_column("id", ene_plugin_db::DbType::Integer)],
            }],
            indexes: vec![ene_plugin_db::DbIndex {
                name: "idx_fs_thing_id".to_string(),
                table: "fs_thing".to_string(),
                columns: vec!["id".to_string()],
                unique: false,
            }],
        };

        let resp = declare(&db, schema).await;
        assert!(
            matches!(resp, DbResponse::SchemaAccepted { .. }),
            "expected SchemaAccepted for prefixed index, got {resp:?}"
        );
    }

    /// A column whose stored value does not decode as its declared type reads
    /// back as `Null` (the pre-existing behaviour), now made observable via a
    /// warning log (#427 item 8).
    #[tokio::test]
    async fn select_type_mismatch_reads_back_as_null() {
        use sea_orm::ConnectionTrait;
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        // SQLite is dynamically typed: this column holds text despite being
        // declared INTEGER.
        db.execute_unprepared("CREATE TABLE fs_test (id INTEGER PRIMARY KEY, qty INTEGER)")
            .await
            .unwrap();
        db.execute_unprepared("INSERT INTO fs_test (qty) VALUES ('not-a-number')")
            .await
            .unwrap();

        let mut declared_tables: HashMap<String, ene_plugin_db::DbTable> = HashMap::new();
        declared_tables.insert(
            "fs_test".to_string(),
            ene_plugin_db::DbTable {
                name: "fs_test".to_string(),
                columns: vec![
                    ene_plugin_db::DbColumn {
                        name: "id".to_string(),
                        ty: ene_plugin_db::DbType::Integer,
                        nullable: false,
                        primary_key: true,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    ene_plugin_db::DbColumn {
                        name: "qty".to_string(),
                        ty: ene_plugin_db::DbType::Integer,
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                ],
            },
        );
        let mut declared_columns: HashMap<String, HashSet<String>> = HashMap::new();
        declared_columns.insert(
            "fs_test".to_string(),
            ["id".to_string(), "qty".to_string()].into_iter().collect(),
        );
        let last_rowid: Arc<std::sync::Mutex<Option<i64>>> = Arc::new(std::sync::Mutex::new(None));

        let resp = DbIpcServer::handle_request(
            &db,
            "fs",
            "fs_",
            &mut declared_tables,
            &mut declared_columns,
            &last_rowid,
            ene_plugin_db::DbRequest::Select {
                table: "fs_test".to_string(),
                columns: vec!["qty".to_string()],
                filter: ene_plugin_db::DbFilter::Always,
                order_by: Vec::new(),
                limit: None,
            },
        )
        .await;

        let rows = match resp {
            ene_plugin_db::DbResponse::Select { rows } => rows,
            other => panic!("expected Select, got {other:?}"),
        };
        assert_eq!(rows.len(), 1);
        // The mismatched value surfaces as Null rather than an opaque decode
        // failure; the mismatch is logged (see `log_select_type_mismatch`).
        assert_eq!(rows[0].get("qty"), Some(&ene_plugin_db::DbValue::Null));
    }
}
