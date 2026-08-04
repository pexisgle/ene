//! Minimal in-process HTTP mock of the `OpenAI Speech API`, used as a test
//! fixture. Compiled into the plugin's `#[cfg(test)]` module tree; it is
//! written without the test lint opt-outs so it stays production-clean.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request the fake API received, for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRequest {
    /// HTTP method.
    pub method: String,
    /// Request target (path + query string).
    pub path: String,
    /// Header name/value pairs, names lowercased.
    pub headers: Vec<(String, String)>,
    /// Request body (empty when the request had no Content-Length).
    pub body: String,
}

/// A scripted HTTP response, consumed FIFO by the server.
#[derive(Debug, Clone)]
pub struct MockResponse {
    status: u16,
    body: Vec<u8>,
    /// Extra response headers (e.g. `Retry-After`).
    headers: Vec<(String, String)>,
    /// Content-Length header override; lets tests declare an oversized
    /// payload without materializing it.
    declared_content_length: Option<usize>,
    chunked: bool,
    chunk_delay: Duration,
    chunk_size: usize,
}

impl MockResponse {
    /// A `200 OK` response with a fixed-length body.
    #[must_use]
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            body,
            headers: Vec::new(),
            declared_content_length: None,
            chunked: false,
            chunk_delay: Duration::ZERO,
            chunk_size: 0,
        }
    }

    /// A response with a custom status code.
    #[must_use]
    pub fn with_status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: Vec::new(),
            declared_content_length: None,
            chunked: false,
            chunk_delay: Duration::ZERO,
            chunk_size: 0,
        }
    }

    /// A `200 OK` chunked response, written in `chunk_size` pieces with
    /// `chunk_delay` between them, so the client's streaming path is
    /// exercised.
    #[must_use]
    pub fn streamed(body: Vec<u8>, chunk_size: usize, chunk_delay: Duration) -> Self {
        Self {
            status: 200,
            body,
            headers: Vec::new(),
            declared_content_length: None,
            chunked: true,
            chunk_delay,
            chunk_size: chunk_size.max(1),
        }
    }

    /// Declares a `Content-Length` that differs from the actual body length
    /// (used to test the client's upfront size check).
    #[must_use]
    pub fn with_declared_length(mut self, length: usize) -> Self {
        self.declared_content_length = Some(length);
        self
    }

    /// Adds a response header (e.g. `Retry-After`).
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// Handle to an in-process fake Speech API.
pub struct MockSpeechServer {
    /// Base URL the server is listening on (no path prefix).
    pub url: String,
    /// Requests received so far, in order.
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockSpeechServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockSpeechServer {
    /// Spawns the fake API on a random local port.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener cannot be bound or converted.
    pub fn spawn() -> io::Result<Self> {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = std_listener.local_addr()?.port();
        // Tokio refuses to register a socket still in blocking mode; the
        // std listener is only used to pick a free port before handing over.
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responses = Arc::new(Mutex::new(VecDeque::new()));
        let task_requests = Arc::clone(&requests);
        let task_responses = Arc::clone(&responses);
        let task = tokio::spawn(async move {
            serve(listener, task_requests, task_responses).await;
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}"),
            requests,
            responses,
            task,
        })
    }

    /// Queues a scripted response; responses are consumed FIFO and the
    /// server answers `404` once the queue is empty.
    pub fn push(&self, response: MockResponse) {
        self.responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(response);
    }
}

/// Serves HTTP requests until the listener fails or the task is aborted.
async fn serve(
    listener: TcpListener,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let requests = Arc::clone(&requests);
        let responses = Arc::clone(&responses);
        tokio::spawn(async move {
            drop(handle_connection(&mut socket, &requests, &responses).await);
        });
    }
}

struct HttpRequest {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

async fn handle_connection(
    socket: &mut TcpStream,
    requests: &Mutex<Vec<RecordedRequest>>,
    responses: &Mutex<VecDeque<MockResponse>>,
) -> io::Result<()> {
    let request = read_request(socket).await?;
    if let Ok(mut guard) = requests.lock() {
        guard.push(record(&request));
    }
    let response = responses
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop_front()
        .unwrap_or_else(|| MockResponse::with_status(404, b"no scripted response".to_vec()));
    write_response(socket, &response).await?;
    socket.shutdown().await
}

fn record(request: &HttpRequest) -> RecordedRequest {
    let mut parts = request.request_line.split_whitespace();
    RecordedRequest {
        method: parts.next().unwrap_or_default().to_string(),
        path: parts.next().unwrap_or_default().to_string(),
        headers: request.headers.clone(),
        body: request.body.clone(),
    }
}

async fn read_request(socket: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut temp = [0u8; 4096];
    let header_end;
    loop {
        let read = socket.read(&mut temp).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before headers",
            ));
        }
        buf.extend_from_slice(&temp[..read]);
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = end + 4;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let read = socket.read(&mut temp).await?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&temp[..read]);
    }
    let body_end = header_end.saturating_add(content_length).min(buf.len());
    let body = String::from_utf8_lossy(&buf[header_end..body_end]).to_string();
    Ok(HttpRequest {
        request_line,
        headers,
        body,
    })
}

async fn write_response(socket: &mut TcpStream, response: &MockResponse) -> io::Result<()> {
    if response.chunked {
        write_chunked(socket, response).await
    } else {
        let content_length = response
            .declared_content_length
            .unwrap_or(response.body.len());
        let mut extra_headers = String::new();
        for (name, value) in &response.headers {
            extra_headers.push_str(name);
            extra_headers.push_str(": ");
            extra_headers.push_str(value);
            extra_headers.push_str("\r\n");
        }
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/octet-stream\r\n\
             {extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            reason_phrase(response.status),
            content_length
        );
        socket.write_all(head.as_bytes()).await?;
        socket.write_all(&response.body).await?;
        socket.flush().await
    }
}

/// Writes the body as HTTP chunked transfer encoding with a delay between
/// chunks, so the client observes incremental arrival.
async fn write_chunked(socket: &mut TcpStream, response: &MockResponse) -> io::Result<()> {
    let mut extra_headers = String::new();
    for (name, value) in &response.headers {
        extra_headers.push_str(name);
        extra_headers.push_str(": ");
        extra_headers.push_str(value);
        extra_headers.push_str("\r\n");
    }
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/octet-stream\r\n\
         {extra_headers}Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        response.status,
        reason_phrase(response.status)
    );
    socket.write_all(head.as_bytes()).await?;
    for chunk in response.body.chunks(response.chunk_size) {
        socket
            .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
            .await?;
        socket.write_all(chunk).await?;
        socket.write_all(b"\r\n").await?;
        socket.flush().await?;
        if !response.chunk_delay.is_zero() {
            tokio::time::sleep(response.chunk_delay).await;
        }
    }
    socket.write_all(b"0\r\n\r\n").await?;
    socket.flush().await
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
