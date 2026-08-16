//! Minimal in-process broker mock of the `OpenAI Speech API`, used as a
//! test fixture. It speaks the protocol-v8 broker channel (handshake +
//! `NetworkFetch` frames) instead of raw HTTP, so tests exercise the same
//! mediation path the host provides. Compiled into the plugin's `#[cfg
//! (test)]` module tree; written without the test lint opt-outs so it stays
//! production-clean.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test fixture uses expect/panic for concise assertions"
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ene_plugin_broker::{BrokerRequest, BrokerResponse, read_framed_json, write_framed_json};
use ene_plugin_proto::{
    HostServiceId, HostServiceRequest, HostServiceResponse, read_host_service_request,
    write_host_service_response,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRequest {
    pub method: String,
    /// Absolute request URL.
    pub url: String,
    /// Header name/value pairs, names as sent.
    pub headers: Vec<(String, String)>,
    /// Host-owned credential the plugin named (`None` when absent).
    pub credential: Option<String>,
    /// Header the credential should be injected into (`None` = default).
    pub credential_header: Option<String>,
    /// Request body (empty when the request had no body).
    pub body: String,
}

/// A scripted response, consumed FIFO by the server.
#[derive(Debug, Clone)]
pub struct MockResponse {
    status: u16,
    body: Vec<u8>,
    /// Extra response headers (e.g. `Retry-After`).
    headers: Vec<(String, String)>,
}

impl MockResponse {
    #[must_use]
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            body,
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_status(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

pub struct MockSpeechServer {
    socket: std::path::PathBuf,
    /// Recorded requests, in arrival order.
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
    _dir: tempfile::TempDir,
}

impl MockSpeechServer {
    pub fn spawn() -> std::io::Result<Self> {
        let dir = tempfile::tempdir().map_err(std::io::Error::other)?;
        let socket = dir.path().join("mock-broker.sock");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responses = Arc::new(Mutex::new(VecDeque::new()));
        // Bind synchronously so the first request never races the accept
        // loop's startup; a lost race would fail the fetch with ENOENT and
        // burn one retry backoff in every test.
        let std_listener =
            std::os::unix::net::UnixListener::bind(&socket).map_err(std::io::Error::other)?;
        std_listener
            .set_nonblocking(true)
            .map_err(std::io::Error::other)?;
        let listener =
            tokio::net::UnixListener::from_std(std_listener).map_err(std::io::Error::other)?;
        let server = Self {
            socket,
            requests: Arc::clone(&requests),
            responses: Arc::clone(&responses),
            _dir: dir,
        };
        tokio::spawn(run_server(
            listener,
            Arc::clone(&requests),
            Arc::clone(&responses),
        ));
        Ok(server)
    }

    /// The fake base URL the plugin targets (never actually reached; the
    /// broker mock intercepts the request).
    #[must_use]
    pub fn url() -> &'static str {
        "https://api.openai.com/v1"
    }

    #[must_use]
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket
    }

    pub fn push(&self, response: MockResponse) {
        self.responses
            .lock()
            .expect("response queue")
            .push_back(response);
    }
}

async fn run_server(
    listener: tokio::net::UnixListener,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
) {
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
        let Ok(Some(request)) = read_framed_json(&mut stream).await else {
            return;
        };
        let BrokerRequest::NetworkFetch {
            method,
            url,
            headers,
            credential,
            credential_header,
            body,
            ..
        } = request
        else {
            panic!("expected NetworkFetch, got {request:?}");
        };
        requests.lock().expect("request log").push(RecordedRequest {
            method: format!("{method:?}").to_ascii_uppercase(),
            url,
            headers,
            credential,
            credential_header,
            body: body.map_or_else(String::new, |body| {
                String::from_utf8_lossy(&body).into_owned()
            }),
        });
        let response = responses
            .lock()
            .expect("response queue")
            .pop_front()
            .unwrap_or_else(|| panic!("mock response queue exhausted"));
        write_framed_json(
            &mut stream,
            &BrokerResponse::NetworkFetchOk {
                status: response.status,
                headers: response.headers,
                body: response.body,
            },
        )
        .await
        .expect("mock response");
    }
}
