//! Per-tool DB IPC server using diesel.
//!
//! Listens on a Unix socket, accepts tool connections, and dispatches
//! [`DbRequest`] messages against the shared `memory.db`.
//!
//! ## Security model
//!
//! - Each tool declares its schema (tables + columns) via `DeclareSchema`.
//! - The server records the declaration in `__tool_schemas` and enforces that
//!   all subsequent requests only reference tables/columns in the declaration.
//! - Table names must start with the tool's prefix (e.g. `fs_`, `utility_`).
//! - DDL (CREATE/ALTER/DROP) is **not** exposed to tools.
//! - `sqlite_master` and other internal tables are blocked.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::Arc;

use diesel::prelude::*;
use diesel::r2d2::{self, ConnectionManager, Pool};
use diesel::sql_types::{BigInt, Binary, Double, Integer, Text};
use diesel::{SqliteConnection, sql_query};
use ene_tool_db::{
    DbErrorCode, DbFilter, DbOrderBy, DbRequest, DbResponse, DbSchema, DbTable, DbValue, Row,
};
use libsqlite3_sys::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// Errors from the DB IPC server.
#[derive(Debug, thiserror::Error)]
pub enum DbServerError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Diesel query error.
    #[error("Diesel error: {0}")]
    Diesel(#[from] diesel::result::Error),
    /// Connection pool error.
    #[error("Pool error: {0}")]
    Pool(#[from] r2d2::PoolError),
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
    /// An internal server error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl DbServerError {
    fn to_error_response(&self) -> DbResponse {
        let (code, message) = match self {
            Self::PermissionDenied(msg) => (DbErrorCode::PermissionDenied, msg.clone()),
            Self::UnknownTable(msg) => (DbErrorCode::UnknownTable, msg.clone()),
            Self::UnknownColumn { table, column } => (
                DbErrorCode::UnknownColumn,
                format!("Unknown column: {table}.{column}"),
            ),
            Self::Io(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Json(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Diesel(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Pool(e) => (DbErrorCode::Internal, e.to_string()),
            Self::Internal(msg) => (DbErrorCode::Internal, msg.clone()),
        };
        DbResponse::Error { code, message }
    }
}

/// Per-tool DB IPC server.
pub struct DbIpcServer {
    db_path: PathBuf,
    socket_path: PathBuf,
    tool_name: String,
    prefix: String,
}

impl DbIpcServer {
    /// Creates a new server for the given tool.
    pub fn new(db_path: PathBuf, socket_path: PathBuf, tool_name: String, prefix: String) -> Self {
        Self {
            db_path,
            socket_path,
            tool_name,
            prefix,
        }
    }

    /// Runs the server, listening for connections and handling requests.
    pub async fn run(self) -> Result<(), DbServerError> {
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        info!(
            tool = %self.tool_name,
            socket = %self.socket_path.display(),
            "DB IPC server listening"
        );

        let manager = ConnectionManager::<SqliteConnection>::new(self.db_path.to_str().unwrap());
        let pool = Pool::builder()
            .max_size(10)
            .build(manager)
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let pool = Arc::new(pool);

        loop {
            let (stream, _) = listener.accept().await?;
            debug!(tool = %self.tool_name, "Accepted DB IPC connection");

            let pool = pool.clone();
            let db_path = self.db_path.clone();
            let tool_name = self.tool_name.clone();
            let prefix = self.prefix.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    Self::handle_connection(stream, pool, db_path, tool_name, prefix).await
                {
                    error!(error = %e, "DB IPC connection error");
                }
            });
        }
    }

    async fn handle_connection(
        stream: UnixStream,
        pool: Arc<DbPool>,
        db_path: PathBuf,
        tool_name: String,
        prefix: String,
    ) -> Result<(), DbServerError> {
        let (mut reader, mut writer) = stream.into_split();

        let mut declared_tables: HashMap<String, DbTable> = HashMap::new();
        let mut declared_columns: HashMap<String, HashSet<String>> = HashMap::new();

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    debug!(tool = %tool_name, "DB IPC connection closed");
                    break;
                }
                Err(e) => return Err(DbServerError::Io(e)),
            }
            let msg_len = u32::from_le_bytes(len_buf) as usize;
            if msg_len > 64 * 1024 * 1024 {
                return Err(DbServerError::Internal(format!(
                    "message too large: {msg_len}"
                )));
            }

            let mut msg_buf = vec![0u8; msg_len];
            reader.read_exact(&mut msg_buf).await?;

            let request: DbRequest = match serde_json::from_slice(&msg_buf) {
                Ok(req) => req,
                Err(e) => {
                    warn!(tool = %tool_name, error = %e, "Invalid JSON in DB request");
                    let response = DbResponse::Error {
                        code: DbErrorCode::Internal,
                        message: format!("Invalid JSON: {e}"),
                    };
                    Self::send_response(&mut writer, &response).await?;
                    continue;
                }
            };

            let response = Self::handle_request(
                &pool,
                &db_path,
                &tool_name,
                &prefix,
                &mut declared_tables,
                &mut declared_columns,
                request,
            )
            .await;

            let is_shutdown = matches!(response, DbResponse::Ack);
            Self::send_response(&mut writer, &response).await?;

            if is_shutdown {
                debug!(tool = %tool_name, "Received shutdown request");
                break;
            }
        }

        Ok(())
    }

    async fn send_response(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        response: &DbResponse,
    ) -> Result<(), DbServerError> {
        let json = serde_json::to_vec(response)?;
        let len = json.len() as u32;
        writer.write_all(&len.to_le_bytes()).await?;
        writer.write_all(&json).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn handle_request(
        pool: &Arc<DbPool>,
        db_path: &std::path::Path,
        tool_name: &str,
        prefix: &str,
        declared_tables: &mut HashMap<String, DbTable>,
        declared_columns: &mut HashMap<String, HashSet<String>>,
        request: DbRequest,
    ) -> DbResponse {
        match request {
            DbRequest::DeclareSchema(schema) => Self::handle_declare_schema(
                pool,
                tool_name,
                prefix,
                declared_tables,
                declared_columns,
                schema,
            )
            .await
            .unwrap_or_else(|e| e.to_error_response()),
            DbRequest::Insert { table, row } => {
                Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_row_columns(declared_columns, &table, &row))
                    .and_then(|_| Self::handle_insert(pool, &table, row))
                    .map(|id| DbResponse::Insert { rowid: id })
                    .unwrap_or_else(|e| e.to_error_response())
            }
            DbRequest::Upsert {
                table,
                row,
                conflict_columns,
            } => Self::validate_table_access(declared_tables, &table)
                .and_then(|_| Self::validate_row_columns(declared_columns, &table, &row))
                .and_then(|_| Self::handle_upsert(pool, &table, row, conflict_columns))
                .map(|rowid| DbResponse::Upsert { rowid })
                .unwrap_or_else(|e| e.to_error_response()),
            DbRequest::Select {
                table,
                columns,
                filter,
                order_by,
                limit,
            } => Self::validate_table_access(declared_tables, &table)
                .and_then(|_| Self::validate_select_columns(declared_columns, &table, &columns))
                .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter))
                .and_then(|_| Self::validate_order_by_columns(declared_columns, &table, &order_by))
                .and_then(|_| {
                    Self::handle_select(pool, db_path, &table, columns, filter, order_by, limit)
                })
                .map(|rows| DbResponse::Select { rows })
                .unwrap_or_else(|e| e.to_error_response()),
            DbRequest::Update { table, set, filter } => {
                Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_row_columns(declared_columns, &table, &set))
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter))
                    .and_then(|_| Self::handle_update(pool, &table, set, filter))
                    .map(|affected| DbResponse::Update { affected })
                    .unwrap_or_else(|e| e.to_error_response())
            }
            DbRequest::Delete { table, filter } => {
                Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter))
                    .and_then(|_| Self::handle_delete(pool, &table, filter))
                    .map(|affected| DbResponse::Delete { affected })
                    .unwrap_or_else(|e| e.to_error_response())
            }
            DbRequest::Count { table, filter } => {
                Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter))
                    .and_then(|_| Self::handle_count(pool, &table, filter))
                    .map(|count| DbResponse::Count { count })
                    .unwrap_or_else(|e| e.to_error_response())
            }
            DbRequest::LastInsertRowId => Self::handle_last_insert_rowid(pool)
                .map(|rowid| DbResponse::LastInsertRowId { rowid })
                .unwrap_or_else(|e| e.to_error_response()),
            DbRequest::Ping => DbResponse::Pong,
            DbRequest::Shutdown => DbResponse::Ack,
        }
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
        pool: &Arc<DbPool>,
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
        }

        let schema_json = serde_json::to_string(&schema)
            .map_err(|e| DbServerError::Internal(format!("Failed to serialize schema: {e}")))?;

        let fingerprint = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

        let mut created_indexes = Vec::new();
        {
            let mut conn = pool.get()?;

            sql_query(
                "INSERT OR REPLACE INTO __tool_schemas (prefix, schema_json, fingerprint, created_at)
                 VALUES (?1, ?2, ?3, datetime('now'))",
            )
            .bind::<Text, _>(prefix)
            .bind::<Text, _>(&schema_json)
            .bind::<Text, _>(&fingerprint)
            .execute(&mut conn)?;

            for table in &schema.tables {
                let create_sql = Self::build_create_table_sql(table);
                sql_query(&create_sql).execute(&mut conn)?;

                for index in &schema.indexes {
                    if index.table == table.name {
                        let create_index_sql = Self::build_create_index_sql(index);
                        sql_query(&create_index_sql).execute(&mut conn)?;
                        created_indexes.push(index.name.clone());
                    }
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

    fn build_create_table_sql(table: &DbTable) -> String {
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
                ene_tool_db::DbType::Integer => "INTEGER",
                ene_tool_db::DbType::Real => "REAL",
                ene_tool_db::DbType::Text => "TEXT",
                ene_tool_db::DbType::Blob => "BLOB",
                ene_tool_db::DbType::Boolean => "INTEGER",
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
                sql.push_str(&Self::db_value_to_sql(default));
            }
        }

        sql.push(')');
        sql
    }

    fn db_value_to_sql(value: &DbValue) -> String {
        match value {
            DbValue::Null => "NULL".to_string(),
            DbValue::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            DbValue::Int(i) => i.to_string(),
            DbValue::Float(f) => f.to_string(),
            DbValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
            DbValue::Blob(b) => format!(
                "X'{}'",
                b.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
        }
    }

    fn build_create_index_sql(index: &ene_tool_db::DbIndex) -> String {
        let columns = index.columns.join(", ");
        let unique = if index.unique { "UNIQUE " } else { "" };
        format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({})",
            index.name, index.table, columns
        )
    }

    fn handle_insert(pool: &Arc<DbPool>, table: &str, row: Row) -> Result<i64, DbServerError> {
        let mut conn = pool.get()?;

        let columns: Vec<String> = row.keys().cloned().collect();
        let values: Vec<DbValue> = columns.iter().map(|k| row[k].clone()).collect();

        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table,
            columns.join(", "),
            placeholders.join(", ")
        );

        let mut query = sql_query(&sql).into_boxed();
        for value in &values {
            query = match value {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        query.execute(&mut conn)?;

        let rowid: i64 = diesel::select(diesel::dsl::sql::<BigInt>("last_insert_rowid()"))
            .get_result(&mut conn)?;

        Ok(rowid)
    }

    fn handle_upsert(
        pool: &Arc<DbPool>,
        table: &str,
        row: Row,
        conflict_columns: Vec<String>,
    ) -> Result<i64, DbServerError> {
        let mut conn = pool.get()?;

        let columns: Vec<String> = row.keys().cloned().collect();
        let values: Vec<DbValue> = columns.iter().map(|k| row[k].clone()).collect();

        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let update_columns: Vec<String> = columns
            .iter()
            .filter(|c| !conflict_columns.contains(c))
            .map(|c| format!("{c} = excluded.{c}"))
            .collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {}",
            table,
            columns.join(", "),
            placeholders.join(", "),
            conflict_columns.join(", "),
            update_columns.join(", ")
        );

        let mut query = sql_query(&sql).into_boxed();
        for value in &values {
            query = match value {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        query.execute(&mut conn)?;

        let rowid: i64 = diesel::select(diesel::dsl::sql::<BigInt>("last_insert_rowid()"))
            .get_result(&mut conn)?;

        Ok(rowid)
    }

    fn handle_select(
        pool: &Arc<DbPool>,
        db_path: &std::path::Path,
        table: &str,
        columns: Vec<String>,
        filter: DbFilter,
        order_by: Vec<DbOrderBy>,
        limit: Option<u64>,
    ) -> Result<Vec<Row>, DbServerError> {
        let mut conn = pool.get()?;

        let select_cols = if columns.is_empty() {
            "*".to_string()
        } else {
            columns.join(", ")
        };

        let mut sql = format!("SELECT {select_cols} FROM {table}");

        let mut params: Vec<DbValue> = Vec::new();
        let (where_clause, filter_params) = Self::build_where_clause(&filter)?;
        if !where_clause.is_empty() {
            sql.push_str(&format!(" WHERE {where_clause}"));
            params.extend(filter_params);
        }

        if !order_by.is_empty() {
            let order_parts: Vec<String> = order_by
                .iter()
                .map(|o| {
                    format!(
                        "{} {}",
                        o.column,
                        if matches!(o.direction, ene_tool_db::DbOrderDirection::Desc) {
                            "DESC"
                        } else {
                            "ASC"
                        }
                    )
                })
                .collect();
            sql.push_str(&format!(" ORDER BY {}", order_parts.join(", ")));
        }

        if let Some(limit) = limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut query = sql_query(&sql).into_boxed();
        for param in &params {
            query = match param {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        let _ = query.execute(&mut conn)?;

        Self::raw_select(db_path, &sql, &params)
    }

    fn raw_select(
        db_path: &std::path::Path,
        sql: &str,
        params: &[DbValue],
    ) -> Result<Vec<Row>, DbServerError> {
        unsafe {
            let path_c = CString::new(
                db_path
                    .to_str()
                    .ok_or_else(|| DbServerError::Internal("invalid path".to_string()))?,
            )
            .map_err(|e| DbServerError::Internal(e.to_string()))?;
            let mut db: *mut sqlite3 = std::ptr::null_mut();

            let rc = sqlite3_open(path_c.as_ptr(), &mut db);
            if rc != SQLITE_OK {
                let err = CStr::from_ptr(sqlite3_errmsg(db))
                    .to_string_lossy()
                    .into_owned();
                sqlite3_close(db);
                return Err(DbServerError::Internal(format!("open failed: {err}")));
            }

            let sql_c = CString::new(sql).map_err(|e| DbServerError::Internal(e.to_string()))?;
            let mut stmt: *mut sqlite3_stmt = std::ptr::null_mut();

            let rc = sqlite3_prepare_v2(db, sql_c.as_ptr(), -1, &mut stmt, std::ptr::null_mut());
            if rc != SQLITE_OK {
                let err = CStr::from_ptr(sqlite3_errmsg(db))
                    .to_string_lossy()
                    .into_owned();
                sqlite3_close(db);
                return Err(DbServerError::Internal(format!("prepare failed: {err}")));
            }

            for (i, param) in params.iter().enumerate() {
                let idx = (i + 1) as c_int;
                match param {
                    DbValue::Null => {
                        sqlite3_bind_null(stmt, idx);
                    }
                    DbValue::Bool(b) => {
                        sqlite3_bind_int(stmt, idx, *b as c_int);
                    }
                    DbValue::Int(i) => {
                        sqlite3_bind_int64(stmt, idx, *i);
                    }
                    DbValue::Float(f) => {
                        sqlite3_bind_double(stmt, idx, *f);
                    }
                    DbValue::Text(s) => {
                        let c = CString::new(s.as_str())
                            .map_err(|e| DbServerError::Internal(e.to_string()))?;
                        sqlite3_bind_text(stmt, idx, c.as_ptr(), -1, Self::SQLITE_TRANSIENT());
                    }
                    DbValue::Blob(b) => {
                        sqlite3_bind_blob(
                            stmt,
                            idx,
                            b.as_ptr() as *const _,
                            b.len() as c_int,
                            Self::SQLITE_TRANSIENT(),
                        );
                    }
                }
            }

            let col_count = sqlite3_column_count(stmt);
            let col_names: Vec<String> = (0..col_count)
                .map(|i| {
                    let name = sqlite3_column_name(stmt, i);
                    CStr::from_ptr(name).to_string_lossy().into_owned()
                })
                .collect();

            let mut rows = Vec::new();
            loop {
                let rc = sqlite3_step(stmt);
                if rc == SQLITE_DONE {
                    break;
                }
                if rc != SQLITE_ROW {
                    sqlite3_finalize(stmt);
                    sqlite3_close(db);
                    let err = CStr::from_ptr(sqlite3_errmsg(db))
                        .to_string_lossy()
                        .into_owned();
                    return Err(DbServerError::Internal(format!("step failed: {err}")));
                }

                let mut row = Row::new();
                for i in 0..col_count {
                    let col_type = sqlite3_column_type(stmt, i);
                    let value = match col_type {
                        SQLITE_NULL => DbValue::Null,
                        SQLITE_INTEGER => DbValue::Int(sqlite3_column_int64(stmt, i)),
                        SQLITE_FLOAT => DbValue::Float(sqlite3_column_double(stmt, i)),
                        SQLITE_TEXT => {
                            let text = sqlite3_column_text(stmt, i);
                            let s = CStr::from_ptr(text as *const _)
                                .to_string_lossy()
                                .into_owned();
                            DbValue::Text(s)
                        }
                        SQLITE_BLOB => {
                            let blob = sqlite3_column_blob(stmt, i);
                            let len = sqlite3_column_bytes(stmt, i) as usize;
                            let bytes = std::slice::from_raw_parts(blob as *const u8, len).to_vec();
                            DbValue::Blob(bytes)
                        }
                        _ => DbValue::Null,
                    };
                    row.insert(col_names[i as usize].clone(), value);
                }
                rows.push(row);
            }

            sqlite3_finalize(stmt);
            sqlite3_close(db);
            Ok(rows)
        }
    }

    #[allow(non_snake_case)]
    unsafe fn SQLITE_TRANSIENT() -> Option<unsafe extern "C" fn(*mut std::os::raw::c_void)> {
        // SAFETY: -1 as a destructor function pointer is a special SQLite constant meaning
        // "SQLite will make a copy of the data", which is what we want since our CString
        // will be dropped at the end of the match arm.
        Some(unsafe {
            std::mem::transmute::<isize, unsafe extern "C" fn(*mut std::os::raw::c_void)>(-1)
        })
    }

    fn handle_update(
        pool: &Arc<DbPool>,
        table: &str,
        set: Row,
        filter: DbFilter,
    ) -> Result<u64, DbServerError> {
        let mut conn = pool.get()?;

        let set_columns: Vec<String> = set.keys().cloned().collect();
        let set_values: Vec<DbValue> = set_columns.iter().map(|k| set[k].clone()).collect();

        let set_parts: Vec<String> = set_columns
            .iter()
            .enumerate()
            .map(|(i, col)| format!("{col} = ?{}", i + 1))
            .collect();

        let mut sql = format!("UPDATE {table} SET {}", set_parts.join(", "));

        let mut params = set_values;
        let (where_clause, filter_params) = Self::build_where_clause(&filter)?;
        if !where_clause.is_empty() {
            sql.push_str(&format!(" WHERE {where_clause}"));
            params.extend(filter_params);
        }

        let mut query = sql_query(&sql).into_boxed();
        for param in &params {
            query = match param {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        let count = query.execute(&mut conn)?;
        Ok(count as u64)
    }

    fn handle_delete(
        pool: &Arc<DbPool>,
        table: &str,
        filter: DbFilter,
    ) -> Result<u64, DbServerError> {
        let mut conn = pool.get()?;

        let mut sql = format!("DELETE FROM {table}");

        let (where_clause, params) = Self::build_where_clause(&filter)?;
        if !where_clause.is_empty() {
            sql.push_str(&format!(" WHERE {where_clause}"));
        }

        let mut query = sql_query(&sql).into_boxed();
        for param in &params {
            query = match param {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        let count = query.execute(&mut conn)?;
        Ok(count as u64)
    }

    fn handle_count(
        pool: &Arc<DbPool>,
        table: &str,
        filter: DbFilter,
    ) -> Result<i64, DbServerError> {
        let mut conn = pool.get()?;

        let mut sql = format!("SELECT COUNT(*) as count FROM {table}");

        let (where_clause, params) = Self::build_where_clause(&filter)?;
        if !where_clause.is_empty() {
            sql.push_str(&format!(" WHERE {where_clause}"));
        }

        let mut query = sql_query(&sql).into_boxed();
        for param in &params {
            query = match param {
                DbValue::Null => query.bind::<diesel::sql_types::Nullable<Integer>, _>(None::<i32>),
                DbValue::Bool(b) => query.bind::<Integer, _>(*b as i32),
                DbValue::Int(i) => query.bind::<BigInt, _>(*i),
                DbValue::Float(f) => query.bind::<Double, _>(*f),
                DbValue::Text(s) => query.bind::<Text, _>(s),
                DbValue::Blob(b) => query.bind::<Binary, _>(b),
            };
        }

        let result = query.load::<CountRow>(&mut conn)?;
        Ok(result.first().map(|r| r.count).unwrap_or(0))
    }

    fn handle_last_insert_rowid(pool: &Arc<DbPool>) -> Result<i64, DbServerError> {
        let mut conn = pool.get()?;
        let rowid: i64 = diesel::select(diesel::dsl::sql::<BigInt>("last_insert_rowid()"))
            .get_result(&mut conn)?;
        Ok(rowid)
    }

    fn build_where_clause(filter: &DbFilter) -> Result<(String, Vec<DbValue>), DbServerError> {
        let mut params = Vec::new();
        let clause = Self::build_filter_expr(filter, &mut params)?;
        Ok((clause, params))
    }

    fn build_filter_expr(
        filter: &DbFilter,
        params: &mut Vec<DbValue>,
    ) -> Result<String, DbServerError> {
        match filter {
            DbFilter::Always => Ok("1=1".to_string()),
            DbFilter::And(filters) => {
                if filters.is_empty() {
                    return Ok("1=1".to_string());
                }
                let parts: Result<Vec<String>, _> = filters
                    .iter()
                    .map(|f| Self::build_filter_expr(f, params))
                    .collect();
                Ok(format!("({})", parts?.join(" AND ")))
            }
            DbFilter::Or(filters) => {
                if filters.is_empty() {
                    return Ok("1=0".to_string());
                }
                let parts: Result<Vec<String>, _> = filters
                    .iter()
                    .map(|f| Self::build_filter_expr(f, params))
                    .collect();
                Ok(format!("({})", parts?.join(" OR ")))
            }
            DbFilter::Not(f) => {
                let inner = Self::build_filter_expr(f, params)?;
                Ok(format!("NOT ({inner})"))
            }
            DbFilter::Eq { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} = ?{idx}"))
            }
            DbFilter::Ne { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} != ?{idx}"))
            }
            DbFilter::Lt { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} < ?{idx}"))
            }
            DbFilter::Le { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} <= ?{idx}"))
            }
            DbFilter::Gt { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} > ?{idx}"))
            }
            DbFilter::Ge { column, value } => {
                params.push(value.clone());
                let idx = params.len();
                Ok(format!("{column} >= ?{idx}"))
            }
            DbFilter::In { column, values } => {
                if values.is_empty() {
                    return Ok("1=0".to_string());
                }
                let start_idx = params.len() + 1;
                for v in values {
                    params.push(v.clone());
                }
                let end_idx = params.len();
                let placeholders: Vec<String> =
                    (start_idx..=end_idx).map(|i| format!("?{i}")).collect();
                Ok(format!("{column} IN ({})", placeholders.join(", ")))
            }
            DbFilter::Like { column, pattern } => {
                params.push(DbValue::Text(pattern.clone()));
                let idx = params.len();
                Ok(format!("{column} LIKE ?{idx}"))
            }
            DbFilter::IsNull { column } => Ok(format!("{column} IS NULL")),
            DbFilter::IsNotNull { column } => Ok(format!("{column} IS NOT NULL")),
        }
    }
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    count: i64,
}
