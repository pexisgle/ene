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
        let mut chunks = Vec::new();
        let (status, headers) = self
            .stream_chunks(request, |chunk| {
                chunks.push(chunk.to_vec());
                Ok(())
            })
            .await?;
        Ok(StreamResponse {
            status,
            headers,
            chunks,
        })
    }

    /// Sends a streaming request and invokes `on_chunk` for every body
    /// chunk as it arrives; returns the response status and headers.
    pub async fn stream_chunks(
        &mut self,
        request: &BrokerRequest,
        on_chunk: impl FnMut(&[u8]) -> Result<(), BrokerClientError>,
    ) -> Result<(u16, Vec<(String, String)>), BrokerClientError> {
        write_framed_json(&mut self.stream, request).await?;
        let mut status = None;
        let mut headers = Vec::new();
        let mut on_chunk = on_chunk;
        loop {
            match read_framed_json(&mut self.stream).await? {
                Some(BrokerResponse::StreamStart {
                    status: frame_status,
                    headers: frame_headers,
                }) => {
                    status = Some(frame_status);
                    headers = frame_headers;
                }
                Some(BrokerResponse::StreamChunk { data }) => on_chunk(&data)?,
                Some(BrokerResponse::StreamEnd) => break,
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
        Ok((
            status.ok_or_else(|| {
                BrokerClientError::Request("stream ended before StreamStart".to_string())
            })?,
            headers,
        ))
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
    async fn stream_chunks_invokes_callback_per_chunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("stream-callback.sock");
        let server = tokio::spawn(run_stream_mock(
            socket.clone(),
            vec![
                BrokerResponse::StreamStart {
                    status: 206,
                    headers: vec![("content-range".to_string(), "bytes 0-3/8".to_string())],
                },
                BrokerResponse::StreamChunk {
                    data: b"part1".to_vec(),
                },
                BrokerResponse::StreamChunk {
                    data: b"part2".to_vec(),
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
        let mut seen = Vec::new();
        let (status, headers) = client
            .stream_chunks(
                &BrokerRequest::NetworkFetchStream {
                    method: HttpMethod::Get,
                    url: "https://example.com/stream".to_string(),
                    headers: vec![],
                    credential: None,
                    body: None,
                    max_bytes: Some(1024),
                },
                |chunk| {
                    seen.push(chunk.to_vec());
                    Ok(())
                },
            )
            .await
            .expect("stream");
        assert_eq!(status, 206);
        assert_eq!(
            headers,
            vec![("content-range".to_string(), "bytes 0-3/8".to_string())]
        );
        assert_eq!(seen, vec![b"part1".to_vec(), b"part2".to_vec()]);
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
                body: None,
                max_bytes: None,
            })
            .await
            .expect_err("denied");
        assert!(matches!(err, BrokerClientError::Denied { .. }));
        server.abort();
    }
}
