//! Host-mediated network session for the `OpenAI` provider plugin.
//!
//! Every HTTP request goes through the `Network` broker: the host validates
//! SSRF, resolves origin approvals, re-checks redirects, injects the API
//! credential by key name, and enforces size caps. The plugin never holds a
//! socket, DNS, or the credential value.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerClientError, BrokerRequest, HttpMethod, StreamSink};
use ene_plugin_proto::{HostServiceId, PluginError, SandboxConfigData};
use parking_lot::RwLock;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;

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

/// A streamed HTTP response: status/headers plus body chunks.
pub struct StreamSession {
    /// HTTP status.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Body chunks in order; an `Err` item is terminal.
    pub chunks: ReceiverStream<Result<Vec<u8>, PluginError>>,
    /// The task driving the broker exchange. It self-terminates on stream
    /// end, transport error, or when the chunk receiver is dropped, so a
    /// dropped session never leaks a blocked read.
    _task: tokio::task::JoinHandle<()>,
}

/// Lazily-connected `Network` broker session shared by every request.
///
/// The socket and auth token arrive through
/// [`ConfigurablePlugin::set_sandbox`]; the session opens on the first
/// request and is reused for the process lifetime.
pub struct OpenAiBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl OpenAiBroker {
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

    /// Sends one non-streaming request and returns the response.
    pub async fn fetch(
        &self,
        method: HttpMethod,
        url: &str,
        headers: Vec<(String, String)>,
        credential: Option<&str>,
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
                credential_header: None,
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

    /// Starts a streamed request. The returned session carries the response
    /// status/headers and yields body chunks as they arrive; transport or
    /// policy errors surface as the first `Err` chunk and/or a failed status
    /// delivery.
    pub async fn stream(
        self: &Arc<Self>,
        method: HttpMethod,
        url: &str,
        headers: Vec<(String, String)>,
        credential: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Result<StreamSession, PluginError> {
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let start_tx = Some(start_tx);
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, PluginError>>(64);
        let broker = Arc::clone(self);
        let request = BrokerRequest::NetworkFetchStream {
            method,
            url: url.to_string(),
            headers,
            credential: credential.map(str::to_string),
            credential_header: None,
            body,
            max_bytes: None,
        };
        let task = tokio::spawn(async move {
            let mut sink = ChannelSink {
                start: start_tx,
                chunks: Some(chunk_tx),
            };
            let result = match broker.session().await {
                Ok(mut client) => match client.as_mut() {
                    Some(client) => client.stream_events(&request, &mut sink).await,
                    None => Err(BrokerClientError::Request(
                        "broker client initialization failed".to_string(),
                    )),
                },
                Err(e) => Err(BrokerClientError::Request(e.to_string())),
            };
            if let Err(e) = result {
                sink.fail(format!("{e}")).await;
            }
        });

        let (status, headers) =
            match tokio::time::timeout(std::time::Duration::from_mins(2), start_rx).await {
                Ok(Ok(Ok((status, headers)))) => (status, headers),
                Ok(Ok(Err(message))) => {
                    task.abort();
                    return Err(PluginError::provider(format!(
                        "broker stream failed: {message}"
                    )));
                }
                Ok(Err(_)) | Err(_) => {
                    task.abort();
                    return Err(PluginError::provider(
                        "broker stream ended before the response started",
                    ));
                }
            };
        Ok(StreamSession {
            status,
            headers,
            chunks: ReceiverStream::new(chunk_rx),
            _task: task,
        })
    }
}

/// Forwards broker stream frames into the plugin's channel: the response
/// start goes to a oneshot (status/headers for retry decisions), body
/// chunks go to the bounded chunk channel with backpressure.
type StartChannel = tokio::sync::oneshot::Sender<Result<(u16, Vec<(String, String)>), String>>;
type ChunkChannel = tokio::sync::mpsc::Sender<Result<Vec<u8>, PluginError>>;

struct ChannelSink {
    start: Option<StartChannel>,
    chunks: Option<ChunkChannel>,
}

impl StreamSink for ChannelSink {
    async fn start(
        &mut self,
        status: u16,
        headers: Vec<(String, String)>,
    ) -> Result<(), BrokerClientError> {
        if let Some(tx) = self.start.take() {
            drop(tx.send(Ok((status, headers))));
        }
        Ok(())
    }

    async fn chunk(&mut self, data: Vec<u8>) -> Result<(), BrokerClientError> {
        let Some(tx) = self.chunks.as_ref() else {
            return Err(BrokerClientError::Closed);
        };
        tx.send(Ok(data))
            .await
            .map_err(|_| BrokerClientError::Closed)
    }
}

impl ChannelSink {
    /// Delivers a terminal error to the caller and closes the chunk stream.
    async fn fail(&mut self, message: String) {
        if let Some(tx) = self.start.take() {
            drop(tx.send(Err(message.clone())));
        }
        if let Some(tx) = self.chunks.take() {
            drop(
                tx.send(Err(PluginError::provider(format!(
                    "broker stream failed: {message}"
                ))))
                .await,
            );
        }
    }
}

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<OpenAiBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<OpenAiBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(OpenAiBroker::new())))
}

/// Configures the shared broker from the host sandbox data.
pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use expect/panic for concise assertions"
)]
mod tests {
    use super::*;
    use ene_plugin_broker::{BrokerRequest, BrokerResponse};
    use ene_plugin_proto::{
        HostServiceRequest, HostServiceResponse, read_framed_json, write_framed_json,
        write_host_service_response,
    };
    use serde_json::json;
    use tokio::net::UnixListener;
    use tokio_stream::StreamExt as _;

    /// Mock host-service server: answers the `Network` open handshake, then
    /// reads one request, asserts its shape, and replies with `frames`.
    async fn run_mock(
        socket: std::path::PathBuf,
        expected: impl FnOnce(BrokerRequest) + Send,
        frames: Vec<BrokerResponse>,
    ) {
        let listener = UnixListener::bind(&socket).expect("bind");
        let (mut stream, _) = listener.accept().await.expect("accept");
        let open: HostServiceRequest = read_framed_json(&mut stream)
            .await
            .expect("open")
            .expect("frame");
        assert!(matches!(
            open,
            HostServiceRequest::Open {
                service: HostServiceId::Network,
                ..
            }
        ));
        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
            .await
            .expect("ack");
        let request: BrokerRequest = read_framed_json(&mut stream)
            .await
            .expect("request")
            .expect("frame");
        expected(request);
        for frame in frames {
            write_framed_json(&mut stream, &frame).await.expect("frame");
        }
    }

    fn test_sandbox(socket: &std::path::Path) -> SandboxConfigData {
        SandboxConfigData {
            broker_socket: Some(socket.to_string_lossy().into_owned()),
            db_auth_token: Some("tok".to_string()),
            ..SandboxConfigData::default()
        }
    }

    async fn wait_for_socket(path: &std::path::Path) {
        // The mock task runs on a runtime worker; under parallel test load
        // it can take a while to be scheduled, so poll with real sleeps
        // rather than a fixed number of yields.
        for _ in 0..200 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_request_names_the_credential_and_forwards_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("openai-stream.sock");
        let server = tokio::spawn(run_mock(
            socket.clone(),
            |request| {
                let BrokerRequest::NetworkFetchStream {
                    method,
                    url,
                    headers,
                    credential,
                    body,
                    ..
                } = request
                else {
                    panic!("expected NetworkFetchStream, got {request:?}");
                };
                assert_eq!(method, HttpMethod::Post);
                assert_eq!(url, "https://api.openai.com/v1/chat/completions");
                assert_eq!(credential.as_deref(), Some("api_key"));
                assert_eq!(
                    headers,
                    vec![("Content-Type".to_string(), "application/json".to_string())]
                );
                let body: serde_json::Value =
                    serde_json::from_slice(&body.expect("body")).expect("json");
                assert_eq!(body["model"], "gpt-4o-mini");
            },
            vec![
                BrokerResponse::StreamStart {
                    status: 200,
                    headers: vec![],
                },
                BrokerResponse::StreamChunk {
                    data: b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n".to_vec(),
                },
                BrokerResponse::StreamEnd,
            ],
        ));
        wait_for_socket(&socket).await;
        let broker = Arc::new(OpenAiBroker::new());
        broker.configure(&test_sandbox(&socket));

        let session = broker
            .stream(
                HttpMethod::Post,
                "https://api.openai.com/v1/chat/completions",
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some("api_key"),
                Some(serde_json::to_vec(&json!({"model": "gpt-4o-mini"})).expect("json")),
            )
            .await
            .expect("stream");
        assert_eq!(session.status, 200);
        let chunks = session.chunks.collect::<Vec<_>>().await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].as_ref().expect("chunk"),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"
        );
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_surfaces_non_ok_status_with_body() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("openai-fetch.sock");
        let server = tokio::spawn(run_mock(
            socket.clone(),
            |request| {
                let BrokerRequest::NetworkFetch {
                    method,
                    url,
                    credential,
                    ..
                } = request
                else {
                    panic!("expected NetworkFetch, got {request:?}");
                };
                assert_eq!(method, HttpMethod::Post);
                assert_eq!(url, "https://api.openai.com/v1/embeddings");
                assert_eq!(credential.as_deref(), Some("api_key"));
            },
            vec![BrokerResponse::NetworkFetchOk {
                status: 401,
                headers: vec![],
                body: br#"{"error":{"message":"bad key"}}"#.to_vec(),
            }],
        ));
        wait_for_socket(&socket).await;
        let broker = Arc::new(OpenAiBroker::new());
        broker.configure(&test_sandbox(&socket));

        let outcome = broker
            .fetch(
                HttpMethod::Post,
                "https://api.openai.com/v1/embeddings",
                vec![],
                Some("api_key"),
                Some(b"{}".to_vec()),
            )
            .await
            .expect("fetch");
        assert_eq!(outcome.status, 401);
        assert_eq!(
            String::from_utf8(outcome.body).expect("utf8"),
            r#"{"error":{"message":"bad key"}}"#
        );
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn broker_denial_surfaces_as_plugin_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("openai-deny.sock");
        let server = tokio::spawn(run_mock(
            socket.clone(),
            |_| {},
            vec![BrokerResponse::error(
                ene_plugin_proto::BrokerErrorCode::Denied,
                "denied by policy",
            )],
        ));
        wait_for_socket(&socket).await;
        let broker = Arc::new(OpenAiBroker::new());
        broker.configure(&test_sandbox(&socket));

        let err = broker
            .fetch(
                HttpMethod::Post,
                "https://api.openai.com/v1/embeddings",
                vec![],
                Some("api_key"),
                Some(b"{}".to_vec()),
            )
            .await
            .expect_err("denied");
        assert!(
            format!("{err}").contains("denied by policy"),
            "unexpected error: {err}"
        );
        server.abort();
    }
}
