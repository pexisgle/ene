//! Wire protocol messages exchanged between a tool's [`DbClient`](crate::DbClient)
//! and the core DB IPC server: [`DbRequest`] for tool-to-server calls,
//! [`DbResponse`] for server replies, and [`DbErrorCode`] for structured errors.

use crate::types::{DbFilter, DbOrderBy, DbSchema, DbValue, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Requests sent from the tool's `DbClient` to the host-service `db` passenger.
#[derive(Debug, Serialize, Deserialize)]
pub enum DbRequest {
    /// Declare the tool's database schema (tables and indexes).
    DeclareSchema(DbSchema),
    /// Insert a row into a table.
    Insert {
        /// Target table name.
        table: String,
        /// Column-value pairs to insert.
        row: Row,
    },
    /// Insert or update a row on conflict.
    Upsert {
        /// Target table name.
        table: String,
        /// Column-value pairs to insert/update.
        row: Row,
        /// Columns that define the conflict (ON CONFLICT clause).
        conflict_columns: Vec<String>,
    },
    /// Select rows matching a filter.
    Select {
        /// Target table name.
        table: String,
        /// Columns to return (empty = all).
        columns: Vec<String>,
        /// Filter expression.
        filter: DbFilter,
        /// Order-by clauses.
        order_by: Vec<DbOrderBy>,
        /// Maximum number of rows to return.
        limit: Option<u64>,
    },
    /// Update rows matching a filter.
    Update {
        /// Target table name.
        table: String,
        /// Column-value pairs to set.
        set: BTreeMap<String, DbValue>,
        /// Filter expression.
        filter: DbFilter,
    },
    /// Delete rows matching a filter.
    Delete {
        /// Target table name.
        table: String,
        /// Filter expression.
        filter: DbFilter,
    },
    /// Count rows matching a filter.
    Count {
        /// Target table name.
        table: String,
        /// Filter expression.
        filter: DbFilter,
    },
    /// Execute a group of write operations atomically.
    ///
    /// The server validates every operation up front, then applies them
    /// inside a single `SQLite` transaction: either all operations commit,
    /// or (if any operation fails) the entire batch is rolled back and
    /// nothing is persisted. The transaction is scoped to this one request,
    /// so a plugin can never hold the write lock open across multiple
    /// round-trips, and a dropped connection can never leave a half-applied
    /// batch behind. See [`DbWriteOp`] for the supported operations and
    /// [`DbResponse::Batch`] for the per-operation outcomes.
    Batch {
        /// The write operations to apply, in order.
        ops: Vec<DbWriteOp>,
    },
    /// Get the most recently inserted rowid.
    LastInsertRowId,
    /// Health-check ping.
    Ping,
    /// Request graceful shutdown.
    Shutdown,
}

/// A single write operation within an atomic [`DbRequest::Batch`].
///
/// Each variant mirrors the corresponding standalone request
/// ([`DbRequest::Insert`], [`DbRequest::Upsert`], [`DbRequest::Update`],
/// [`DbRequest::Delete`]) but carries no request-level framing: the batch
/// is the unit of execution, and the server reports one
/// [`DbBatchOpResult`](crate::DbBatchOpResult) per operation, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbWriteOp {
    /// Insert a row into a table.
    Insert {
        /// Target table name.
        table: String,
        /// Column-value pairs to insert.
        row: Row,
    },
    /// Insert or update a row on conflict.
    Upsert {
        /// Target table name.
        table: String,
        /// Column-value pairs to insert/update.
        row: Row,
        /// Columns that define the conflict (ON CONFLICT clause).
        conflict_columns: Vec<String>,
    },
    /// Update rows matching a filter.
    Update {
        /// Target table name.
        table: String,
        /// Column-value pairs to set.
        set: BTreeMap<String, DbValue>,
        /// Filter expression.
        filter: DbFilter,
    },
    /// Delete rows matching a filter.
    Delete {
        /// Target table name.
        table: String,
        /// Filter expression.
        filter: DbFilter,
    },
}

/// Responses sent from the DB IPC server back to the tool's `DbClient`.
#[derive(Debug, Serialize, Deserialize)]
pub enum DbResponse {
    /// Schema was accepted and tables/indexes were created.
    SchemaAccepted {
        /// Names of created tables.
        tables: Vec<String>,
        /// Names of created indexes.
        indexes: Vec<String>,
    },
    /// Row was inserted successfully.
    Insert {
        /// The new row's rowid.
        rowid: i64,
    },
    /// Row was upserted successfully.
    Upsert {
        /// The row's rowid.
        rowid: i64,
    },
    /// Rows were selected successfully.
    Select {
        /// The selected rows.
        rows: Vec<Row>,
    },
    /// Rows were updated successfully.
    Update {
        /// Number of affected rows.
        affected: u64,
    },
    /// Rows were deleted successfully.
    Delete {
        /// Number of affected rows.
        affected: u64,
    },
    /// Row count result.
    Count {
        /// The matching row count.
        count: i64,
    },
    /// Result of an atomic [`DbRequest::Batch`].
    ///
    /// Returned only when the whole batch committed. `results` holds one
    /// entry per operation, in the same order as the request's `ops`. If
    /// any operation failed, the server rolls the entire batch back and
    /// instead returns [`DbResponse::Error`] naming the failing operation;
    /// in that case nothing from the batch is persisted.
    Batch {
        /// Per-operation outcomes, in request order.
        results: Vec<DbBatchOpResult>,
    },
    /// Last insert rowid result.
    LastInsertRowId {
        /// The most recently inserted rowid.
        rowid: i64,
    },
    /// Pong response to a Ping request.
    Pong,
    /// Acknowledgement (e.g. for Shutdown).
    Ack,
    /// Error response.
    Error {
        /// The error code.
        code: DbErrorCode,
        /// Human-readable error message.
        message: String,
    },
}

/// Outcome of a single operation inside a committed [`DbRequest::Batch`].
///
/// Mirrors the subset of [`DbResponse`] that write operations produce, so a
/// caller can recover the same `rowid`/`affected` information it would have
/// gotten from the standalone requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbBatchOpResult {
    /// A row was inserted; carries the new row's `rowid`.
    Insert {
        /// The new row's rowid.
        rowid: i64,
    },
    /// A row was upserted; carries the row's `rowid`.
    Upsert {
        /// The row's rowid.
        rowid: i64,
    },
    /// Rows were updated; carries the number of affected rows.
    Update {
        /// Number of affected rows.
        affected: u64,
    },
    /// Rows were deleted; carries the number of affected rows.
    Delete {
        /// Number of affected rows.
        affected: u64,
    },
}

/// Error codes returned by the DB IPC server.
///
/// Deserialization is forward-compatible: an unknown code (e.g. emitted by a
/// newer host) maps to [`DbErrorCode::Unknown`] instead of failing, so older
/// plugins can still surface a diagnostic rather than dropping the error
/// response wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DbErrorCode {
    /// The tool does not have permission to access the requested resource.
    PermissionDenied,
    /// The specified table does not exist or is not declared.
    UnknownTable,
    /// The specified column does not exist in the table.
    UnknownColumn,
    /// A value's type does not match the column's declared type.
    TypeMismatch,
    /// The filter expression is invalid.
    InvalidFilter,
    /// The declared schema conflicts with the one already stored for this
    /// prefix in a way that cannot be applied automatically (e.g. a column
    /// type change). The plugin must reconcile the difference explicitly.
    SchemaConflict,
    /// A write was rejected because it would push the plugin's storage in the
    /// shared `memory.db` past its configured quota
    /// (`plugins.list.<name>.db_quota_mb`). Reads and deletes are still
    /// permitted so the plugin can free space.
    QuotaExceeded,
    /// The requested operation or service is not implemented by the host.
    Unsupported,
    /// An internal server error occurred.
    Internal,
    /// An error code this build does not know about (emitted by a newer
    /// host). Keeps deserialization of error responses forward-compatible.
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for DbErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "PERMISSION_DENIED"),
            Self::UnknownTable => write!(f, "UNKNOWN_TABLE"),
            Self::UnknownColumn => write!(f, "UNKNOWN_COLUMN"),
            Self::TypeMismatch => write!(f, "TYPE_MISMATCH"),
            Self::InvalidFilter => write!(f, "INVALID_FILTER"),
            Self::SchemaConflict => write!(f, "SCHEMA_CONFLICT"),
            Self::QuotaExceeded => write!(f, "QUOTA_EXCEEDED"),
            Self::Unsupported => write!(f, "UNSUPPORTED"),
            Self::Unknown => write!(f, "UNKNOWN"),
            Self::Internal => write!(f, "INTERNAL"),
        }
    }
}
