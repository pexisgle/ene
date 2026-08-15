//! Host-mediated network session for the calc plugin.
//!
//! Currency conversion goes through the `Network` broker: the host
//! validates SSRF, resolves origin approvals, re-checks redirects, and
//! enforces size caps. The plugin never opens a socket or resolves DNS
//! itself.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerRequest, HttpMethod};
use ene_plugin_proto::{HostServiceId, SandboxConfigData, ToolError};
use parking_lot::RwLock;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct FetchOutcome {
    pub status: u16,
    /// Response body (size-capped by the host).
    pub body: Vec<u8>,
}

/// Lazily-connected `Network` broker session shared by every action.
pub struct CalcBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl CalcBroker {
    /// A broker with no connection configuration yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
            client: Mutex::new(None),
        }
    }

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, sandbox: &SandboxConfigData) {
        self.socket.write().clone_from(&sandbox.broker_socket);
        self.token.write().clone_from(&sandbox.db_auth_token);
    }

    pub async fn fetch(&self, url: &str) -> Result<FetchOutcome, ToolError> {
        let mut client = self
            .session()
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker connect failed: {e}")))?;
        let Some(client) = client.as_mut() else {
            return Err(ToolError::execution_failed(
                "broker client initialization failed",
            ));
        };
        let response = client
            .request(&BrokerRequest::NetworkFetch {
                method: HttpMethod::Get,
                url: url.to_string(),
                headers: Vec::new(),
                credential: None,
                credential_header: None,
                body: None,
                max_bytes: None,
            })
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker request failed: {e}")))?;
        match response {
            ene_plugin_broker::BrokerResponse::NetworkFetchOk { status, body, .. } => {
                Ok(FetchOutcome { status, body })
            }
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Opens the broker session on first use.
    async fn session(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<BrokerClient>>, ToolError> {
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
        Ok(client)
    }
}

impl Default for CalcBroker {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<CalcBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<CalcBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(CalcBroker::new())))
}

pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}
