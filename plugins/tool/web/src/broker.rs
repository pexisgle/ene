//! Broker session for the web plugin.
//!
//! All HTTP traffic is mediated by the host through the `Network` broker:
//! the host validates SSRF, resolves origin approvals, re-checks every
//! redirect hop, and enforces size caps. The plugin only sends the request
//! and consumes the response.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerRequest, BrokerResponse, HttpMethod};
use ene_plugin_proto::{HostServiceId, SandboxConfigData, ToolError};
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// One mediated HTTP response.
pub struct FetchOutcome {
    /// HTTP status code.
    pub status: u16,
    /// Response headers (authorization/cookie headers are stripped by the
    /// host).
    pub headers: Vec<(String, String)>,
    /// Response body (size-capped by the host).
    pub body: Vec<u8>,
}

impl FetchOutcome {
    /// The `Content-Type` header value, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.headers.iter().find_map(|(key, value)| {
            (key.eq_ignore_ascii_case("content-type")).then_some(value.as_str())
        })
    }
}

/// Lazily-connected `Network` broker session shared by every action.
///
/// The socket and auth token arrive through `set_sandbox`; the session is
/// opened on the first request and reused for the process lifetime.
pub struct WebBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl std::fmt::Debug for WebBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebBroker").finish_non_exhaustive()
    }
}

impl Default for WebBroker {
    fn default() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
            client: Mutex::new(None),
        }
    }
}

impl WebBroker {
    /// A broker with no connection configuration yet.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, sandbox: &SandboxConfigData) {
        self.socket.write().clone_from(&sandbox.broker_socket);
        self.token.write().clone_from(&sandbox.db_auth_token);
    }

    /// Sends one mediated HTTP request.
    pub async fn fetch(
        &self,
        method: HttpMethod,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        max_bytes: u64,
    ) -> Result<FetchOutcome, ToolError> {
        let mut client = self.client.lock().await;
        if client.is_none() {
            let socket = self.socket.read().clone();
            let token = self.token.read().clone();
            let (Some(socket), Some(token)) = (socket, token) else {
                return Err(ToolError::execution_failed(
                    "broker channel is not configured (missing broker socket/token from the host)",
                ));
            };
            *client = Some(
                BrokerClient::connect(
                    std::path::Path::new(&socket),
                    &token,
                    HostServiceId::Network,
                )
                .await
                .map_err(|e| ToolError::execution_failed(format!("broker connect failed: {e}")))?,
            );
        }
        let Some(client) = client.as_mut() else {
            return Err(ToolError::execution_failed(
                "broker client initialization failed",
            ));
        };
        let response = client
            .request(&BrokerRequest::NetworkFetch {
                method,
                url: url.to_string(),
                headers,
                credential: None,
                body,
                max_bytes: Some(max_bytes),
            })
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker request failed: {e}")))?;
        match response {
            BrokerResponse::NetworkFetchOk {
                status,
                headers,
                body,
            } => Ok(FetchOutcome {
                status,
                headers,
                body,
            }),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }
}
