//! Host-service channel wire types.
//!
//! Plugins connect outbound to a single host-service socket. The first
//! framed message selects a service and authenticates; subsequent messages
//! use that service's own request/response types (for example `DbRequest`
//! after [`HostServiceId::Db`]).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum framed message size on the host-service channel (64 MiB).
pub const HOST_SERVICE_MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

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
    /// Reserved: credential / secret retrieval (not yet implemented).
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

async fn write_framed_json<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let json = serde_json::to_vec(value).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("serialize failed: {e}"),
        )
    })?;
    let Ok(len) = u32::try_from(json.len()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large to frame",
        ));
    };
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_framed_json<R, T>(reader: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let Ok(msg_len) = usize::try_from(u32::from_le_bytes(len_buf)) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message length overflow on this platform",
        ));
    };
    if msg_len > HOST_SERVICE_MAX_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("message too large: {msg_len}"),
        ));
    }
    let mut msg_buf = vec![0u8; msg_len];
    reader.read_exact(&mut msg_buf).await?;
    let value = serde_json::from_slice(&msg_buf).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid JSON: {e}"),
        )
    })?;
    Ok(Some(value))
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
