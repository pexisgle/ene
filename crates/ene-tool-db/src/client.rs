use crate::messages::{DbErrorCode, DbRequest, DbResponse};
use crate::types::{DbFilter, DbOrderBy, DbSchema, DbValue, Row};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Errors that can occur when communicating with the DB IPC server.
#[derive(Error, Debug)]
pub enum DbError {
    /// IO or transport error on the Unix socket.
    #[error("transport error: {0}")]
    Transport(#[from] std::io::Error),
    /// Server returned an error response.
    #[error("server error [{code}]: {message}")]
    Server {
        /// The error code from the server.
        code: DbErrorCode,
        /// Human-readable error message.
        message: String,
    },
    /// Server returned an unexpected response variant.
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),
    /// The connection was closed by the server.
    #[error("connection closed")]
    ConnectionClosed,
}

/// Client for communicating with the per-tool DB IPC server over a Unix socket.
pub struct DbClient {
    stream: UnixStream,
}

impl DbClient {
    /// Connects to the DB IPC server at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self, DbError> {
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self { stream })
    }

    async fn send_request(&mut self, req: &DbRequest) -> Result<DbResponse, DbError> {
        let json = serde_json::to_vec(req).map_err(|e| {
            DbError::Transport(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("serialize failed: {e}"),
            ))
        })?;
        let len = json.len() as u32;
        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&json).await?;
        self.stream.flush().await?;

        let mut len_buf = [0u8; 4];
        match self.stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(DbError::ConnectionClosed);
            }
            Err(e) => return Err(DbError::Transport(e)),
        }
        let resp_len = u32::from_le_bytes(len_buf) as usize;
        if resp_len > MAX_MESSAGE_SIZE {
            return Err(DbError::Transport(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("response too large: {resp_len}"),
            )));
        }
        let mut buf = vec![0u8; resp_len];
        self.stream.read_exact(&mut buf).await?;
        let resp: DbResponse = serde_json::from_slice(&buf).map_err(|e| {
            DbError::Transport(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("deserialize failed: {e}"),
            ))
        })?;
        Ok(resp)
    }

    fn check_error(resp: DbResponse) -> Result<DbResponse, DbError> {
        if let DbResponse::Error { code, message } = &resp {
            return Err(DbError::Server {
                code: code.clone(),
                message: message.clone(),
            });
        }
        Ok(resp)
    }

    /// Declares the tool's database schema (tables and indexes).
    /// Returns the list of created table and index names.
    pub async fn declare_schema(
        &mut self,
        schema: DbSchema,
    ) -> Result<(Vec<String>, Vec<String>), DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::DeclareSchema(schema)).await?)?;
        match resp {
            DbResponse::SchemaAccepted { tables, indexes } => Ok((tables, indexes)),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected SchemaAccepted, got {other:?}"
            ))),
        }
    }

    /// Inserts a row into the given table. Returns the new row's `rowid`.
    pub async fn insert(&mut self, table: &str, row: Row) -> Result<i64, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Insert {
                table: table.to_string(),
                row,
            })
            .await?,
        )?;
        match resp {
            DbResponse::Insert { rowid } => Ok(rowid),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Insert, got {other:?}"
            ))),
        }
    }

    /// Inserts or updates a row on conflict. Returns the `rowid`.
    pub async fn upsert(
        &mut self,
        table: &str,
        row: Row,
        conflict_columns: &[&str],
    ) -> Result<i64, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Upsert {
                table: table.to_string(),
                row,
                conflict_columns: conflict_columns.iter().map(|s| s.to_string()).collect(),
            })
            .await?,
        )?;
        match resp {
            DbResponse::Upsert { rowid } => Ok(rowid),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Upsert, got {other:?}"
            ))),
        }
    }

    /// Selects rows matching the filter, ordered and limited as specified.
    pub async fn select(
        &mut self,
        table: &str,
        columns: &[&str],
        filter: DbFilter,
        order_by: Vec<DbOrderBy>,
        limit: Option<u64>,
    ) -> Result<Vec<Row>, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Select {
                table: table.to_string(),
                columns: columns.iter().map(|s| s.to_string()).collect(),
                filter,
                order_by,
                limit,
            })
            .await?,
        )?;
        match resp {
            DbResponse::Select { rows } => Ok(rows),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Select, got {other:?}"
            ))),
        }
    }

    /// Updates rows matching the filter with the given column values.
    pub async fn update(
        &mut self,
        table: &str,
        set: BTreeMap<String, DbValue>,
        filter: DbFilter,
    ) -> Result<u64, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Update {
                table: table.to_string(),
                set,
                filter,
            })
            .await?,
        )?;
        match resp {
            DbResponse::Update { affected } => Ok(affected),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Update, got {other:?}"
            ))),
        }
    }

    /// Deletes rows matching the filter. Returns the number of affected rows.
    pub async fn delete(&mut self, table: &str, filter: DbFilter) -> Result<u64, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Delete {
                table: table.to_string(),
                filter,
            })
            .await?,
        )?;
        match resp {
            DbResponse::Delete { affected } => Ok(affected),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Delete, got {other:?}"
            ))),
        }
    }

    /// Counts rows matching the filter.
    pub async fn count(&mut self, table: &str, filter: DbFilter) -> Result<i64, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Count {
                table: table.to_string(),
                filter,
            })
            .await?,
        )?;
        match resp {
            DbResponse::Count { count } => Ok(count),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Count, got {other:?}"
            ))),
        }
    }

    /// Returns the most recently inserted `rowid` on this connection.
    pub async fn last_insert_rowid(&mut self) -> Result<i64, DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::LastInsertRowId).await?)?;
        match resp {
            DbResponse::LastInsertRowId { rowid } => Ok(rowid),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected LastInsertRowId, got {other:?}"
            ))),
        }
    }

    /// Sends a health-check ping to the server.
    pub async fn ping(&mut self) -> Result<(), DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::Ping).await?)?;
        match resp {
            DbResponse::Pong => Ok(()),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Pong, got {other:?}"
            ))),
        }
    }

    /// Requests a graceful shutdown of the DB server.
    pub async fn shutdown(&mut self) -> Result<(), DbError> {
        let _ = self.send_request(&DbRequest::Shutdown).await;
        Ok(())
    }
}
