//! Capability-call wire types and the host-service `capability` passenger.
//!
//! A consumer plugin that declared `requires` for a capability opens the
//! `capability` passenger on the shared host-service socket and sends
//! [`CapabilityServiceRequest::Call`]. The host resolves the provider from
//! the capability registry, forwards the same [`CapabilityCall`] over the
//! provider's plugin IPC connection, and returns the provider's
//! [`CapabilityCallResult`] unchanged — so the two hops share one canonical
//! body and one stable error vocabulary.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::CapabilityRef;
use crate::transport::IpcStream;

/// One call from a consumer plugin to a capability provided by another
/// plugin.
///
/// `capability` is the provider's declared reference (`gguf-runner@1`),
/// `method` and `payload` are defined by that capability's contract — the
/// host treats the payload as opaque and never interprets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityCall {
    /// Capability reference being called (`name@major`).
    pub capability: CapabilityRef,
    /// Method name defined by the capability contract (e.g. `generate`).
    pub method: String,
    /// Method-specific JSON payload, opaque to the host.
    pub payload: serde_json::Value,
}

/// Stable failure categories for capability calls.
///
/// The same codes appear on both hops (provider IPC response and host-service
/// response), so a consumer sees the provider's failure class unchanged.
///
/// The wire vocabulary is stable and documented, but the Rust enum is
/// `#[non_exhaustive]` so adding a code later never breaks exhaustive matches
/// in consumer builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CapabilityCallErrorCode {
    /// The calling plugin did not declare a matching `requires` entry.
    Forbidden,
    /// No provider for the requested capability is registered.
    NoProvider,
    /// The capability reference or method payload is malformed.
    InvalidRequest,
    /// The provider does not serve the capability or method (or its binary
    /// predates capability calls).
    NotSupported,
    /// The provider failed while executing the call.
    Provider,
    /// The call exceeded the provider connection's request timeout.
    Timeout,
    /// The provider connection failed or the provider crashed.
    Transport,
    /// An internal host error.
    Internal,
}

/// A capability call failure with a stable code and a diagnostic message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCallError {
    /// Stable failure category.
    pub code: CapabilityCallErrorCode,
    /// Human-readable diagnostic detail.
    pub message: String,
}

impl CapabilityCallError {
    /// Creates an error with the given category and message.
    #[must_use]
    pub fn new(code: CapabilityCallErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Result of a capability call: the provider's JSON response or a typed
/// error. Shared by the provider IPC hop and the host-service passenger.
pub type CapabilityCallResult = Result<serde_json::Value, CapabilityCallError>;

/// Requests on the host-service `capability` passenger.
///
/// After [`crate::HostServiceResponse::OpenAck`], each frame is one call and
/// the peer answers with exactly one [`CapabilityServiceResponse`] (strict
/// request/response alternation, like the `db` passenger).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CapabilityServiceRequest {
    /// Invoke a capability method through the host.
    Call {
        /// The call to mediate.
        call: CapabilityCall,
    },
}

/// Responses on the host-service `capability` passenger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CapabilityServiceResponse {
    /// Outcome of the preceding [`CapabilityServiceRequest::Call`].
    Result {
        /// The mediated result.
        result: CapabilityCallResult,
    },
}

/// Writes a length-prefixed JSON [`CapabilityServiceRequest`].
pub async fn write_capability_service_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &CapabilityServiceRequest,
) -> std::io::Result<()> {
    write_framed_json(writer, request).await
}

/// Reads a length-prefixed JSON [`CapabilityServiceRequest`].
pub async fn read_capability_service_request<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<CapabilityServiceRequest>> {
    read_framed_json(reader).await
}

/// Writes a length-prefixed JSON [`CapabilityServiceResponse`].
pub async fn write_capability_service_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &CapabilityServiceResponse,
) -> std::io::Result<()> {
    write_framed_json(writer, response).await
}

/// Reads a length-prefixed JSON [`CapabilityServiceResponse`].
pub async fn read_capability_service_response<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<CapabilityServiceResponse>> {
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
    if msg_len > crate::host_service::HOST_SERVICE_MAX_MESSAGE_SIZE {
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

/// Server-side interface for the host-service `capability` passenger.
///
/// Implemented by the host's mediation layer ([`CapabilityMediator`] in
/// `ene-plugin-host`); the shared host-service acceptor in `ene-store`
/// authenticates the session and hands the stream to this interface. The
/// trait lives here because both crates need it and neither may depend on the
/// other; it is a wire-session interface with no business logic.
#[async_trait]
pub trait CapabilityServiceHandler: Send + Sync {
    /// Serves one authenticated capability session until EOF or an I/O error.
    ///
    /// `consumer` is the plugin name derived from the session's auth token.
    async fn serve(&self, stream: IpcStream, consumer: String) -> std::io::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_call() -> CapabilityCall {
        CapabilityCall {
            capability: CapabilityRef::parse("gguf-runner@1").unwrap(),
            method: "generate".into(),
            payload: serde_json::json!({ "model": "stories260K", "prompt": "Once" }),
        }
    }

    #[test]
    fn capability_call_roundtrip() {
        let call = sample_call();
        let json = serde_json::to_string(&call).unwrap();
        let deser: CapabilityCall = serde_json::from_str(&json).unwrap();
        assert_eq!(call, deser);
    }

    #[test]
    fn service_request_roundtrip() {
        let req = CapabilityServiceRequest::Call {
            call: sample_call(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deser: CapabilityServiceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deser);
    }

    #[test]
    fn service_response_ok_roundtrip() {
        let resp = CapabilityServiceResponse::Result {
            result: Ok(serde_json::json!({ "text": "hello" })),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deser: CapabilityServiceResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deser);
    }

    #[test]
    fn service_response_error_roundtrip() {
        let resp = CapabilityServiceResponse::Result {
            result: Err(CapabilityCallError::new(
                CapabilityCallErrorCode::Forbidden,
                "no matching requires declaration",
            )),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deser: CapabilityServiceResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deser);
    }

    #[test]
    fn error_codes_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&CapabilityCallErrorCode::NoProvider).unwrap(),
            "\"no_provider\""
        );
        assert_eq!(
            serde_json::to_string(&CapabilityCallErrorCode::NotSupported).unwrap(),
            "\"not_supported\""
        );
    }

    #[tokio::test]
    async fn framing_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = CapabilityServiceRequest::Call {
            call: sample_call(),
        };
        write_capability_service_request(&mut a, &req)
            .await
            .unwrap();
        drop(a);
        let got = read_capability_service_request(&mut b)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, req);
    }
}
