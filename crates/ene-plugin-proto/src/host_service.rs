//! Host-service channel wire types.
//!
//! Plugins connect outbound to a single host-service socket. The first
//! framed message selects a service and authenticates; subsequent messages
//! use that service's own request/response types (for example `DbRequest`
//! after [`HostServiceId::Db`]).

use crate::frame::{read_framed_json, write_framed_json};
use crate::transport::IpcStream;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

/// Maximum framed message size on the host-service channel (64 MiB).
pub use crate::frame::MAX_FRAMED_MESSAGE_SIZE as HOST_SERVICE_MAX_MESSAGE_SIZE;

/// Identifies a service multiplexed on the host-service socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostServiceId {
    /// Typed CRUD against the host `memory.db` (`ene-plugin-db`).
    Db,
    /// Reserved: host-mediated asset provisioning (not yet implemented).
    Assets,
    /// Reserved: capability mediation (not yet implemented).
    Capability,
    /// Credential / secret retrieval (implemented by the host's
    /// `CredentialPassenger`; see the `ene-plugin-host` credential service).
    Credential,
}

/// Requests sent on a new host-service connection before a service session
/// is established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostServiceRequest {
    /// Authenticate and open a session for `service`.
    ///
    /// Must be the first message on every connection. After
    /// [`HostServiceResponse::OpenAck`], the stream speaks only that
    /// service's protocol.
    Open {
        /// Service to open.
        service: HostServiceId,
        /// Pre-shared token issued by the host for this plugin.
        token: String,
    },
}

/// Responses to [`HostServiceRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostServiceResponse {
    /// The service session is open; subsequent frames use the service protocol.
    OpenAck,
    /// The open request was rejected.
    Error {
        /// Structured error code.
        code: HostServiceErrorCode,
        /// Human-readable detail.
        message: String,
    },
}

/// Structured errors for host-service open failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostServiceErrorCode {
    /// The requested service id is unknown or not implemented yet.
    UnknownService,
    /// The auth token was missing or did not match a registration.
    AuthRejected,
    /// An internal host error occurred while opening the session.
    Internal,
}

/// A passenger service multiplexed on the host-service socket.
///
/// Signature-only abstraction over the services a host can open (`db`,
/// `credential`, …): the implementor — not the socket acceptor — owns
/// authentication and the service protocol for its [`HostServiceId`]. The
/// acceptor selects a passenger after the `Open` frame and hands it the raw
/// stream plus the presented token; the passenger writes its own `Open`
/// response and then speaks the service protocol until the connection ends.
#[async_trait]
pub trait HostServicePassenger: Send + Sync {
    /// Serves one connection for this passenger's service.
    ///
    /// `token` is the pre-shared token from the `Open` frame. The implementor
    /// authenticates it, writes the `OpenAck`/`Error` response, and then owns
    /// `stream` until the session ends.
    async fn serve(&self, stream: IpcStream, token: String);
}

/// Writes a length-prefixed JSON [`HostServiceRequest`].
pub async fn write_host_service_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &HostServiceRequest,
) -> std::io::Result<()> {
    write_framed_json(writer, request).await
}

/// Reads a length-prefixed JSON [`HostServiceRequest`].
pub async fn read_host_service_request<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<HostServiceRequest>> {
    read_framed_json(reader).await
}

/// Writes a length-prefixed JSON [`HostServiceResponse`].
pub async fn write_host_service_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &HostServiceResponse,
) -> std::io::Result<()> {
    write_framed_json(writer, response).await
}

/// Reads a length-prefixed JSON [`HostServiceResponse`].
pub async fn read_host_service_response<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<HostServiceResponse>> {
    read_framed_json(reader).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_request_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = HostServiceRequest::Open {
            service: HostServiceId::Db,
            token: "ene-db-deadbeef".into(),
        };
        write_host_service_request(&mut a, &req).await.unwrap();
        drop(a);
        let got = read_host_service_request(&mut b).await.unwrap().unwrap();
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn open_ack_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_host_service_response(&mut a, &HostServiceResponse::OpenAck)
            .await
            .unwrap();
        drop(a);
        let got = read_host_service_response(&mut b).await.unwrap().unwrap();
        assert_eq!(got, HostServiceResponse::OpenAck);
    }

    #[test]
    fn service_ids_serialize_snake_case() {
        let json = serde_json::to_string(&HostServiceId::Db).unwrap();
        assert_eq!(json, "\"db\"");
        let json = serde_json::to_string(&HostServiceId::Credential).unwrap();
        assert_eq!(json, "\"credential\"");
    }
}
