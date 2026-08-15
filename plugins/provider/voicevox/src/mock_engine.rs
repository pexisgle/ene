//! Minimal VOICEVOX-compatible HTTP engine used as a test fixture.
//!
//! Compiled into the plugin's `#[cfg(test)]` module tree (for in-process
//! external-mode tests and the managed-mode child that re-executes the test
//! harness). It is built without the test lint opt-outs, so it stays
//! production-clean.

use std::io;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Engine's `audio_query` response: a fixed query with camelCase fields.
pub const EXAMPLE_AUDIO_QUERY: &str = r#"{
  "accentPhrases": [{"moras": [{"text": "こ", "consonant": "k", "vowel": "o"}]}],
  "speedScale": 1.2,
  "pitchScale": 0.0,
  "intonationScale": 1.0,
  "volumeScale": 1.0,
  "prePhonemeLength": 0.1,
  "postPhonemeLength": 0.1,
  "outputSamplingRate": 24000,
  "outputStereo": false,
  "kana": "コ"
}"#;

/// A request the fake engine received, for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRequest {
    /// HTTP method.
    pub method: String,
    /// Request target (path + query string).
    pub path: String,
    /// Request body (empty for GET).
    pub body: String,
}

/// Handle to an in-process fake engine.
pub struct MockEngineHandle {
    /// Base URL the engine is serving on.
    pub url: String,
    /// Requests received so far, in order.
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockEngineHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Spawns an in-process fake engine on a random local port.
///
/// # Errors
///
/// Returns an I/O error when the listener cannot be bound or converted.
pub fn spawn_mock_engine() -> io::Result<MockEngineHandle> {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = std_listener.local_addr()?.port();
    // Tokio refuses to register a socket still in blocking mode; the
    // std listener is only used to pick a free port before handing over.
    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let task_requests = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        serve_engine(listener, task_requests).await;
    });
    Ok(MockEngineHandle {
        url: format!("http://127.0.0.1:{port}"),
        requests,
        task,
    })
}

/// Serves HTTP requests until the listener fails or the task is aborted.
///
/// Every request is answered with `Connection: close`, so a client that
/// wants a second request opens a second connection.
pub async fn serve_engine(listener: TcpListener, requests: Arc<Mutex<Vec<RecordedRequest>>>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let requests = Arc::clone(&requests);
        tokio::spawn(async move {
            drop(handle_connection(&mut socket, &requests).await);
        });
    }
}

struct HttpRequest {
    request_line: String,
    body: String,
}

async fn handle_connection(
    socket: &mut TcpStream,
    requests: &Mutex<Vec<RecordedRequest>>,
) -> io::Result<()> {
    let request = read_request(socket).await?;
    let (status, content_type, body) = route(&request, requests);
    socket
        .write_all(&http_response(status, content_type, &body))
        .await?;
    socket.shutdown().await
}

async fn read_request(socket: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut temp = [0u8; 1024];
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
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let read = socket.read(&mut temp).await?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&temp[..read]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..header_end + content_length]).to_string();
    let request_line = headers.lines().next().unwrap_or_default().to_string();
    Ok(HttpRequest { request_line, body })
}

fn route(
    request: &HttpRequest,
    requests: &Mutex<Vec<RecordedRequest>>,
) -> (u16, &'static str, Vec<u8>) {
    let mut parts = request.request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    if let Ok(mut guard) = requests.lock() {
        guard.push(RecordedRequest {
            method: method.clone(),
            path: path.clone(),
            body: request.body.clone(),
        });
    }
    if method == "GET" && path.starts_with("/version") {
        (
            200,
            "application/json",
            b"{\"version\":\"0.15.0\"}".to_vec(),
        )
    } else if method == "GET" && path.starts_with("/speakers") {
        (
            200,
            "application/json",
            r#"[{"name":"四国めたん","speaker_uuid":"7ffcb7ce-00ec-4bdc-82cd-45a8889e43ff","styles":[{"name":"ノーマル","id":2},{"name":"あまあま","id":0}]},{"name":"ずんだもん","speaker_uuid":"388f246b-8c41-4ac1-8e2d-5d79f3fa56fa","styles":[{"name":"ノーマル","id":3}]}]"#
                .to_string()
                .into_bytes(),
        )
    } else if method == "POST" && path.starts_with("/audio_query") {
        (
            200,
            "application/json",
            EXAMPLE_AUDIO_QUERY.as_bytes().to_vec(),
        )
    } else if method == "POST" && path.starts_with("/synthesis") {
        (200, "audio/wav", wav_fixture())
    } else {
        (404, "text/plain", b"not found".to_vec())
    }
}

fn http_response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(body);
    response
}

/// A valid mono s16 24 kHz WAV whose PCM bytes are a constant pattern.
#[expect(
    clippy::expect_used,
    reason = "test fixture builds a fixed in-memory WAV; the writer cannot fail"
)]
fn wav_fixture() -> Vec<u8> {
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::io::Cursor;

    const DATA_LEN: usize = 480;
    let spec = WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::with_capacity(44 + DATA_LEN));
    let mut writer = WavWriter::new(&mut out, spec).expect("valid spec");
    for _ in 0..DATA_LEN / 2 {
        writer.write_sample(0x5A5Au16 as i16).expect("write sample");
    }
    writer.finalize().expect("finalize");
    out.into_inner()
}
