//! Wire protocol messages exchanged between a tool's [`DbClient`](crate::DbClient)
//! and the core DB IPC server: [`DbRequest`] for tool-to-server calls,
//! [`DbResponse`] for server replies, and [`DbErrorCode`] for structured errors.

use crate::types::{DbFilter, DbOrderBy, DbSchema, DbValue, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Requests sent from the tool's `DbClient` to the host-service `db` passenger.
#[derive(Debug, Serialize, Deserialize)]
pub enum DbRequest {
    DeclareSchema(DbSchema),
    Insert {
        table: String,
        row: Row,
    },
    Upsert {
        table: String,
        row: Row,
        conflict_columns: Vec<String>,
    },
    Select {
        table: String,
        /// Columns to return (empty = all).
        columns: Vec<String>,
        filter: DbFilter,
        order_by: Vec<DbOrderBy>,
        limit: Option<u64>,
    },
    Update {
        table: String,
        set: BTreeMap<String, DbValue>,
        filter: DbFilter,
    },
    Delete {
        table: String,
        filter: DbFilter,
    },
    Count {
        table: String,
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
        ops: Vec<DbWriteOp>,
    },
    LastInsertRowId,
    Ping,
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
    Insert {
        table: String,
        row: Row,
    },
    Upsert {
        table: String,
        row: Row,
        conflict_columns: Vec<String>,
    },
    Update {
        table: String,
        set: BTreeMap<String, DbValue>,
        filter: DbFilter,
    },
    Delete {
        table: String,
        filter: DbFilter,
    },
}

/// Responses sent from the DB IPC server back to the tool's `DbClient`.
#[derive(Debug, Serialize, Deserialize)]
pub enum DbResponse {
    SchemaAccepted {
        tables: Vec<String>,
        indexes: Vec<String>,
    },
    Insert {
        rowid: i64,
    },
    Upsert {
        rowid: i64,
    },
    Select {
        rows: Vec<Row>,
    },
    Update {
        affected: u64,
    },
    Delete {
        affected: u64,
    },
    Count {
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
        results: Vec<DbBatchOpResult>,
    },
    LastInsertRowId {
        rowid: i64,
    },
    Pong,
    Ack,
    Error {
        code: DbErrorCode,
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
    Insert { rowid: i64 },
    Upsert { rowid: i64 },
    Update { affected: u64 },
    Delete { affected: u64 },
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
    UnknownTable,
    UnknownColumn,
    TypeMismatch,
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
    Unsupported,
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
