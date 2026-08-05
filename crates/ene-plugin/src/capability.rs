//! Client for the host-service `capability` passenger.
//!
//! A consumer plugin that declared `requires` entries opens a capability
//! session against the shared host-service socket and invokes provider
//! capabilities through the host. The socket path and auth token come from
//! [`SandboxConfigData`](ene_plugin_proto::SandboxConfigData)
//! (`host_service_socket` / `db_auth_token`), delivered via
//! [`ConfigurablePlugin::set_sandbox`](crate::ConfigurablePlugin::set_sandbox).

use std::path::Path;

use ene_plugin_proto::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityServiceRequest,
    CapabilityServiceResponse, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, IpcStream, read_capability_service_response, read_host_service_response,
    write_capability_service_request, write_host_service_request,
};
use thiserror::Error;

/// Errors from opening and using a capability host-service session.
#[derive(Debug, Error)]
pub enum CapabilityClientError {
    /// IO or transport error.
    #[error("transport error: {0}")]
    Transport(#[from] std::io::Error),
    /// The server closed the connection before the response.
    #[error("connection closed")]
    ConnectionClosed,
    /// The server returned an unexpected response variant.
    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),
    /// The host rejected the session `Open` (bad token or unknown service).
    #[error("host service open rejected: {message}")]
    Open {
        /// The rejection code from the host.
        code: HostServiceErrorCode,
        /// Human-readable rejection detail.
        message: String,
    },
}

/// Client for the host-service `capability` passenger.
///
/// One session is one connection with strict request/response alternation
/// (mirroring the `db` passenger); calls must not be pipelined on a single
/// client.
pub struct CapabilityClient {
    stream: IpcStream,
}

impl CapabilityClient {
    /// Opens an authenticated `capability` session on the host-service socket.
    ///
    /// `token` is the plugin's host-service auth token (`db_auth_token` in
    /// [`SandboxConfigData`](ene_plugin_proto::SandboxConfigData)); the host
    /// derives the caller's identity from it.
    pub async fn open(socket_path: &Path, token: &str) -> Result<Self, CapabilityClientError> {
        let mut stream = IpcStream::connect(socket_path).await?;
        write_host_service_request(
            &mut stream,
            &HostServiceRequest::Open {
                service: HostServiceId::Capability,
                token: token.to_string(),
            },
        )
        .await?;
        match read_host_service_response(&mut stream).await? {
            Some(HostServiceResponse::OpenAck) => Ok(Self { stream }),
            Some(HostServiceResponse::Error { code, message }) => {
                Err(CapabilityClientError::Open { code, message })
            }
            None => Err(CapabilityClientError::ConnectionClosed),
        }
    }

    /// Invokes one capability method through the host.
    ///
    /// The host authenticates the call against the caller's declared
    /// `requires`, resolves the provider, and forwards the call; the returned
    /// error carries the stable capability-call code (forbidden / no provider
    /// / provider failure / timeout / transport).
    pub async fn call(
        &mut self,
        call: &CapabilityCall,
    ) -> Result<serde_json::Value, CapabilityCallError> {
        write_capability_service_request(
            &mut self.stream,
            &CapabilityServiceRequest::Call { call: call.clone() },
        )
        .await
        .map_err(|e| CapabilityCallError::new(CapabilityCallErrorCode::Transport, e.to_string()))?;
        match read_capability_service_response(&mut self.stream).await {
            Ok(Some(CapabilityServiceResponse::Result { result })) => result,
            Ok(None) => Err(CapabilityCallError::new(
                CapabilityCallErrorCode::Transport,
                "host service closed the capability session",
            )),
            Err(e) => Err(CapabilityCallError::new(
                CapabilityCallErrorCode::Transport,
                e.to_string(),
            )),
        }
    }
}
