//! Host-mediated network session for the `OpenAI` TTS plugin.
//!
//! Every synthesis request goes through the `Network` broker: the host
//! validates SSRF, resolves origin approvals, injects the API credential by
//! key name, and enforces size caps. The plugin never holds a socket, DNS,
//! or the credential value.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerClientError, BrokerRequest, HttpMethod};
use ene_plugin_proto::{HostServiceId, PluginError, SandboxConfigData};
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// One mediated HTTP response body.
#[derive(Debug)]
pub struct FetchOutcome {
    /// HTTP status.
    pub status: u16,
    /// Response headers (authorization-like headers are stripped by the
    /// host unless the host itself injected them).
    pub headers: Vec<(String, String)>,
    /// Response body (size-capped by the host).
    pub body: Vec<u8>,
}

/// Lazily-connected `Network` broker session shared by every request.
///
/// The socket and auth token arrive through
/// [`ConfigurablePlugin::set_sandbox`]; the session opens on the first
/// request and is reused for the process lifetime.
pub struct OpenAiTtsBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl OpenAiTtsBroker {
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

    /// Drops any cached session so a reconfiguration takes effect. Tests
    /// switch sockets between cases; production reconnects rebuild the
    /// session through the same path on the next request.
    #[cfg(test)]
    pub(crate) async fn reset(&self) {
        *self.client.lock().await = None;
    }

    /// Sends one request and returns the response.
    pub async fn fetch(
        &self,
        method: HttpMethod,
        url: &str,
        headers: Vec<(String, String)>,
        credential: Option<&str>,
        credential_header: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<FetchOutcome, BrokerClientError> {
        let mut client = self
            .session()
            .await
            .map_err(|e| BrokerClientError::Request(e.to_string()))?;
        let Some(client) = client.as_mut() else {
            return Err(BrokerClientError::Request(
                "broker client initialization failed".to_string(),
            ));
        };
        let response = client
            .request(&BrokerRequest::NetworkFetch {
                method,
                url: url.to_string(),
                headers,
                credential: credential.map(str::to_string),
                credential_header: credential_header.map(str::to_string),
                body,
                max_bytes: None,
            })
            .await?;
        match response {
            ene_plugin_broker::BrokerResponse::NetworkFetchOk {
                status,
                headers,
                body,
            } => Ok(FetchOutcome {
                status,
                headers,
                body,
            }),
            other => Err(BrokerClientError::Request(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Opens the broker session on first use.
    async fn session(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<BrokerClient>>, PluginError> {
        let mut client = self.client.lock().await;
        if client.is_none() {
            let socket = self.socket.read().clone();
            let token = self.token.read().clone();
            let (Some(socket), Some(token)) = (socket, token) else {
                return Err(PluginError::provider(
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
                .map_err(|e| PluginError::provider(format!("broker connect failed: {e}")))?,
            );
        }
        Ok(client)
    }
}

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<OpenAiTtsBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<OpenAiTtsBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(OpenAiTtsBroker::new())))
}

/// Configures the shared broker from the host sandbox data.
pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}
