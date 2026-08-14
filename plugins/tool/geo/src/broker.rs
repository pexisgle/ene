//! Host-mediated network session for the geo plugin.
//!
//! Every API request goes through the `Network` broker: the host validates
//! SSRF, resolves origin approvals, re-checks redirects, and enforces size
//! caps. The plugin never opens a socket or resolves DNS itself.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerRequest, HttpMethod};
use ene_plugin_proto::{HostServiceId, SandboxConfigData, ToolError};
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// One mediated HTTP response body.
#[derive(Debug)]
pub struct FetchOutcome {
    /// HTTP status.
    pub status: u16,
    /// Response body (size-capped by the host).
    pub body: Vec<u8>,
}

/// Lazily-connected `Network` broker session shared by every action.
///
/// The socket and auth token arrive through `set_sandbox`; the session
/// opens on the first request and is reused for the process lifetime.
pub struct GeoBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl GeoBroker {
    /// A broker with no connection configuration yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
            client: Mutex::new(None),
        }
    }
}

impl Default for GeoBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl GeoBroker {

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, sandbox: &SandboxConfigData) {
        self.socket.write().clone_from(&sandbox.broker_socket);
        self.token.write().clone_from(&sandbox.db_auth_token);
    }

    /// Sends one request and returns the response.
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
            ene_plugin_broker::BrokerResponse::NetworkFetchOk {
                status,
                body,
                ..
            } => Ok(FetchOutcome { status, body }),
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

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<GeoBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<GeoBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(GeoBroker::new())))
}

/// Configures the shared broker from the host sandbox data.
pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ene_plugin_broker::{BrokerRequest, BrokerResponse, read_framed_json, write_framed_json};
    use ene_plugin_proto::{
        HostServiceRequest, HostServiceResponse, read_host_service_request,
        write_host_service_response,
    };

    /// A scripted response, consumed FIFO by the mock.
    pub struct MockResponse {
        status: u16,
        body: Vec<u8>,
    }

    impl MockResponse {
        /// A `200 OK` response with a fixed-length body.
        #[must_use]
        pub fn ok(body: Vec<u8>) -> Self {
            Self {
                status: 200,
                body,
            }
        }
    }

    /// Broker-frame mock of the geo APIs: records nothing, serves scripted
    /// responses. Tests reset the shared broker and point it at the mock's
    /// socket.
    pub struct MockBroker {
        socket: std::path::PathBuf,
        responses: std::sync::Arc<
            std::sync::Mutex<std::collections::VecDeque<MockResponse>>,
        >,
        _dir: tempfile::TempDir,
    }

    impl MockBroker {
        /// Spawns the mock on a fresh unix socket.
        #[must_use]
        pub fn spawn() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let socket = dir.path().join("geo-mock.sock");
            let responses =
                std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
            let server = Self {
                socket,
                responses: std::sync::Arc::clone(&responses),
                _dir: dir,
            };
            tokio::spawn(run_server(
                server.socket.clone(),
                std::sync::Arc::clone(&responses),
            ));
            server
        }

        /// Queues a response for the next request.
        pub fn push(&self, response: MockResponse) {
            self.responses
                .lock()
                .expect("response queue")
                .push_back(response);
        }
    }

    /// Points the shared broker at `mock`'s socket, dropping any cached
    /// session first.
    pub async fn configure_test_broker(mock: &MockBroker) {
        // The mock task runs on a runtime worker; under parallel test load
        // it can take a while to be scheduled, so poll with real sleeps.
        for _ in 0..200 {
            if mock.socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        broker()
            .client
            .lock()
            .await
            .take();
        broker().configure(&SandboxConfigData {
            broker_socket: Some(mock.socket.to_string_lossy().into_owned()),
            db_auth_token: Some("tok".to_string()),
            ..SandboxConfigData::default()
        });
    }

    async fn run_server(
        socket: std::path::PathBuf,
        responses: std::sync::Arc<
            std::sync::Mutex<std::collections::VecDeque<MockResponse>>,
        >,
    ) {
        let listener = tokio::net::UnixListener::bind(&socket).expect("mock bind");
        let (mut stream, _) = listener.accept().await.expect("mock accept");
        let open: HostServiceRequest = read_host_service_request(&mut stream)
            .await
            .expect("mock open")
            .expect("open frame");
        assert!(matches!(
            open,
            HostServiceRequest::Open {
                service: HostServiceId::Network,
                ..
            }
        ));
        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
            .await
            .expect("mock ack");
        loop {
            let Ok(Some(_request)) =
                read_framed_json::<_, BrokerRequest>(&mut stream).await
            else {
                return;
            };
            let response = responses
                .lock()
                .expect("response queue")
                .pop_front()
                .unwrap_or_else(|| panic!("mock response queue exhausted"));
            write_framed_json(
                &mut stream,
                &BrokerResponse::NetworkFetchOk {
                    status: response.status,
                    headers: Vec::new(),
                    body: response.body,
                },
            )
            .await
            .expect("mock response");
        }
    }
}
