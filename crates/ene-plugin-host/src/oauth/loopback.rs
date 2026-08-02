//! Loopback redirect server for the OAuth authorization flow (RFC 8252 §8.3).
//!
//! The flow binds an ephemeral `127.0.0.1` port and serves exactly the
//! authorization callback: one `GET /callback` whose `state` matches the
//! flow's. A hand-rolled HTTP/1.1 reader (bounded at 16 KiB, `httparse`
//! parse) replaces a full server crate because the surface is one request;
//! nothing outside the loopback interface is ever accepted.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::FlowError;

/// Hard cap on the request a callback may carry; anything larger is
/// rejected outright (a real authorization server never exceeds this).
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// How long a connected socket may sit without finishing its request before
/// it is dropped and the flow keeps listening.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// One parsed authorization callback.
struct ParsedCallback {
    /// Authorization code, present on success.
    code: Option<String>,
    /// OAuth `state` echoed by the authorization server.
    state: String,
    /// `error` parameter, present when the server refused consent.
    error: Option<String>,
}

/// Loopback authorization-callback listener.
pub(crate) struct LoopbackServer {
    listener: TcpListener,
}

impl LoopbackServer {
    /// Binds an ephemeral port on `127.0.0.1` only.
    ///
    /// Binding `0.0.0.0` would let any host on the network deliver a
    /// callback and steal an authorization code; the loopback exception in
    /// RFC 8252 §8.3 is exactly what the flow needs.
    pub(crate) async fn bind() -> Result<Self, FlowError> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        Ok(Self { listener })
    }

    /// The bound loopback address (for the `redirect_uri`).
    pub(crate) fn local_addr(&self) -> Result<SocketAddr, FlowError> {
        Ok(self.listener.local_addr()?)
    }

    /// Waits for a callback whose `state` matches `expected_state`.
    ///
    /// On a matching callback the authorization code is returned (alongside
    /// the echoed state) and the server stops listening. A mismatched state
    /// — a CSRF probe or a stale flow — fails the flow with a 400 response:
    /// an attacker must not be able to keep the flow alive while guessing
    /// states. Malformed or non-callback requests get a 4xx and are ignored
    /// until the timeout.
    pub(crate) async fn wait_for_callback(
        &self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<(String, String), FlowError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(FlowError::Timeout);
            }
            let (mut socket, _) =
                match tokio::time::timeout(remaining, self.listener.accept()).await {
                    Ok(Ok(pair)) => pair,
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => return Err(FlowError::Timeout),
                };

            let request =
                match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request(&mut socket)).await {
                    Ok(Ok(request)) => request,
                    // A socket that stalls or sends garbage is dropped and
                    // the flow keeps listening for the real callback.
                    Ok(Err(())) | Err(_) => {
                        respond(&mut socket, 400).await;
                        continue;
                    }
                };

            let Some(callback) = parse_callback(&request) else {
                respond(&mut socket, 404).await;
                continue;
            };
            if callback.state != expected_state {
                respond(&mut socket, 400).await;
                return Err(FlowError::Callback(
                    "authorization callback state did not match the flow".to_string(),
                ));
            }
            if let Some(error) = callback.error {
                respond(&mut socket, 400).await;
                return Err(FlowError::Callback(format!(
                    "authorization server reported: {error}"
                )));
            }
            let Some(code) = callback.code else {
                respond(&mut socket, 400).await;
                return Err(FlowError::Callback(
                    "authorization callback carried no code".to_string(),
                ));
            };
            respond(&mut socket, 200).await;
            return Ok((code, callback.state));
        }
    }
}

/// Reads one HTTP/1.1 request head (up to [`MAX_REQUEST_BYTES`]) from the
/// socket. Bodyless `GET` requests have no body to drain.
async fn read_request(socket: &mut TcpStream) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];
    loop {
        let n = socket.read(&mut chunk).await.map_err(|_| ())?;
        if n == 0 {
            return if buf.is_empty() { Err(()) } else { Ok(buf) };
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_REQUEST_BYTES {
            return Err(());
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
    }
}

/// Extracts the `GET /callback` query parameters from a request head.
fn parse_callback(request: &[u8]) -> Option<ParsedCallback> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut headers);
    let httparse::Status::Complete(_) = parsed.parse(request).ok()? else {
        return None;
    };
    if parsed.method != Some("GET") {
        return None;
    }
    let path = parsed.path?;
    let (path_only, query) = path.split_once('?').unwrap_or((path, ""));
    if path_only != "/callback" {
        return None;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    Some(ParsedCallback {
        code,
        state: state?,
        error,
    })
}

/// Writes a minimal fixed-body HTTP/1.1 response.
async fn respond(socket: &mut TcpStream, status: u16) {
    let (reason, body, content_type) = match status {
        200 => (
            "OK",
            "<html><body><h1>Authorization complete</h1><p>You can close this window now.</p></body></html>",
            "text/html",
        ),
        400 => (
            "Bad Request",
            "<html><body><h1>Authorization failed</h1><p>The authorization request was invalid.</p></body></html>",
            "text/html",
        ),
        _ => (
            "Not Found",
            "<html><body><h1>Not Found</h1></body></html>",
            "text/html",
        ),
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    drop(socket.write_all(head.as_bytes()).await);
    drop(socket.write_all(body.as_bytes()).await);
    drop(socket.flush().await);
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn parses_callback_query() {
        let request = b"GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let callback = parse_callback(request).unwrap();
        assert_eq!(callback.code.as_deref(), Some("abc123"));
        assert_eq!(callback.state, "xyz");
        assert!(callback.error.is_none());
    }

    #[test]
    fn parses_errored_callback() {
        let request =
            b"GET /callback?error=access_denied&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let callback = parse_callback(request).unwrap();
        assert!(callback.code.is_none());
        assert_eq!(callback.error.as_deref(), Some("access_denied"));
        assert_eq!(callback.state, "xyz");
    }

    #[test]
    fn rejects_non_callback_paths_and_methods() {
        let wrong_path = b"GET /other?code=x&state=y HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert!(parse_callback(wrong_path).is_none());
        let post = b"POST /callback HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert!(parse_callback(post).is_none());
    }

    #[test]
    fn rejects_callback_without_state() {
        let request = b"GET /callback?code=abc123 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert!(parse_callback(request).is_none());
    }
}
