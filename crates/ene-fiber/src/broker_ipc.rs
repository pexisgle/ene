//! Broker RPC listener exposed to one plugin process.

use crate::broker::{
    Broker, BrokerError, fs_read, fs_read_bytes, fs_search, fs_write, fs_write_bytes, net_fetch,
};
use crate::fiber::FiberUid;
use base64::{Engine, engine::general_purpose::STANDARD};
use ene_plugin_ipc::{BrokerErrorCode, BrokerRequest, BrokerResponse};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[cfg(unix)]
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(windows)]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

const BROKER_RPC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum BrokerIpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(String),
    #[error("broker: {0}")]
    Broker(#[from] BrokerError),
}

/// Serve broker requests over a platform-local endpoint until stopped.
///
/// The first request must prove possession of the spawn token before the
/// server binds that connection to the activating fiber's broker grants.
pub struct BrokerServer {
    endpoint: String,
    task: tokio::task::JoinHandle<()>,
    #[cfg(unix)]
    socket: Option<PathBuf>,
}

impl BrokerServer {
    /// Bind a platform-local listener for `uid` and dispatch into `broker`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener cannot be bound.
    pub fn bind(
        broker: Arc<parking_lot::Mutex<Broker>>,
        uid: FiberUid,
        row_id: &str,
        token: &str,
    ) -> Result<Self, BrokerIpcError> {
        #[cfg(unix)]
        {
            let socket = crate::spawn::broker_endpoint(row_id);
            if let Err(err) = std::fs::remove_file(&socket)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err.into());
            }
            let listener = UnixListener::bind(&socket)?;
            let token = token.to_owned();
            let task = tokio::spawn(async move {
                if let Err(err) = accept_loop_unix(broker, uid, listener, token).await {
                    tracing::warn!(error = %err, "broker ipc stopped");
                }
            });
            Ok(Self {
                endpoint: socket.to_string_lossy().into_owned(),
                task,
                socket: Some(socket),
            })
        }
        #[cfg(windows)]
        {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            listener.set_nonblocking(true)?;
            let endpoint = listener.local_addr()?.to_string();
            tracing::debug!(row_id = %row_id, "binding broker ipc listener");
            let token = token.to_owned();
            let task = tokio::spawn(async move {
                let Ok(listener) = TcpListener::from_std(listener) else {
                    tracing::warn!("broker ipc stopped");
                    return;
                };
                if let Err(err) = accept_loop_windows(broker, uid, listener, token).await {
                    tracing::warn!(error = %err, "broker ipc stopped");
                }
            });
            Ok(Self { endpoint, task })
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn cleanup(&mut self) {
        self.task.abort();
        #[cfg(unix)]
        if let Some(socket) = self.socket.take() {
            drop(std::fs::remove_file(socket));
        }
    }

    pub fn shutdown(mut self) {
        self.cleanup();
    }
}

impl Drop for BrokerServer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(unix)]
async fn accept_loop_unix(
    broker: Arc<parking_lot::Mutex<Broker>>,
    uid: FiberUid,
    listener: UnixListener,
    token: String,
) -> Result<(), BrokerIpcError> {
    while let Ok((stream, _)) = listener.accept().await {
        let broker = Arc::clone(&broker);
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_connection(stream, broker, uid, &token).await {
                tracing::debug!(error = %err, "broker connection closed");
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
async fn accept_loop_windows(
    broker: Arc<parking_lot::Mutex<Broker>>,
    uid: FiberUid,
    listener: TcpListener,
    token: String,
) -> Result<(), BrokerIpcError> {
    while let Ok((stream, _)) = listener.accept().await {
        let broker = Arc::clone(&broker);
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_connection(stream, broker, uid, &token).await {
                tracing::debug!(error = %err, "broker connection closed");
            }
        });
    }
    Ok(())
}

async fn serve_connection<S>(
    mut stream: S,
    broker: Arc<parking_lot::Mutex<Broker>>,
    uid: FiberUid,
    token: &str,
) -> Result<(), BrokerIpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(BrokerRequest::Hello { token: offered }) =
        ene_plugin_ipc::read_broker_request(&mut stream)
            .await
            .map_err(|err| BrokerIpcError::Codec(err.to_string()))?
    else {
        return Err(BrokerIpcError::Codec(
            "first broker request is not hello".to_owned(),
        ));
    };
    if !constant_time_eq(offered.as_bytes(), token.as_bytes()) {
        let response = BrokerResponse::Error {
            code: BrokerErrorCode::Denied,
            message: "broker hello rejected".to_owned(),
        };
        ene_plugin_ipc::write_broker_response(&mut stream, response)
            .await
            .map_err(|err| BrokerIpcError::Codec(err.to_string()))?;
        return Ok(());
    }
    ene_plugin_ipc::write_broker_response(&mut stream, BrokerResponse::HelloOk)
        .await
        .map_err(|err| BrokerIpcError::Codec(err.to_string()))?;

    while let Some(request) = ene_plugin_ipc::read_broker_request(&mut stream)
        .await
        .map_err(|err| BrokerIpcError::Codec(err.to_string()))?
    {
        let timeout_eligible = is_timeout_eligible_request(&request);
        let worker_broker = Arc::clone(&broker);
        let worker = tokio::task::spawn_blocking(move || dispatch(&worker_broker, uid, request));
        let response = if timeout_eligible {
            match tokio::time::timeout(BROKER_RPC_TIMEOUT, worker).await {
                Ok(result) => join_response(result),
                Err(_) => timeout_response(BROKER_RPC_TIMEOUT),
            }
        } else {
            join_response(worker.await)
        };
        ene_plugin_ipc::write_broker_response(&mut stream, response)
            .await
            .map_err(|err| BrokerIpcError::Codec(err.to_string()))?;
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn is_timeout_eligible_request(request: &BrokerRequest) -> bool {
    matches!(
        request,
        BrokerRequest::FsRead { .. }
            | BrokerRequest::FsReadBytes { .. }
            | BrokerRequest::FsSearch { .. }
    )
}

fn join_response(result: Result<BrokerResponse, tokio::task::JoinError>) -> BrokerResponse {
    match result {
        Ok(response) => response,
        Err(join_err) => BrokerResponse::Error {
            code: BrokerErrorCode::Internal,
            message: format!("broker worker failed: {join_err}"),
        },
    }
}

fn dispatch(
    broker: &parking_lot::Mutex<Broker>,
    uid: FiberUid,
    request: BrokerRequest,
) -> BrokerResponse {
    let workspace = {
        let broker = broker.lock();
        let Some(op) = request_operation(&request) else {
            return BrokerResponse::Error {
                code: BrokerErrorCode::Denied,
                message: "broker hello is accepted once per connection".to_owned(),
            };
        };
        if !broker.has_grant(uid, op) {
            return BrokerResponse::Error {
                code: BrokerErrorCode::Denied,
                message: format!("denied {op} for fiber {uid}"),
            };
        }
        broker.workspace().to_owned()
    };

    match request {
        BrokerRequest::Hello { .. } => BrokerResponse::Error {
            code: BrokerErrorCode::Denied,
            message: "broker hello is accepted once per connection".to_owned(),
        },
        BrokerRequest::FsRead { path } => match fs_read(&workspace, Path::new(&path)) {
            Ok(text) => BrokerResponse::FsReadOk { text },
            Err(err) => error_response(&err),
        },
        BrokerRequest::FsReadBytes { path } => match fs_read_bytes(&workspace, Path::new(&path)) {
            Ok(bytes) => BrokerResponse::FsReadBytesOk {
                bytes_base64: STANDARD.encode(bytes),
            },
            Err(err) => error_response(&err),
        },
        BrokerRequest::FsWrite { path, text } => match fs_write(&workspace, Path::new(&path), &text) {
            Ok(()) => BrokerResponse::FsWriteOk,
            Err(err) => error_response(&err),
        },
        BrokerRequest::FsWriteBytes { path, bytes_base64 } => {
            let bytes = match STANDARD.decode(bytes_base64) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return BrokerResponse::Error {
                        code: BrokerErrorCode::InvalidArgument,
                        message: format!("invalid base64: {err}"),
                    };
                }
            };
            match fs_write_bytes(&workspace, Path::new(&path), &bytes) {
                Ok(()) => BrokerResponse::FsWriteBytesOk,
                Err(err) => error_response(&err),
            }
        }
        BrokerRequest::FsSearch {
            path,
            query,
            regex,
            case_insensitive,
            include,
            context_lines,
            count,
            max,
        } => match fs_search(
            &workspace,
            Path::new(&path),
            &query,
            regex,
            case_insensitive,
            include.as_deref(),
            context_lines,
            count,
            max,
        ) {
            Ok(matches) => BrokerResponse::FsSearchOk { matches },
            Err(err) => error_response(&err),
        },
        BrokerRequest::NetFetch { url } => match net_fetch(&url) {
            Ok(value) => BrokerResponse::NetFetchOk { value },
            Err(err) => error_response(&err),
        },
    }
}

fn request_operation(request: &BrokerRequest) -> Option<&'static str> {
    match request {
        BrokerRequest::Hello { .. } => None,
        BrokerRequest::FsRead { .. } | BrokerRequest::FsReadBytes { .. } => Some("fs.read"),
        BrokerRequest::FsWrite { .. } | BrokerRequest::FsWriteBytes { .. } => Some("fs.write"),
        BrokerRequest::FsSearch { .. } => Some("fs.search"),
        BrokerRequest::NetFetch { .. } => Some("net.fetch"),
    }
}

pub(crate) fn timeout_response(timeout: Duration) -> BrokerResponse {
    BrokerResponse::Error {
        code: BrokerErrorCode::Timeout,
        message: format!("broker operation timed out after {timeout:?}"),
    }
}

fn error_response(err: &BrokerError) -> BrokerResponse {
    BrokerResponse::Error {
        code: match err {
            BrokerError::Denied { .. } => BrokerErrorCode::Denied,
            BrokerError::InvalidGlob(_) => BrokerErrorCode::InvalidGlob,
            BrokerError::InvalidRegex(_) => BrokerErrorCode::InvalidRegex,
            BrokerError::Io(_) => BrokerErrorCode::Io,
            BrokerError::Timeout => BrokerErrorCode::Timeout,
            BrokerError::InvalidUrl(_) => BrokerErrorCode::InvalidUrl,
            BrokerError::Ssrf(_) => BrokerErrorCode::Ssrf,
            BrokerError::Fetch(_) | BrokerError::RedirectLoop => BrokerErrorCode::Fetch,
            BrokerError::SearchEngineUnavailable => BrokerErrorCode::SearchEngineUnavailable,
            BrokerError::PathEscape(_) => BrokerErrorCode::PathEscape,
            BrokerError::Oversize | BrokerError::Binary => BrokerErrorCode::Oversize,
            BrokerError::Symlink => BrokerErrorCode::Symlink,
            BrokerError::NotEmpty => BrokerErrorCode::DirectoryNotEmpty,
            BrokerError::ReadOnly => BrokerErrorCode::ReadOnlyPath,
            _ => BrokerErrorCode::Internal,
        },
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{error_response, is_timeout_eligible_request, timeout_response};
    use crate::broker::BrokerError;
    use ene_plugin_ipc::{BrokerErrorCode, BrokerRequest, BrokerResponse};
    use std::time::Duration;

    #[test]
    fn broker_errors_are_machine_classified() {
        let cases = [
            (
                BrokerError::InvalidGlob("../x".to_owned()),
                BrokerErrorCode::InvalidGlob,
            ),
            (
                BrokerError::InvalidRegex("(".to_owned()),
                BrokerErrorCode::InvalidRegex,
            ),
            (BrokerError::Oversize, BrokerErrorCode::Oversize),
            (BrokerError::Symlink, BrokerErrorCode::Symlink),
            (BrokerError::NotEmpty, BrokerErrorCode::DirectoryNotEmpty),
            (BrokerError::ReadOnly, BrokerErrorCode::ReadOnlyPath),
            (
                BrokerError::SearchEngineUnavailable,
                BrokerErrorCode::SearchEngineUnavailable,
            ),
        ];
        for (error, expected) in cases {
            let response = error_response(&error);
            assert!(matches!(
                response,
                BrokerResponse::Error { code, .. } if code == expected
            ));
        }
        assert_eq!(
            timeout_response(Duration::from_secs(1)),
            BrokerResponse::Error {
                code: BrokerErrorCode::Timeout,
                message: "broker operation timed out after 1s".to_owned(),
            }
        );
    }

    #[test]
    fn only_read_only_file_operations_use_outer_timeout() {
        assert!(is_timeout_eligible_request(&BrokerRequest::FsRead {
            path: "a".to_owned(),
        }));
        assert!(is_timeout_eligible_request(&BrokerRequest::FsReadBytes {
            path: "a".to_owned(),
        }));
        assert!(is_timeout_eligible_request(&BrokerRequest::FsSearch {
            path: ".".to_owned(),
            query: "x".to_owned(),
            regex: false,
            case_insensitive: false,
            include: None,
            context_lines: 0,
            count: false,
            max: 1,
        }));
        assert!(!is_timeout_eligible_request(&BrokerRequest::FsWrite {
            path: "a".to_owned(),
            text: "x".to_owned(),
        }));
        assert!(!is_timeout_eligible_request(&BrokerRequest::FsWriteBytes {
            path: "a".to_owned(),
            bytes_base64: "eA==".to_owned(),
        }));
        assert!(!is_timeout_eligible_request(&BrokerRequest::NetFetch {
            url: "https://example.invalid".to_owned(),
        }));
    }
}
