//! IPC client used by tool binaries to perform typed CRUD operations against
//! the host-service `db` passenger over a length-prefixed JSON connection.

use crate::messages::{DbBatchOpResult, DbErrorCode, DbRequest, DbResponse, DbWriteOp};
use crate::types::{DbFilter, DbOrderBy, DbSchema, DbValue, Row};
use ene_plugin_proto::transport::IpcStream;
use ene_plugin_proto::{
    HostServiceErrorCode, HostServiceId, HostServiceRequest, HostServiceResponse,
    read_host_service_response, write_host_service_request,
};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Errors that can occur when communicating with the DB IPC server.
#[derive(Error, Debug)]
pub enum DbError {
    /// IO or transport error.
    #[error("transport error: {0}")]
    Transport(#[from] std::io::Error),
    /// Server returned an unexpected response variant.
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),
    /// The connection was closed by the server.
    #[error("connection closed")]
    ConnectionClosed,
    /// The server rejected the auth token presented on Handshake.
    #[error("auth rejected [{code}]: {message}")]
    Auth {
        /// The error code from the server.
        code: DbErrorCode,
        /// Human-readable error message.
        message: String,
    },
    /// The tool does not have permission to access the requested resource.
    #[error("permission denied: {message}")]
    PermissionDenied {
        /// Human-readable error message.
        message: String,
    },
    /// The specified table does not exist or is not declared.
    #[error("unknown table: {message}")]
    UnknownTable {
        /// Human-readable error message.
        message: String,
    },
    /// The specified column does not exist.
    #[error("unknown column: {message}")]
    UnknownColumn {
        /// Human-readable error message.
        message: String,
    },
    /// A value's type does not match the column's declared type.
    #[error("type mismatch: {message}")]
    TypeMismatch {
        /// Human-readable error message.
        message: String,
    },
    /// The filter expression is invalid.
    #[error("invalid filter: {message}")]
    InvalidFilter {
        /// Human-readable error message.
        message: String,
    },
    /// The declared schema conflicts with the stored schema in a way that
    /// cannot be applied automatically (e.g. a column type change).
    #[error("schema conflict: {message}")]
    SchemaConflict {
        /// Human-readable error message.
        message: String,
    },
    /// A write was rejected because it would exceed the plugin's configured
    /// DB storage quota (`plugins.list.<name>.db_quota_mb`). Reads and
    /// deletes remain available so the plugin can free space.
    #[error("storage quota exceeded: {message}")]
    QuotaExceeded {
        /// Human-readable error message.
        message: String,
    },
    /// An internal server error occurred.
    #[error("internal server error: {message}")]
    Internal {
        /// Human-readable error message.
        message: String,
    },
    /// An error code this build does not know about (emitted by a newer
    /// host). The error is surfaced as a diagnostic rather than being
    /// dropped wholesale.
    #[error("unknown database error: {message}")]
    Unknown {
        /// Human-readable error message.
        message: String,
    },
}

/// Client for communicating with the host-service `db` passenger.
pub struct DbClient {
    stream: IpcStream,
    /// Socket path captured at connect-time so the client can be
    /// [`reconnect`](Self::reconnect)ed after the server restarts.
    socket_path: std::path::PathBuf,
    /// Optional auth token captured at connect-time. `Some` for
    /// `connect_with_token` clients, `None` for the unauthenticated
    /// `connect` variant.
    auth_token: Option<String>,
}

impl DbClient {
    /// Connects to the host-service socket without opening a service
    /// session. Callers must follow with [`Self::open_db_service`].
    pub(crate) async fn connect(socket_path: &Path) -> Result<Self, DbError> {
        let stream = IpcStream::connect(socket_path).await?;
        Ok(Self {
            stream,
            socket_path: socket_path.to_path_buf(),
            auth_token: None,
        })
    }

    /// Connects to the host-service socket and opens the `db` passenger
    /// with the pre-shared auth token. Returns an error if the host
    /// rejects the token or the `db` service is unavailable.
    pub async fn connect_with_token(socket_path: &Path, token: &str) -> Result<Self, DbError> {
        let mut client = Self::connect(socket_path).await?;
        client.open_db_service(token).await?;
        client.auth_token = Some(token.to_string());
        Ok(client)
    }

    /// Path the IPC client connected to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Re-establishes the IPC connection after the server has restarted.
    ///
    /// A `ConnectionClosed` error would otherwise force callers to drop the
    /// client and rebuild it from scratch, losing every cached
    /// `schema`/`rowid` state. Because `socket_path` and `auth_token` are
    /// captured at connect-time, the same client can be brought back online
    /// here.
    ///
    /// If the new connection succeeds but the host-service open fails
    /// (e.g. auth rejected), the old stream is preserved so subsequent
    /// retries don't operate on an unauthenticated socket.
    pub async fn reconnect(&mut self) -> Result<(), DbError> {
        let new_stream = IpcStream::connect(&self.socket_path).await?;
        let old_stream = std::mem::replace(&mut self.stream, new_stream);
        if let Some(token) = self.auth_token.clone() {
            match self.open_db_service(&token).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    self.stream = old_stream;
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }

    async fn open_db_service(&mut self, token: &str) -> Result<(), DbError> {
        write_host_service_request(
            &mut self.stream,
            &HostServiceRequest::Open {
                service: HostServiceId::Db,
                token: token.to_string(),
            },
        )
        .await?;
        match read_host_service_response(&mut self.stream).await? {
            Some(HostServiceResponse::OpenAck) => Ok(()),
            Some(HostServiceResponse::Error { code, message }) => Err(DbError::Auth {
                code: match code {
                    HostServiceErrorCode::AuthRejected | HostServiceErrorCode::UnknownService => {
                        DbErrorCode::PermissionDenied
                    }
                    HostServiceErrorCode::Internal => DbErrorCode::Internal,
                },
                message,
            }),
            None => Err(DbError::ConnectionClosed),
        }
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
        if let DbResponse::Error { code, message } = resp {
            let err = match code {
                DbErrorCode::PermissionDenied => DbError::PermissionDenied { message },
                DbErrorCode::UnknownTable => DbError::UnknownTable { message },
                DbErrorCode::UnknownColumn => DbError::UnknownColumn { message },
                DbErrorCode::TypeMismatch => DbError::TypeMismatch { message },
                DbErrorCode::InvalidFilter => DbError::InvalidFilter { message },
                DbErrorCode::SchemaConflict => DbError::SchemaConflict { message },
                DbErrorCode::QuotaExceeded => DbError::QuotaExceeded { message },
                DbErrorCode::Unknown => DbError::Unknown { message },
                DbErrorCode::Internal => DbError::Internal { message },
            };
            return Err(err);
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
                conflict_columns: conflict_columns
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
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
        order_by: &[DbOrderBy],
        limit: Option<u64>,
    ) -> Result<Vec<Row>, DbError> {
        let resp = Self::check_error(
            self.send_request(&DbRequest::Select {
                table: table.to_string(),
                columns: columns
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                filter,
                order_by: order_by.to_vec(),
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

    /// Applies a group of write operations atomically and returns one
    /// [`DbBatchOpResult`] per operation, in the same order as `ops`.
    ///
    /// The server executes the whole batch inside a single `SQLite`
    /// transaction: either every operation commits, or the entire batch is
    /// rolled back and nothing is persisted. On rollback the server returns
    /// an [`Err`](DbError) naming the failing operation, so a partial batch
    /// can never be observed. Use this whenever several writes must stand or
    /// fall together (for example, recording a multi-row undo entry) instead
    /// of issuing the writes one request at a time.
    ///
    /// The server holds its write lock for the duration of the batch and
    /// rejects batches larger than its operation limit (10,000) up front, so
    /// keep batches bounded: split large workloads into multiple `batch`
    /// calls rather than sending one giant list.
    pub async fn batch(&mut self, ops: Vec<DbWriteOp>) -> Result<Vec<DbBatchOpResult>, DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::Batch { ops }).await?)?;
        match resp {
            DbResponse::Batch { results } => Ok(results),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Batch, got {other:?}"
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

    /// Most recent `rowid` from an INSERT on this connection.
    pub async fn last_insert_rowid(&mut self) -> Result<i64, DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::LastInsertRowId).await?)?;
        match resp {
            DbResponse::LastInsertRowId { rowid } => Ok(rowid),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected LastInsertRowId, got {other:?}"
            ))),
        }
    }

    /// Health-check round-trip to the IPC server.
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
    ///
    /// Returns the server's response so a server that already died is
    /// detectable: ignoring the result would report success even when the
    /// connection was closed.
    pub async fn shutdown(&mut self) -> Result<(), DbError> {
        let resp = Self::check_error(self.send_request(&DbRequest::Shutdown).await?)?;
        match resp {
            DbResponse::Ack => Ok(()),
            other => Err(DbError::UnexpectedResponse(format!(
                "expected Ack, got {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_error_maps_all_error_codes_to_typed_variants() {
        let err_codes = vec![
            (DbErrorCode::PermissionDenied, "Permission Denied message"),
            (DbErrorCode::UnknownTable, "Unknown Table message"),
            (DbErrorCode::UnknownColumn, "Unknown Column message"),
            (DbErrorCode::TypeMismatch, "Type Mismatch message"),
            (DbErrorCode::InvalidFilter, "Invalid Filter message"),
            (DbErrorCode::SchemaConflict, "Schema Conflict message"),
            (DbErrorCode::Internal, "Internal message"),
        ];

        for (code, msg) in err_codes {
            let resp = DbResponse::Error {
                code: code.clone(),
                message: msg.to_string(),
            };
            let res = DbClient::check_error(resp);
            if let Err(err) = res {
                match (code.clone(), err) {
                    (DbErrorCode::PermissionDenied, DbError::PermissionDenied { message }) => {
                        assert_eq!(message, "Permission Denied message");
                    }
                    (DbErrorCode::UnknownTable, DbError::UnknownTable { message }) => {
                        assert_eq!(message, "Unknown Table message");
                    }
                    (DbErrorCode::UnknownColumn, DbError::UnknownColumn { message }) => {
                        assert_eq!(message, "Unknown Column message");
                    }
                    (DbErrorCode::TypeMismatch, DbError::TypeMismatch { message }) => {
                        assert_eq!(message, "Type Mismatch message");
                    }
                    (DbErrorCode::InvalidFilter, DbError::InvalidFilter { message }) => {
                        assert_eq!(message, "Invalid Filter message");
                    }
                    (DbErrorCode::SchemaConflict, DbError::SchemaConflict { message }) => {
                        assert_eq!(message, "Schema Conflict message");
                    }
                    (DbErrorCode::Internal, DbError::Internal { message }) => {
                        assert_eq!(message, "Internal message");
                    }
                    _ => {
                        let actual_code = format!("{code:?}");
                        assert_eq!(actual_code, "None", "Unexpected error mapping");
                    }
                }
            } else {
                let got_error = false;
                assert!(got_error, "Expected error response");
            }
        }
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "reconnect harness panics on protocol or IO failure"
    )]
    mod reconnect_auth {
        use super::*;
        use ene_plugin_proto::transport::{IpcListener, IpcStream, cleanup_path};
        use ene_plugin_proto::{
            HostServiceId, HostServiceRequest, HostServiceResponse, read_host_service_request,
            write_host_service_response,
        };
        use std::path::PathBuf;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn read_framed(stream: &mut IpcStream) -> DbRequest {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.expect("read len");
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await.expect("read body");
            serde_json::from_slice(&buf).expect("decode request")
        }

        async fn write_framed(stream: &mut IpcStream, resp: &DbResponse) {
            let json = serde_json::to_vec(resp).expect("encode response");
            let len = json.len() as u32;
            stream
                .write_all(&len.to_le_bytes())
                .await
                .expect("write len");
            stream.write_all(&json).await.expect("write body");
            stream.flush().await.expect("flush");
        }

        async fn serve_authed_session(stream: &mut IpcStream, expected_token: &str) {
            match read_host_service_request(stream).await.expect("read open") {
                Some(HostServiceRequest::Open {
                    service: HostServiceId::Db,
                    token,
                }) => {
                    assert_eq!(token, expected_token, "open token mismatch");
                    write_host_service_response(stream, &HostServiceResponse::OpenAck)
                        .await
                        .expect("write open ack");
                }
                other => panic!("expected Open(Db), got {other:?}"),
            }
            match read_framed(stream).await {
                DbRequest::Ping => write_framed(stream, &DbResponse::Pong).await,
                other => panic!("expected Ping, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn connect_with_token_captures_token_for_reconnect() {
            let socket_path: PathBuf = std::env::temp_dir().join(format!(
                "ene-plugin-db-reconnect-{}.sock",
                std::process::id()
            ));
            cleanup_path(&socket_path);

            let expected_token = "test-token-secret";
            let path_for_server = socket_path.clone();
            let token_for_server = expected_token.to_string();

            let server = tokio::spawn(async move {
                let mut listener = IpcListener::bind(&path_for_server).expect("bind");
                // First connection: initial connect_with_token + ping.
                let mut stream = listener.accept().await.expect("accept first");
                serve_authed_session(&mut stream, &token_for_server).await;
                drop(stream);

                // Second connection: reconnect + ping (must re-handshake).
                let mut stream = listener.accept().await.expect("accept second");
                serve_authed_session(&mut stream, &token_for_server).await;
            });

            // Wait briefly for the listener to be ready.
            for _ in 0..50 {
                if socket_path.exists() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let mut client = DbClient::connect_with_token(&socket_path, expected_token)
                .await
                .expect("connect_with_token");
            client.ping().await.expect("ping after connect");

            // Drop the first socket from the client side by reconnecting.
            client.reconnect().await.expect("reconnect re-handshakes");
            client.ping().await.expect("ping after reconnect");

            server.await.expect("server task");
            cleanup_path(&socket_path);
        }
    }

    #[cfg(unix)]
    #[expect(
        clippy::expect_used,
        clippy::panic,
        reason = "batch wire harness panics on protocol or IO failure"
    )]
    mod batch_wire {
        use super::*;
        use ene_plugin_proto::transport::{IpcListener, cleanup_path};
        use ene_plugin_proto::{
            HostServiceId, HostServiceRequest, HostServiceResponse, read_host_service_request,
            write_host_service_response,
        };
        use std::path::PathBuf;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn read_framed(stream: &mut ene_plugin_proto::transport::IpcStream) -> DbRequest {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.expect("read len");
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await.expect("read body");
            serde_json::from_slice(&buf).expect("decode request")
        }

        async fn write_framed(
            stream: &mut ene_plugin_proto::transport::IpcStream,
            resp: &DbResponse,
        ) {
            let json = serde_json::to_vec(resp).expect("encode response");
            let len = json.len() as u32;
            stream
                .write_all(&len.to_le_bytes())
                .await
                .expect("write len");
            stream.write_all(&json).await.expect("write body");
            stream.flush().await.expect("flush");
        }

        #[tokio::test]
        async fn batch_round_trips_over_ipc() {
            let socket_path: PathBuf = std::env::temp_dir()
                .join(format!("ene-plugin-db-batch-{}.sock", std::process::id()));
            cleanup_path(&socket_path);

            let token = "batch-token";
            let path_for_server = socket_path.clone();
            let token_for_server = token.to_string();

            let server = tokio::spawn(async move {
                let mut listener = IpcListener::bind(&path_for_server).expect("bind");
                let mut stream = listener.accept().await.expect("accept");

                match read_host_service_request(&mut stream)
                    .await
                    .expect("read open")
                {
                    Some(HostServiceRequest::Open {
                        service: HostServiceId::Db,
                        token,
                    }) => {
                        assert_eq!(token, token_for_server, "open token mismatch");
                        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
                            .await
                            .expect("write open ack");
                    }
                    other => panic!("expected Open(Db), got {other:?}"),
                }

                match read_framed(&mut stream).await {
                    DbRequest::Batch { ops } => {
                        assert_eq!(ops.len(), 2, "expected two ops on the wire");
                        assert!(matches!(ops[0], DbWriteOp::Insert { .. }));
                        assert!(matches!(ops[1], DbWriteOp::Delete { .. }));
                        write_framed(
                            &mut stream,
                            &DbResponse::Batch {
                                results: vec![
                                    DbBatchOpResult::Insert { rowid: 7 },
                                    DbBatchOpResult::Delete { affected: 3 },
                                ],
                            },
                        )
                        .await;
                    }
                    other => panic!("expected Batch, got {other:?}"),
                }
            });

            for _ in 0..50 {
                if socket_path.exists() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let mut client = DbClient::connect_with_token(&socket_path, token)
                .await
                .expect("connect_with_token");
            let results = client
                .batch(vec![
                    DbWriteOp::Insert {
                        table: "fs_test".to_string(),
                        row: std::collections::BTreeMap::from([(
                            "name".to_string(),
                            DbValue::Text("x".to_string()),
                        )]),
                    },
                    DbWriteOp::Delete {
                        table: "fs_test".to_string(),
                        filter: DbFilter::Always,
                    },
                ])
                .await
                .expect("batch");

            assert_eq!(
                results,
                vec![
                    DbBatchOpResult::Insert { rowid: 7 },
                    DbBatchOpResult::Delete { affected: 3 },
                ]
            );

            server.await.expect("server task");
            cleanup_path(&socket_path);
        }
    }
}
