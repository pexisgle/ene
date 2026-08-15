//! # ene-plugin-broker
//!
//! Plugin-side client for the protocol-v8 broker channel. A plugin opens one
//! authenticated session per host service (`file`, `network`, `process`,
//! `credential`, `artifact`, `platform`) on the shared host-service socket
//! and exchanges typed [`BrokerRequest`] / [`BrokerResponse`] frames. The
//! host pins the plugin identity to the channel, so a plugin can never
//! impersonate another one.
//!
//! Everything is mediated: the plugin never opens sockets, user files, or
//! processes directly.

#![warn(missing_docs)]

use std::path::Path;

pub use ene_plugin_proto::ws::{WebSocketRequest, WebSocketResponse};
pub use ene_plugin_proto::{
    BrokerRequest, BrokerResponse, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, IpcStream, read_framed_json, read_host_service_response,
    write_framed_json, write_host_service_request,
};
use tokio::io::AsyncWriteExt;

/// Errors from the broker channel.
#[derive(Debug, thiserror::Error)]
pub enum BrokerClientError {
    /// The socket connection failed.
    #[error("broker connect failed: {0}")]
    Connect(String),
    /// The `Open` handshake was rejected.
    #[error("broker open rejected: {message} ({code:?})")]
    OpenRejected {
        /// Error code from the host.
        code: HostServiceErrorCode,
        /// Human-readable detail.
        message: String,
    },
    /// A broker request failed.
    #[error("broker request failed: {0}")]
    Request(String),
    /// The host returned a structured broker error.
    #[error("broker error {code:?}: {message}")]
    Denied {
        /// Structured code.
        code: ene_plugin_proto::BrokerErrorCode,
        /// Human-readable detail.
        message: String,
    },
    /// A WebSocket handshake failed with an HTTP status.
    #[error("WebSocket handshake failed: {message}")]
    HttpStatus {
        /// HTTP status from the peer (`None` when unknown).
        status: Option<u16>,
        /// Human-readable detail.
        message: String,
    },
    /// I/O failure on the channel.
    #[error("broker I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The peer closed the channel.
    #[error("broker channel closed by host")]
    Closed,
}

/// One authenticated broker session.
pub struct BrokerClient {
    stream: IpcStream,
}

/// A collected streamed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamResponse {
    /// HTTP status.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// All body chunks in order.
    pub chunks: Vec<Vec<u8>>,
}

impl StreamResponse {
    /// Concatenated body bytes.
    #[must_use]
    pub fn body(&self) -> Vec<u8> {
        let len = self.chunks.iter().map(Vec::len).sum();
        let mut body = Vec::with_capacity(len);
        for chunk in &self.chunks {
            body.extend_from_slice(chunk);
        }
        body
    }
}

/// Consumes streamed broker events as they arrive.
///
/// The methods are async so a sink can forward chunks with backpressure;
/// each returned future borrows `&mut self` only for its own execution
/// (RPITIT), so a sink can hold channels and other owned state without
/// lifetime gymnastics.
pub trait StreamSink {
    /// `StreamStart`: response status and headers, delivered before any
    /// body chunk.
    fn start(
        &mut self,
        status: u16,
        headers: Vec<(String, String)>,
    ) -> impl std::future::Future<Output = Result<(), BrokerClientError>> + Send + '_;

    /// One `StreamChunk` body fragment.
    fn chunk(
        &mut self,
        data: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<(), BrokerClientError>> + Send + '_;
}

/// [`StreamSink`] that buffers a whole streamed response.
#[derive(Default)]
struct Collector {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    chunks: Vec<Vec<u8>>,
}

impl StreamSink for Collector {
    async fn start(
        &mut self,
        status: u16,
        headers: Vec<(String, String)>,
    ) -> Result<(), BrokerClientError> {
        self.status = Some(status);
        self.headers = headers;
        Ok(())
    }

    async fn chunk(&mut self, data: Vec<u8>) -> Result<(), BrokerClientError> {
        self.chunks.push(data);
        Ok(())
    }
}

impl std::fmt::Debug for BrokerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerClient").finish_non_exhaustive()
    }
}

impl BrokerClient {
    /// Connects to the host-service socket and opens `service`.
    pub async fn connect(
        socket_path: &Path,
        token: &str,
        service: HostServiceId,
    ) -> Result<Self, BrokerClientError> {
        let mut stream = IpcStream::connect(socket_path)
            .await
            .map_err(|e| BrokerClientError::Connect(e.to_string()))?;
        write_host_service_request(
            &mut stream,
            &HostServiceRequest::Open {
                service,
                token: token.to_string(),
            },
        )
        .await?;
        match read_host_service_response(&mut stream).await? {
            Some(HostServiceResponse::OpenAck) => Ok(Self { stream }),
            Some(HostServiceResponse::Error { code, message }) => {
                Err(BrokerClientError::OpenRejected { code, message })
            }
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Sends one request and returns the matching response.
    pub async fn request(
        &mut self,
        request: &BrokerRequest,
    ) -> Result<BrokerResponse, BrokerClientError> {
        write_framed_json(&mut self.stream, request).await?;
        match read_framed_json(&mut self.stream).await? {
            Some(BrokerResponse::Error { code, message }) => {
                Err(BrokerClientError::Denied { code, message })
            }
            Some(response) => Ok(response),
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Sends a streaming request and collects `StreamStart` / `StreamChunk` /
    /// `StreamEnd` frames until the terminal frame.
    pub async fn collect_stream(
        &mut self,
        request: &BrokerRequest,
    ) -> Result<StreamResponse, BrokerClientError> {
        let mut collector = Collector::default();
        self.stream_events(request, &mut collector).await?;
        Ok(StreamResponse {
            status: collector.status.ok_or_else(|| {
                BrokerClientError::Request("stream ended before StreamStart".to_string())
            })?,
            headers: collector.headers,
            chunks: collector.chunks,
        })
    }

    /// Sends a streaming request and feeds `StreamStart` / `StreamChunk`
    /// frames to `sink` as they arrive.
    ///
    /// The sink's futures are awaited between socket reads, so a slow sink
    /// applies backpressure to the channel. Returning `Err` from a sink
    /// method aborts the exchange.
    pub async fn stream_events<S: StreamSink + ?Sized>(
        &mut self,
        request: &BrokerRequest,
        sink: &mut S,
    ) -> Result<(), BrokerClientError> {
        write_framed_json(&mut self.stream, request).await?;
        loop {
            match read_framed_json(&mut self.stream).await? {
                Some(BrokerResponse::StreamStart { status, headers }) => {
                    sink.start(status, headers).await?;
                }
                Some(BrokerResponse::StreamChunk { data }) => {
                    sink.chunk(data).await?;
                }
                Some(BrokerResponse::StreamEnd) => return Ok(()),
                Some(BrokerResponse::Error { code, message }) => {
                    return Err(BrokerClientError::Denied { code, message });
                }
                Some(other) => {
                    return Err(BrokerClientError::Request(format!(
                        "unexpected streaming frame: {other:?}"
                    )));
                }
                None => return Err(BrokerClientError::Closed),
            }
        }
    }

    /// Shuts the session down cleanly.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.stream.shutdown().await
    }
}

/// One event received on a host-mediated WebSocket session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketEvent {
    /// A text message from the peer.
    Text(String),
    /// A binary message from the peer.
    Binary(Vec<u8>),
    /// The connection closed; the session is finished.
    Closed {
        /// Close code.
        code: u16,
        /// Close reason.
        reason: String,
    },
    /// A session error; the session is finished.
    Error {
        /// HTTP status when the handshake failed on the wire, else `None`.
        status: Option<u16>,
        /// Human-readable detail.
        message: String,
    },
}

/// A host-mediated WebSocket session.
///
/// The host validates SSRF and origin approvals, injects the named
/// credential, and relays frames; the plugin only sends and receives
/// [`WebSocketEvent`]s. Sends are fire-and-forget: [`recv`](Self::recv)
/// yields pushed messages until the session closes.
pub struct WebSocketSession {
    stream: IpcStream,
}

impl WebSocketSession {
    /// Opens a session on the host-service socket and completes the
    /// WebSocket handshake for `url`.
    pub async fn connect(
        socket_path: &Path,
        token: &str,
        url: &str,
        headers: Vec<(String, String)>,
        credential: Option<&str>,
    ) -> Result<(Self, String), BrokerClientError> {
        let mut stream = IpcStream::connect(socket_path)
            .await
            .map_err(|e| BrokerClientError::Connect(e.to_string()))?;
        write_host_service_request(
            &mut stream,
            &HostServiceRequest::Open {
                service: HostServiceId::WebSocket,
                token: token.to_string(),
            },
        )
        .await?;
        match read_host_service_response(&mut stream).await? {
            Some(HostServiceResponse::OpenAck) => {}
            Some(HostServiceResponse::Error { code, message }) => {
                return Err(BrokerClientError::OpenRejected { code, message });
            }
            None => return Err(BrokerClientError::Closed),
        }
        write_framed_json(
            &mut stream,
            &WebSocketRequest::Open {
                url: url.to_string(),
                headers,
                credential: credential.map(str::to_string),
            },
        )
        .await?;
        match read_framed_json(&mut stream).await? {
            Some(WebSocketResponse::OpenOk { final_url }) => Ok((Self { stream }, final_url)),
            Some(WebSocketResponse::Error { status, message }) => {
                Err(BrokerClientError::HttpStatus { status, message })
            }
            Some(other) => Err(BrokerClientError::Request(format!(
                "unexpected frame while opening WebSocket: {other:?}"
            ))),
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Sends a text frame (fire-and-forget).
    pub async fn send_text(&mut self, data: &str) -> Result<(), BrokerClientError> {
        write_framed_json(
            &mut self.stream,
            &WebSocketRequest::SendText {
                data: data.to_string(),
            },
        )
        .await?;
        Ok(())
    }

    /// Sends a binary frame (fire-and-forget).
    pub async fn send_binary(&mut self, data: &[u8]) -> Result<(), BrokerClientError> {
        write_framed_json(
            &mut self.stream,
            &WebSocketRequest::SendBinary {
                data: data.to_vec(),
            },
        )
        .await?;
        Ok(())
    }

    /// Starts the close handshake.
    pub async fn close(&mut self, code: u16, reason: &str) -> Result<(), BrokerClientError> {
        write_framed_json(
            &mut self.stream,
            &WebSocketRequest::Close {
                code,
                reason: reason.to_string(),
            },
        )
        .await?;
        Ok(())
    }

    /// Reads the next pushed frame. `Closed` / `Error` events are terminal:
    /// no further reads succeed.
    pub async fn recv(&mut self) -> Result<WebSocketEvent, BrokerClientError> {
        match read_framed_json(&mut self.stream).await? {
            Some(WebSocketResponse::MessageText { data }) => Ok(WebSocketEvent::Text(data)),
            Some(WebSocketResponse::MessageBinary { data }) => Ok(WebSocketEvent::Binary(data)),
            Some(WebSocketResponse::Closed { code, reason }) => {
                Ok(WebSocketEvent::Closed { code, reason })
            }
            Some(WebSocketResponse::Error { status, message }) => {
                Ok(WebSocketEvent::Error { status, message })
            }
            Some(WebSocketResponse::OpenOk { .. }) => Err(BrokerClientError::Request(
                "unexpected OpenOk after the session opened".to_string(),
            )),
            None => Err(BrokerClientError::Closed),
        }
    }

    /// Shuts the session down cleanly.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.stream.shutdown().await
    }
}

// Re-export the wire types so plugin authors import one crate.
pub use ene_plugin_proto::{
    ArtifactInfo, BrokerErrorCode, ConflictMode, FileEntry, HttpMethod, WireArtifactKind,
};

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit tests use expect for assertions")]
mod tests {
    use super::*;
    use ene_plugin_proto::{HostServiceRequest, HostServiceResponse, write_host_service_response};
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;

    /// Mock host-service server that answers one `NetworkFetchStream` with
    /// the given frames.
    async fn run_stream_mock(socket: std::path::PathBuf, frames: Vec<BrokerResponse>) {
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
        // Consume the streaming request before answering, mirroring the
        // host session loop (request/response framing per exchange).
        let request: BrokerRequest = read_framed_json(&mut stream)
            .await
            .expect("request")
            .expect("frame");
        assert!(matches!(request, BrokerRequest::NetworkFetchStream { .. }));
        for frame in frames {
            write_framed_json(&mut stream, &frame).await.expect("frame");
        }
        drop(stream.shutdown().await);
    }

    #[tokio::test]
    async fn collect_stream_reassembles_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("stream.sock");
        let server = tokio::spawn(run_stream_mock(
            socket.clone(),
            vec![
                BrokerResponse::StreamStart {
                    status: 200,
                    headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
                },
                BrokerResponse::StreamChunk {
                    data: b"data: hello\n\n".to_vec(),
                },
                BrokerResponse::StreamChunk {
                    data: b"data: world\n\n".to_vec(),
                },
                BrokerResponse::StreamEnd,
            ],
        ));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let mut client = BrokerClient::connect(&socket, "tok", HostServiceId::Network)
            .await
            .expect("connect");
        let response = client
            .collect_stream(&BrokerRequest::NetworkFetchStream {
                method: HttpMethod::Get,
                url: "https://example.com/stream".to_string(),
                headers: vec![],
                credential: None,
                credential_header: None,
                body: None,
                max_bytes: Some(1024),
            })
            .await
            .expect("stream");
        assert_eq!(response.status, 200);
        assert_eq!(response.chunks.len(), 2);
        assert_eq!(response.body(), b"data: hello\n\ndata: world\n\n");
        server.abort();
    }

    #[tokio::test]
    async fn collect_stream_surfaces_host_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("stream-error.sock");
        let server = tokio::spawn(run_stream_mock(
            socket.clone(),
            vec![BrokerResponse::error(
                BrokerErrorCode::Denied,
                "denied by policy",
            )],
        ));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let mut client = BrokerClient::connect(&socket, "tok", HostServiceId::Network)
            .await
            .expect("connect");
        let err = client
            .collect_stream(&BrokerRequest::NetworkFetchStream {
                method: HttpMethod::Get,
                url: "https://example.com/stream".to_string(),
                headers: vec![],
                credential: None,
                credential_header: None,
                body: None,
                max_bytes: None,
            })
            .await
            .expect_err("denied");
        assert!(matches!(err, BrokerClientError::Denied { .. }));
        server.abort();
    }
}
