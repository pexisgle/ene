//! Host-mediated WebSocket session for the edge-tts plugin.
//!
//! Every connection goes through the host's `WebSocket` passenger: the host
//! validates SSRF and origin approvals, pins the resolved address, and
//! relays frames. The plugin never opens a socket or speaks TLS itself.

use std::sync::Arc;

use ene_plugin_broker::WebSocketSession;
use ene_plugin_proto::SandboxConfigData;
use parking_lot::RwLock;

/// Lazily-connected host-service handle shared by every synthesis.
pub struct EdgeBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
}

impl EdgeBroker {
    /// A broker with no connection configuration yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
        }
    }

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, sandbox: &SandboxConfigData) {
        self.socket.write().clone_from(&sandbox.broker_socket);
        self.token.write().clone_from(&sandbox.db_auth_token);
    }

    pub async fn open(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Result<WebSocketSession, ene_plugin_broker::BrokerClientError> {
        let socket = self.socket.read().clone();
        let token = self.token.read().clone();
        let (Some(socket), Some(token)) = (socket, token) else {
            return Err(ene_plugin_broker::BrokerClientError::Request(
                "broker channel is not configured (missing broker socket/token from the host)"
                    .to_string(),
            ));
        };
        let (session, _final_url) =
            WebSocketSession::connect(std::path::Path::new(&socket), &token, url, headers, None)
                .await?;
        Ok(session)
    }
}

impl Default for EdgeBroker {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<EdgeBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<EdgeBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(EdgeBroker::new())))
}

pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test fixture uses expect/panic for concise assertions"
)]
pub(crate) mod tests {
    use super::*;
    use ene_plugin_proto::ws::{WebSocketRequest, WebSocketResponse};
    use ene_plugin_proto::{
        HostServiceId, HostServiceRequest, HostServiceResponse, read_framed_json,
        read_host_service_request, write_framed_json, write_host_service_response,
    };
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    /// Serializes tests that reconfigure the process-wide shared broker.
    pub static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A mock `WebSocket` passenger that bridges the plugin session to a
    /// real WebSocket server (the tests' local fake Edge endpoint).
    pub struct MockWsBroker {
        socket: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    impl MockWsBroker {
        /// Spawns the mock on a fresh unix socket.
        #[must_use]
        pub fn spawn() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let socket = dir.path().join("edge-ws-mock.sock");
            let server = Self { socket, _dir: dir };
            tokio::spawn(run_server(server.socket.clone()));
            server
        }
    }

    /// Acquires the shared broker, points it at a fresh mock, and returns
    /// the serialization guard. The mock is intentionally leaked so its
    /// socket path outlives the lazily established session.
    pub async fn with_broker() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mock = MockWsBroker::spawn();
        for _ in 0..200 {
            if mock.socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        broker().configure(&SandboxConfigData {
            broker_socket: Some(mock.socket.to_string_lossy().into_owned()),
            db_auth_token: Some("tok".to_string()),
            ..SandboxConfigData::default()
        });
        let _ = Box::leak(Box::new(mock));
        guard
    }

    async fn run_server(socket: std::path::PathBuf) {
        let listener = tokio::net::UnixListener::bind(&socket).expect("mock bind");
        loop {
            let (stream, _) = listener.accept().await.expect("mock accept");
            serve_session(stream).await;
        }
    }

    async fn serve_session(mut stream: tokio::net::UnixStream) {
        let open: HostServiceRequest = read_host_service_request(&mut stream)
            .await
            .expect("mock open")
            .expect("open frame");
        assert!(matches!(
            open,
            HostServiceRequest::Open {
                service: HostServiceId::WebSocket,
                ..
            }
        ));
        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
            .await
            .expect("mock ack");

        let open_frame: WebSocketRequest = read_framed_json(&mut stream)
            .await
            .expect("open frame")
            .expect("open request");
        let WebSocketRequest::Open { url, headers, .. } = open_frame else {
            panic!("expected Open");
        };
        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                url.clone(),
            )
            .expect("client request");
        {
            let request_headers = request.headers_mut();
            for (key, value) in headers {
                if let (Ok(key), Ok(value)) = (
                    http::header::HeaderName::try_from(key),
                    http::header::HeaderValue::try_from(value),
                ) {
                    request_headers.insert(key, value);
                }
            }
        }
        let host = request.uri().host().expect("host").to_string();
        let port = request.uri().port_u16().unwrap_or(80);
        let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
            .await
            .expect("tcp connect");
        let ws = match tokio_tungstenite::client_async(request, tcp).await {
            Ok((ws, _)) => ws,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                let status = response.status().as_u16();
                let mut message = format!("WebSocket handshake failed: HTTP {status}");
                if let Some(date) = response
                    .headers()
                    .get(http::header::DATE)
                    .and_then(|value| value.to_str().ok())
                {
                    message.push_str(" Date: ");
                    message.push_str(date);
                }
                drop(
                    write_framed_json(
                        &mut stream,
                        &WebSocketResponse::Error {
                            status: Some(status),
                            message,
                        },
                    )
                    .await,
                );
                return;
            }
            Err(e) => {
                drop(
                    write_framed_json(
                        &mut stream,
                        &WebSocketResponse::Error {
                            status: None,
                            message: e.to_string(),
                        },
                    )
                    .await,
                );
                return;
            }
        };
        write_framed_json(&mut stream, &WebSocketResponse::OpenOk { final_url: url })
            .await
            .expect("mock open ok");

        let (mut ipc_read, mut ipc_write) = tokio::io::split(stream);
        let (mut ws_sink, mut ws_stream) = ws.split();
        let to_ws = tokio::spawn(async move {
            loop {
                let Ok(Some(request)) =
                    read_framed_json::<_, WebSocketRequest>(&mut ipc_read).await
                else {
                    return;
                };
                let message = match request {
                    WebSocketRequest::SendText { data } => Message::Text(data.into()),
                    WebSocketRequest::SendBinary { data } => Message::Binary(data.into()),
                    WebSocketRequest::Close { code, reason } => {
                        drop(ws_sink
                            .send(Message::Close(Some(
                                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code),
                                    reason: reason.into(),
                                },
                            )))
                            .await);
                        return;
                    }
                    WebSocketRequest::Open { .. } => return,
                };
                if ws_sink.send(message).await.is_err() {
                    return;
                }
            }
        });
        while let Some(message) = ws_stream.next().await {
            let Ok(message) = message else {
                drop(
                    write_framed_json(
                        &mut ipc_write,
                        &WebSocketResponse::Closed {
                            code: 1006,
                            reason: "connection reset".to_string(),
                        },
                    )
                    .await,
                );
                break;
            };
            let response = match message {
                Message::Text(text) => WebSocketResponse::MessageText {
                    data: text.to_string(),
                },
                Message::Binary(binary) => WebSocketResponse::MessageBinary {
                    data: binary.to_vec(),
                },
                Message::Close(frame) => {
                    let (code, reason) = frame.map_or((1006, "closed".to_string()), |f| {
                        (f.code.into(), f.reason.to_string())
                    });
                    drop(
                        write_framed_json(
                            &mut ipc_write,
                            &WebSocketResponse::Closed { code, reason },
                        )
                        .await,
                    );
                    break;
                }
                _ => continue,
            };
            if write_framed_json(&mut ipc_write, &response).await.is_err() {
                break;
            }
        }
        to_ws.abort();
    }
}
