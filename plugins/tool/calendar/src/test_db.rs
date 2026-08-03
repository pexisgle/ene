//! In-memory mock of the host-service DB IPC server for tests.
//!
//! Shared by the store and action tests: answers the framed `DbRequest`
//! protocol over a Unix socket with an in-memory `calendar_accounts` /
//! `calendar_events` store. Schema declarations are acknowledged without
//! validation, so `from_row` mismatches against the real DDL are not
//! caught here — real-schema verification happens against the actual
//! host-service `db`.

use crate::store::CalendarStore;
use ene_plugin_db::{DbFilter, DbRequest, DbResponse, DbValue, Row};
use ene_plugin_proto::transport::{IpcListener, IpcStream, cleanup_path};
use ene_plugin_proto::{
    HostServiceId, HostServiceRequest, HostServiceResponse, read_host_service_request,
    write_host_service_response,
};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct MockDb {
    accounts: Vec<Row>,
    events: Vec<Row>,
}

impl MockDb {
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn handle_request(&mut self, req: &DbRequest) -> DbResponse {
        match req {
            DbRequest::Handshake { .. } => DbResponse::HandshakeAck,
            DbRequest::DeclareSchema(_) => DbResponse::SchemaAccepted {
                tables: vec![
                    "calendar_accounts".to_string(),
                    "calendar_events".to_string(),
                ],
                indexes: vec!["idx_calendar_events_account_start".to_string()],
            },
            DbRequest::Insert { table, row } => {
                let rows = match table.as_str() {
                    "calendar_accounts" => &mut self.accounts,
                    "calendar_events" => &mut self.events,
                    other => {
                        return DbResponse::Error {
                            code: ene_plugin_db::DbErrorCode::Internal,
                            message: format!("unknown table {other}"),
                        };
                    }
                };
                rows.push(row.clone());
                DbResponse::Insert {
                    rowid: rows.len() as i64,
                }
            }
            DbRequest::Select {
                table,
                filter,
                order_by,
                limit,
                ..
            } => {
                let source = match table.as_str() {
                    "calendar_accounts" => self.accounts.as_slice(),
                    "calendar_events" => self.events.as_slice(),
                    other => {
                        return DbResponse::Error {
                            code: ene_plugin_db::DbErrorCode::Internal,
                            message: format!("unknown table {other}"),
                        };
                    }
                };
                let mut matched: Vec<Row> = source
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
            DbRequest::Update { table, set, filter } => {
                let rows = match table.as_str() {
                    "calendar_accounts" => &mut self.accounts,
                    "calendar_events" => &mut self.events,
                    other => {
                        return DbResponse::Error {
                            code: ene_plugin_db::DbErrorCode::Internal,
                            message: format!("unknown table {other}"),
                        };
                    }
                };
                let mut affected = 0u64;
                for row in rows.iter_mut() {
                    if matches_filter(row, filter) {
                        for (k, v) in set {
                            row.insert(k.clone(), v.clone());
                        }
                        affected += 1;
                    }
                }
                DbResponse::Update { affected }
            }
            DbRequest::Delete { table, filter } => {
                let rows = match table.as_str() {
                    "calendar_accounts" => &mut self.accounts,
                    "calendar_events" => &mut self.events,
                    other => {
                        return DbResponse::Error {
                            code: ene_plugin_db::DbErrorCode::Internal,
                            message: format!("unknown table {other}"),
                        };
                    }
                };
                let before = rows.len();
                rows.retain(|r| !matches_filter(r, filter));
                DbResponse::Delete {
                    affected: (before - rows.len()) as u64,
                }
            }
            DbRequest::Batch { ops } => {
                let mut results = Vec::with_capacity(ops.len());
                for op in ops {
                    match op {
                        ene_plugin_db::DbWriteOp::Delete { table, filter } => {
                            let rows = match table.as_str() {
                                "calendar_accounts" => &mut self.accounts,
                                "calendar_events" => &mut self.events,
                                other => {
                                    return DbResponse::Error {
                                        code: ene_plugin_db::DbErrorCode::Internal,
                                        message: format!("unknown table {other}"),
                                    };
                                }
                            };
                            let before = rows.len();
                            rows.retain(|r| !matches_filter(r, filter));
                            results.push(ene_plugin_db::DbBatchOpResult::Delete {
                                affected: (before - rows.len()) as u64,
                            });
                        }
                        other => {
                            return DbResponse::Error {
                                code: ene_plugin_db::DbErrorCode::Internal,
                                message: format!("unsupported batch op {other:?}"),
                            };
                        }
                    }
                }
                DbResponse::Batch { results }
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
        DbFilter::Always => true,
        DbFilter::And(filters) => filters.iter().all(|f| matches_filter(row, f)),
        DbFilter::Or(filters) => filters.iter().any(|f| matches_filter(row, f)),
        DbFilter::Not(f) => !matches_filter(row, f),
        DbFilter::Eq { column, value } => row.get(column) == Some(value),
        DbFilter::Ne { column, value } => row.get(column) != Some(value),
        DbFilter::Lt { column, value } => compare_values(row.get(column), Some(value)).is_lt(),
        DbFilter::Le { column, value } => compare_values(row.get(column), Some(value)).is_le(),
        DbFilter::Gt { column, value } => compare_values(row.get(column), Some(value)).is_gt(),
        DbFilter::Ge { column, value } => compare_values(row.get(column), Some(value)).is_ge(),
        DbFilter::In { column, values } => row.get(column).is_some_and(|v| values.contains(v)),
        DbFilter::Like { column, pattern } => row
            .get(column)
            .and_then(DbValue::as_str)
            .is_some_and(|v| v.contains(&pattern.replace('%', ""))),
        DbFilter::IsNull { column } => matches!(row.get(column), None | Some(DbValue::Null)),
        DbFilter::IsNotNull { column } => !matches!(row.get(column), None | Some(DbValue::Null)),
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
    let json = serde_json::to_vec(resp).expect("serialize mock response");
    let len = json.len() as u32;
    stream
        .write_all(&len.to_le_bytes())
        .await
        .expect("write len");
    stream.write_all(&json).await.expect("write body");
    stream.flush().await.expect("flush");
}

/// Spawns the mock DB server on a fresh socket; returns the socket path and
/// the server task.
pub async fn spawn_mock_db() -> (PathBuf, tokio::task::JoinHandle<()>) {
    let socket_path = std::env::temp_dir().join(format!(
        "ene-calendar-test-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos()
    ));
    cleanup_path(&socket_path);

    let mut listener = IpcListener::bind(&socket_path).expect("bind mock socket");

    let handle = tokio::spawn(async move {
        let mut db = MockDb::new();
        loop {
            let Ok(mut stream) = listener.accept().await else {
                break;
            };
            match read_host_service_request(&mut stream).await {
                Ok(Some(HostServiceRequest::Open {
                    service: HostServiceId::Db,
                    ..
                })) => {
                    if write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
                        .await
                        .is_err()
                    {
                        continue;
                    }
                }
                _ => continue,
            }
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

/// Connects a [`CalendarStore`] to a fresh mock DB server.
pub async fn make_store() -> (CalendarStore, PathBuf, tokio::task::JoinHandle<()>) {
    let (path, handle) = spawn_mock_db().await;
    let store = CalendarStore::new(&path, Some("test-token"))
        .await
        .expect("connect to mock db");
    (store, path, handle)
}
