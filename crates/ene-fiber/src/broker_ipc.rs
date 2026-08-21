//! Broker RPC listener exposed to one plugin process.

use crate::broker::{Broker, BrokerError};
use crate::fiber::FiberUid;
use ene_plugin_ipc::{BrokerErrorCode, BrokerRequest, BrokerResponse};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
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

/// Serve broker requests over a local socket until the plugin disconnects.
///
/// The socket path is returned so the supervisor can advertise it to the
/// plugin. Only the granted fiber uid can access the shared [`Broker`].
pub struct BrokerServer {
    socket: PathBuf,
    task: tokio::task::JoinHandle<()>,
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
    ) -> Result<Self, BrokerIpcError> {
        let socket = crate::spawn::broker_endpoint(row_id);
        if let Err(err) = std::fs::remove_file(&socket)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err.into());
        }
        let listener = UnixListener::bind(&socket)?;
        let task = tokio::spawn(async move {
            if let Err(err) = accept_loop(broker, uid, listener).await {
                tracing::warn!(error = %err, "broker ipc stopped");
            }
        });
        Ok(Self { socket, task })
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn shutdown(self) {
        self.task.abort();
        drop(std::fs::remove_file(&self.socket));
    }
}

async fn accept_loop(
    broker: Arc<parking_lot::Mutex<Broker>>,
    uid: FiberUid,
    listener: UnixListener,
) -> Result<(), BrokerIpcError> {
    loop {
        let (stream, _) = listener.accept().await?;
        let broker = Arc::clone(&broker);
        tokio::spawn(async move {
            if let Err(err) = serve_connection(stream, broker, uid).await {
                tracing::debug!(error = %err, "broker connection closed");
            }
        });
    }
}

async fn serve_connection<S>(
    mut stream: S,
    broker: Arc<parking_lot::Mutex<Broker>>,
    uid: FiberUid,
) -> Result<(), BrokerIpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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

fn dispatch(
    broker: &parking_lot::Mutex<Broker>,
    uid: FiberUid,
    request: BrokerRequest,
) -> BrokerResponse {
    match request {
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
