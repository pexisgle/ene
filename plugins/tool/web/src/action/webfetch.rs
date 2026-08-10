use std::sync::Arc;

use ene_plugin::prelude::*;

use crate::broker::WebBroker;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;

fn is_binary_content_type(mime: &str) -> bool {
    let lower = mime.to_ascii_lowercase();
    lower.starts_with("image/")
        || lower.starts_with("audio/")
        || lower.starts_with("video/")
        || lower.starts_with("font/")
        || lower == "application/pdf"
        || lower == "application/octet-stream"
        || lower == "application/zip"
        || lower == "application/gzip"
        || lower == "application/x-tar"
        || lower == "application/x-gzip"
        || lower.starts_with("application/x-")
}

/// Strips HTML tags and returns plain text content.
///
/// Detaches script/style/noscript/template subtrees before extracting
/// text nodes, then collapses whitespace into a single readable line.
fn html_to_plain_text(html: &str) -> String {
    use scraper::{Html, Node, Selector};

    const SKIP_TAGS: &[&str] = &["script", "style", "noscript", "template"];

    let mut document = Html::parse_document(html);

    // Detach non-content subtrees so their text is not extracted.
    let root_id = document.root_element().id();
    let skip_ids: Vec<_> = document
        .tree
        .get(root_id)
        .map(|root| {
            root.descendants()
                .filter_map(|node| match node.value() {
                    Node::Element(el) if SKIP_TAGS.contains(&el.name()) => Some(node.id()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    for id in skip_ids {
        if let Some(mut node) = document.tree.get_mut(id) {
            node.detach();
        }
    }

    // Prefer <body> content; fall back to the full document.
    let text: String = if let Ok(sel) = Selector::parse("body")
        && let Some(body) = document.select(&sel).next()
    {
        body.text().collect::<Vec<_>>().join(" ")
    } else {
        document.root_element().text().collect::<Vec<_>>().join(" ")
    };

    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "web",
    name = "fetch",
    summary = "Fetch a URL and return its content as text or markdown.",
    description = "Fetches content from a URL and returns it in the requested format (markdown, text, or html). Supports configurable timeout and automatically converts HTML to readable markdown.",
    category = "WebFetch",
    keywords_primary = "fetch, url, web, download, html",
    side_effects = "Network { external: true }"
)]
pub struct WebFetchAction {
    #[tool(skip)]
    #[serde(skip)]
    broker: Arc<WebBroker>,
    /// The URL to fetch content from (must start with http:// or https://).
    url: String,
    /// The format to return: text, markdown, or html. Defaults to markdown.
    #[arg(enum_values = "text, markdown, html", default = "markdown")]
    #[serde(default)]
    format: Option<String>,
    /// Optional timeout in seconds (max 120).
    ///
    /// Kept for schema compatibility; the host enforces its own timeouts on
    /// broker-mediated requests.
    #[arg(minimum = 1, maximum = 120, default = "30")]
    #[serde(default)]
    timeout: Option<u64>,
}

impl WebFetchAction {
    pub const fn new(broker: Arc<WebBroker>) -> Self {
        Self {
            broker,
            url: String::new(),
            format: None,
            timeout: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let url = &self.url;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidArguments {
                message: "URL must start with http:// or https://".to_string(),
            });
        }

        // URL shape check only: the host validates SSRF, origins, and every
        // redirect hop through the Network broker.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidArguments {
                message: "URL must start with http:// or https://".to_string(),
            });
        }
        if let Some(timeout) = self.timeout
            && !(1..=120).contains(&timeout)
        {
            return Err(ToolError::InvalidArguments {
                message: "timeout must be between 1 and 120 seconds".to_string(),
            });
        }

        let format = self.format.as_deref().unwrap_or("markdown");

        let accept_header = match format {
            "text" => "text/plain;q=1.0, text/html;q=0.8, */*;q=0.1",
            "html" => "text/html;q=1.0, text/plain;q=0.8, */*;q=0.1",
            _ => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        };

        // The host mediates the request: SSRF checks, origin approval, and
        // redirect re-validation all happen there.
        let response = self
            .broker
            .fetch(
                ene_plugin_broker::HttpMethod::Get,
                url,
                vec![
                    ("Accept".to_string(), accept_header.to_string()),
                    ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
                ],
                None,
                MAX_RESPONSE_SIZE as u64,
                None,
                None,
            )
            .await?;

        if !(200..300).contains(&response.status) {
            return Err(ToolError::execution_failed(format!(
                "HTTP request returned status: {} {}",
                response.status,
                status_reason(response.status)
            )));
        }

        // Reject known binary content types before attempting
        // UTF-8 conversion, which would produce garbage.
        if let Some(content_type) = response.content_type() {
            let mime = content_type.split(';').next().unwrap_or_default().trim();
            if is_binary_content_type(mime) {
                return Err(ToolError::execution_failed(format!(
                    "Cannot display binary content (Content-Type: {mime})"
                )));
            }
        }

        let body = String::from_utf8_lossy(&response.body);

        match format {
            "html" => Ok(body.to_string()),
            "text" => Ok(html_to_plain_text(&body)),
            _ => Ok(ene_util::html::html_to_markdown(&body)),
        }
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
mod tests {
    use super::*;
    use ene_plugin_broker::{BrokerRequest, BrokerResponse, HttpMethod};
    use ene_plugin_proto::{
        BrokerErrorCode, HostServiceId, HostServiceRequest, HostServiceResponse, read_framed_json,
        write_framed_json, write_host_service_response,
    };
    use tokio::net::UnixListener;

    struct RecordedRequest {
        method: HttpMethod,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    }

    /// Minimal host-service mock: authenticates the `Network` open, then
    /// answers every fetch with a canned response (or the provided status).
    async fn run_mock_broker(
        socket: std::path::PathBuf,
        mock_body: &'static [u8],
        content_type: &'static str,
        status: u16,
        recorded: std::sync::Arc<parking_lot::Mutex<Vec<RecordedRequest>>>,
    ) {
        let listener = UnixListener::bind(&socket).unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let open: HostServiceRequest = read_framed_json(&mut stream).await.unwrap().unwrap();
        assert!(matches!(
            open,
            HostServiceRequest::Open {
                service: HostServiceId::Network,
                ..
            }
        ));
        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
            .await
            .unwrap();
        loop {
            let Some(request) = read_framed_json::<_, BrokerRequest>(&mut stream)
                .await
                .unwrap()
            else {
                return;
            };
            match request {
                BrokerRequest::NetworkFetch {
                    method,
                    url,
                    headers,
                    body,
                    ..
                } => {
                    recorded.lock().push(RecordedRequest {
                        method,
                        url,
                        headers,
                        body,
                    });
                    let response = BrokerResponse::NetworkFetchOk {
                        status,
                        headers: vec![("content-type".to_string(), content_type.to_string())],
                        body: mock_body.to_vec(),
                    };
                    write_framed_json(&mut stream, &response).await.unwrap();
                }
                other => {
                    write_framed_json(
                        &mut stream,
                        &BrokerResponse::error(
                            BrokerErrorCode::Internal,
                            format!("unexpected request: {other:?}"),
                        ),
                    )
                    .await
                    .unwrap();
                }
            }
        }
    }

    fn broker_with_mock(
        socket: &std::path::Path,
    ) -> (
        std::sync::Arc<crate::broker::WebBroker>,
        std::sync::Arc<parking_lot::Mutex<Vec<RecordedRequest>>>,
    ) {
        let broker = crate::broker::WebBroker::new();
        let sandbox = ene_plugin_proto::SandboxConfigData {
            broker_socket: Some(socket.to_string_lossy().into_owned()),
            db_auth_token: Some("test-token".to_string()),
            ..Default::default()
        };
        broker.configure(&sandbox);
        (
            broker,
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new())),
        )
    }

    #[tokio::test]
    async fn fetch_round_trips_through_the_broker_and_converts_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let html: &'static [u8] = b"<html><body><h1>Hello</h1><p>World</p></body></html>";
        let recorded = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let server = tokio::spawn(run_mock_broker(
            socket.clone(),
            html,
            "text/html",
            200,
            std::sync::Arc::clone(&recorded),
        ));
        // Wait for the listener to bind.
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let (broker, _) = broker_with_mock(&socket);
        let action = WebFetchAction {
            broker,
            url: "https://example.com/page".to_string(),
            format: Some("markdown".to_string()),
            timeout: Some(30),
        };
        let output = action.run().await.unwrap();
        assert!(output.contains("Hello"), "markdown output: {output}");

        let requests = recorded.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, HttpMethod::Get);
        assert_eq!(requests[0].url, "https://example.com/page");
        assert!(requests[0].body.is_none());
        assert!(requests[0].headers.iter().any(|(key, _)| key == "Accept"));
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn fetch_reports_non_success_status() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker-404.sock");
        let recorded = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let server = tokio::spawn(run_mock_broker(
            socket.clone(),
            b"not found",
            "text/plain",
            404,
            std::sync::Arc::clone(&recorded),
        ));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let (broker, _) = broker_with_mock(&socket);
        let action = WebFetchAction {
            broker,
            url: "https://example.com/missing".to_string(),
            format: None,
            timeout: None,
        };
        let err = action.run().await.unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("404"), "error message: {message}");
        server.abort();
    }

    #[tokio::test]
    async fn fetch_rejects_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker-bin.sock");
        let recorded = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let server = tokio::spawn(run_mock_broker(
            socket.clone(),
            b"\x89PNG\r\n\x1a\n",
            "image/png",
            200,
            std::sync::Arc::clone(&recorded),
        ));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let (broker, _) = broker_with_mock(&socket);
        let action = WebFetchAction {
            broker,
            url: "https://example.com/image.png".to_string(),
            format: None,
            timeout: None,
        };
        let err = action.run().await.unwrap_err();
        let message = format!("{err:?}");
        assert!(message.contains("binary"), "error message: {message}");
        server.abort();
    }
}
