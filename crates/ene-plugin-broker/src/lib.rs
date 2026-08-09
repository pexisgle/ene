//! # ene-plugin-broker
//!
//! Plugin-side client for the protocol-v8 broker channel. A plugin opens one
//! authenticated session per host service (`file`, `network`, `process`,
//! `credential`, `artifact`, `platform`) on the shared host-service socket
//! and exchanges typed [`BrokerRequest`] / [`BrokerResponse`] frames. The
//! host pins the plugin identity to the channel, so a plugin can never
//! impersonate another one.
//!
//! Everything is mediated: the plugin never opens sockets, user files, or
//! processes directly.

#![warn(missing_docs)]

use std::path::Path;

pub use ene_plugin_proto::{
    BrokerRequest, BrokerResponse, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, IpcStream, read_framed_json, read_host_service_response,
    write_framed_json, write_host_service_request,
};
use tokio::io::AsyncWriteExt;

/// Errors from the broker channel.
#[derive(Debug, thiserror::Error)]
pub enum BrokerClientError {
    /// The socket connection failed.
    #[error("broker connect failed: {0}")]
    Connect(String),
    /// The `Open` handshake was rejected.
    #[error("broker open rejected: {message} ({code:?})")]
    OpenRejected {
        /// Error code from the host.
        code: HostServiceErrorCode,
        /// Human-readable detail.
        message: String,
    },
    /// A broker request failed.
    #[error("broker request failed: {0}")]
    Request(String),
    /// The host returned a structured broker error.
    #[error("broker error {code:?}: {message}")]
    Denied {
        /// Structured code.
        code: ene_plugin_proto::BrokerErrorCode,
        /// Human-readable detail.
        message: String,
    },
    /// I/O failure on the channel.
    #[error("broker I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The peer closed the channel.
    #[error("broker channel closed by host")]
    Closed,
}

/// One authenticated broker session.
pub struct BrokerClient {
    stream: IpcStream,
}

impl std::fmt::Debug for BrokerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerClient").finish_non_exhaustive()
    }
}

impl BrokerClient {
    /// Connects to the host-service socket and opens `service`.
    pub async fn connect(
        socket_path: &Path,
        token: &str,
        service: HostServiceId,
    ) -> Result<Self, BrokerClientError> {
        let mut stream = IpcStream::connect(socket_path)
            .await
            .map_err(|e| BrokerClientError::Connect(e.to_string()))?;
        write_host_service_request(
            &mut stream,
            &HostServiceRequest::Open {
                service,
                token: token.to_string(),
            },
        )
        .await?;
        match read_host_service_response(&mut stream).await? {
            Some(HostServiceResponse::OpenAck) => Ok(Self { stream }),
            Some(HostServiceResponse::Error { code, message }) => {
                Err(BrokerClientError::OpenRejected { code, message })
            }
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Sends one request and returns the matching response.
    pub async fn request(
        &mut self,
        request: &BrokerRequest,
    ) -> Result<BrokerResponse, BrokerClientError> {
        write_framed_json(&mut self.stream, request).await?;
        match read_framed_json(&mut self.stream).await? {
            Some(BrokerResponse::Error { code, message }) => {
                Err(BrokerClientError::Denied { code, message })
            }
            Some(response) => Ok(response),
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Shuts the session down cleanly.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.stream.shutdown().await
    }
}

// Re-export the wire types so plugin authors import one crate.
pub use ene_plugin_proto::{
    ArtifactInfo, BrokerErrorCode, ConflictMode, FileEntry, HttpMethod, WireArtifactKind,
};
