//! Broker RPC listener exposed to one plugin process.

use crate::broker::{Broker, BrokerError};
use crate::fiber::FiberUid;
use ene_plugin_ipc::{BrokerErrorCode, BrokerRequest, BrokerResponse};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

#[cfg(unix)]
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(windows)]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

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

    pub fn shutdown(self) {
        self.task.abort();
        #[cfg(unix)]
        if let Some(socket) = self.socket {
            drop(std::fs::remove_file(socket));
        }
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
        let response = dispatch(&broker, uid, request);
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

fn dispatch(
    broker: &parking_lot::Mutex<Broker>,
    uid: FiberUid,
    request: BrokerRequest,
) -> BrokerResponse {
    match request {
        BrokerRequest::Hello { .. } => BrokerResponse::Error {
            code: BrokerErrorCode::Denied,
            message: "broker hello is accepted once per connection".to_owned(),
        },
        BrokerRequest::FsRead { path } => {
            let broker = broker.lock();
            match broker.fs_read(uid, Path::new(&path)) {
                Ok(text) => BrokerResponse::FsReadOk { text },
                Err(err) => error_response(&err),
            }
        }
        BrokerRequest::FsWrite { path, text } => {
            let broker = broker.lock();
            match broker.fs_write(uid, Path::new(&path), &text) {
                Ok(()) => BrokerResponse::FsWriteOk,
                Err(err) => error_response(&err),
            }
        }
        BrokerRequest::NetFetch { url } => {
            let broker = broker.lock();
            match broker.net_fetch(uid, &url) {
                Ok(value) => BrokerResponse::NetFetchOk { value },
                Err(err) => error_response(&err),
            }
        }
    }
}

fn error_response(err: &BrokerError) -> BrokerResponse {
    BrokerResponse::Error {
        code: match &err {
            BrokerError::Denied { .. } => BrokerErrorCode::Denied,
            BrokerError::PathEscape(_) => BrokerErrorCode::PathEscape,
            BrokerError::Io(_) => BrokerErrorCode::Io,
            BrokerError::InvalidUrl(_) => BrokerErrorCode::InvalidUrl,
            BrokerError::Ssrf(_) => BrokerErrorCode::Ssrf,
            BrokerError::Fetch(_) => BrokerErrorCode::Fetch,
            _ => BrokerErrorCode::Internal,
        },
        message: err.to_string(),
    }
}
