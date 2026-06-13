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
//! - Table names must start with the tool's prefix (e.g. `fs_`, `utility_`).
//! - DDL (CREATE/ALTER/DROP) is **not** exposed to tools.
//! - `sqlite_master` and other internal tables are blocked.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use ene_memory::entities::tool_schemas;
use ene_tool_db::{
    DbErrorCode, DbFilter, DbOrderBy, DbRequest, DbResponse, DbSchema, DbTable, DbValue, Row,
};
use sea_orm::sea_query::{Alias, Condition, Expr, Query, SqliteQueryBuilder};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, EntityTrait,
    Statement,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

/// Errors from the DB IPC server.
#[derive(Debug, thiserror::Error)]
pub enum DbServerError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// SeaORM database error.
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
            Self::Db(e) => (DbErrorCode::Internal, e.to_string()),
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

        let opt = ConnectOptions::new(format!("sqlite:{}", self.db_path.to_str().unwrap()));
        let db = Database::connect(opt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        loop {
            let (stream, _) = listener.accept().await?;
            debug!(tool = %self.tool_name, "Accepted DB IPC connection");

            let db = db.clone();
            let tool_name = self.tool_name.clone();
            let prefix = self.prefix.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, db, tool_name, prefix).await {
                    error!(error = %e, "DB IPC connection error");
                }
            });
        }
    }

    async fn handle_connection(
        stream: UnixStream,
        db: DatabaseConnection,
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
                &db,
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
        db: &DatabaseConnection,
        tool_name: &str,
        prefix: &str,
        declared_tables: &mut HashMap<String, DbTable>,
        declared_columns: &mut HashMap<String, HashSet<String>>,
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
                    .and_then(|_| Self::validate_row_columns(declared_columns, &table, &row));
                match res {
                    Ok(_) => match Self::handle_insert(db, &table, row).await {
                        Ok(id) => DbResponse::Insert { rowid: id },
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
                    .and_then(|_| Self::validate_row_columns(declared_columns, &table, &row));
                match res {
                    Ok(_) => match Self::handle_upsert(db, &table, row, conflict_columns).await {
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
                    .and_then(|_| Self::validate_row_columns(declared_columns, &table, &set))
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter));
                match res {
                    Ok(_) => match Self::handle_update(db, &table, set, filter).await {
                        Ok(affected) => DbResponse::Update { affected },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Delete { table, filter } => {
                let res = Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter));
                match res {
                    Ok(_) => match Self::handle_delete(db, &table, filter).await {
                        Ok(affected) => DbResponse::Delete { affected },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::Count { table, filter } => {
                let res = Self::validate_table_access(declared_tables, &table)
                    .and_then(|_| Self::validate_filter_columns(declared_columns, &table, &filter));
                match res {
                    Ok(_) => match Self::handle_count(db, &table, filter).await {
                        Ok(count) => DbResponse::Count { count },
                        Err(e) => e.to_error_response(),
                    },
                    Err(e) => e.to_error_response(),
                }
            }
            DbRequest::LastInsertRowId => Self::handle_last_insert_rowid(db)
                .await
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
        }

        let schema_json = serde_json::to_string(&schema)
            .map_err(|e| DbServerError::Internal(format!("Failed to serialize schema: {e}")))?;

        let fingerprint = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

        let mut created_indexes = Vec::new();

        // Use SeaORM ActiveModel to insert into tool_schemas
        let active_model = tool_schemas::ActiveModel {
            prefix: sea_orm::ActiveValue::Set(prefix.to_string()),
            schema_json: sea_orm::ActiveValue::Set(schema_json.clone()),
            fingerprint: sea_orm::ActiveValue::Set(fingerprint.clone()),
            created_at: sea_orm::ActiveValue::Set(chrono::Utc::now().to_rfc3339()),
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
            .exec(db)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        for table in &schema.tables {
            let create_sql = Self::build_create_table_sql(table);
            db.execute(Statement::from_string(DatabaseBackend::Sqlite, create_sql))
                .await
                .map_err(|e| DbServerError::Internal(e.to_string()))?;

            for index in &schema.indexes {
                if index.table == table.name {
                    let create_index_sql = Self::build_create_index_sql(index);
                    db.execute(Statement::from_string(
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

    async fn handle_insert(
        db: &DatabaseConnection,
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
            .execute(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.last_insert_id() as i64)
    }

    async fn handle_upsert(
        db: &DatabaseConnection,
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
            .execute(stmt)
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
            let order = if matches!(o.direction, ene_tool_db::DbOrderDirection::Desc) {
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
            .query_all(stmt)
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

        let mut rows = Vec::new();
        for result in query_results {
            let mut row = Row::new();
            for col in &cols_to_fetch {
                let col_def = table_def.columns.iter().find(|c| &c.name == col);
                let val = if let Some(def) = col_def {
                    match def.ty {
                        ene_tool_db::DbType::Integer => {
                            if let Ok(Some(v)) = result.try_get::<Option<i64>>("", col) {
                                DbValue::Int(v)
                            } else {
                                DbValue::Null
                            }
                        }
                        ene_tool_db::DbType::Real => {
                            if let Ok(Some(v)) = result.try_get::<Option<f64>>("", col) {
                                DbValue::Float(v)
                            } else {
                                DbValue::Null
                            }
                        }
                        ene_tool_db::DbType::Text => {
                            if let Ok(Some(v)) = result.try_get::<Option<String>>("", col) {
                                DbValue::Text(v)
                            } else {
                                DbValue::Null
                            }
                        }
                        ene_tool_db::DbType::Blob => {
                            if let Ok(Some(v)) = result.try_get::<Option<Vec<u8>>>("", col) {
                                DbValue::Blob(v)
                            } else {
                                DbValue::Null
                            }
                        }
                        ene_tool_db::DbType::Boolean => {
                            if let Ok(Some(v)) = result.try_get::<Option<bool>>("", col) {
                                DbValue::Bool(v)
                            } else if let Ok(Some(v)) = result.try_get::<Option<i64>>("", col) {
                                DbValue::Bool(v != 0)
                            } else {
                                DbValue::Null
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

    async fn handle_update(
        db: &DatabaseConnection,
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
            .execute(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        Ok(exec_res.rows_affected())
    }

    async fn handle_delete(
        db: &DatabaseConnection,
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
            .execute(stmt)
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
            .query_one(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;

        let count: i64 = match res {
            Some(row) => row.try_get("", "count").unwrap_or(0),
            None => 0,
        };

        Ok(count)
    }

    async fn handle_last_insert_rowid(db: &DatabaseConnection) -> Result<i64, DbServerError> {
        let stmt =
            Statement::from_string(DatabaseBackend::Sqlite, "SELECT last_insert_rowid() AS id");
        let res = db
            .query_one(stmt)
            .await
            .map_err(|e| DbServerError::Internal(e.to_string()))?;
        let id = match res {
            Some(row) => row.try_get("", "id").unwrap_or(0),
            None => 0,
        };
        Ok(id)
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
