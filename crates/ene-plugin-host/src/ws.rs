//! Host-mediated WebSocket sessions (the `WebSocket` broker passenger).
//!
//! A plugin opens a session with [`WebSocketRequest::Open`]; the host
//! validates the origin (the HTTPS-equivalent origin of a `wss://` URL is
//! approved with the same categories as `https://`), pins the resolved
//! address (SSRF/DNS-rebinding guard), injects the named credential, and
//! relays frames in both directions until either side closes.

use std::sync::Arc;

use ene_plugin_proto::BrokerErrorCode;
use ene_plugin_proto::ws::{WebSocketRequest, WebSocketResponse};
use ene_plugin_proto::{read_framed_json, write_framed_json};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::either::Either;

use crate::broker::{BrokerError, BrokerHub};

/// Duplex stream: plain TCP for `ws://`, rustls for `wss://`.
type WsIo = Either<tokio::net::TcpStream, tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

/// Maximum size of one relayed WebSocket message (4 MiB; Edge audio chunks
/// are a few hundred KiB at most).
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

enum WsCommand {
    Text(String),
    Binary(Vec<u8>),
    Close { code: u16, reason: String },
    ProtocolError(String),
}

impl BrokerHub {
    pub(crate) async fn serve_ws_session(
        &self,
        plugin: &str,
        mut stream: ene_plugin_proto::transport::IpcStream,
    ) -> std::io::Result<()> {
        let Some(request) = read_framed_json::<_, WebSocketRequest>(&mut stream).await? else {
            return Ok(());
        };
        let WebSocketRequest::Open {
            url,
            headers,
            credential,
        } = request
        else {
            write_framed_json(
                &mut stream,
                &WebSocketResponse::Error {
                    status: None,
                    message: "the first frame of a WebSocket session must be Open".to_string(),
                },
            )
            .await?;
            return Ok(());
        };

        let result = async {
            let Some(state) = self.plugins.get(plugin) else {
                return Err(BrokerError::new(
                    BrokerErrorCode::NotDeclared,
                    format!("plugin '{plugin}' has no verified manifest"),
                ));
            };
            Self::require_service(state, "network")?;
            self.ws_connect(plugin, state, &url, &headers, credential.as_deref())
                .await
        }
        .await;

        match result {
            Ok(ws) => Self::ws_relay(ws, stream, url).await,
            Err(e) => {
                write_framed_json(
                    &mut stream,
                    &WebSocketResponse::Error {
                        status: e.http_status,
                        message: e.message,
                    },
                )
                .await
            }
        }
    }
}

impl BrokerHub {
    /// Maps a WebSocket handshake failure to a broker error, preserving the
    /// HTTP status (and the `Date` header, which the Edge service uses for
    /// its 5-minute token window) for the plugin.
    fn ws_handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> BrokerError {
        if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
            let status = response.status();
            let mut message = format!("WebSocket handshake failed: HTTP {status}");
            if let Some(date) = response
                .headers()
                .get(http::header::DATE)
                .and_then(|value| value.to_str().ok())
            {
                message.push_str(" Date: ");
                message.push_str(date);
            }
            return BrokerError::with_http_status(
                BrokerErrorCode::Internal,
                message,
                status.as_u16(),
            );
        }
        BrokerError::new(
            BrokerErrorCode::Internal,
            format!("WebSocket handshake failed: {error}"),
        )
    }

    /// Validates and connects a WebSocket URL, returning the upgraded
    /// stream. Applies the same gates as an HTTPS fetch: origin approval,
    /// SSRF with address pinning, and credential injection.
    async fn ws_connect(
        &self,
        plugin: &str,
        state: &crate::broker::PluginState,
        url: &str,
        headers: &[(String, String)],
        credential: Option<&str>,
    ) -> Result<WebSocketStream<WsIo>, BrokerError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?;
        let scheme = parsed.scheme();
        if scheme != "ws" && scheme != "wss" {
            return Err(BrokerError::new(
                BrokerErrorCode::InvalidTarget,
                "only ws:// and wss:// URLs are supported",
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| BrokerError::new(BrokerErrorCode::InvalidTarget, "URL has no host"))?;
        let port = parsed
            .port_or_known_default()
            .unwrap_or(if scheme == "wss" { 443 } else { 80 });
        // `wss://` carries the same trust as `https://`; approve the
        // HTTPS-equivalent origin so fixed-origin manifests work unchanged.
        let http_scheme = if scheme == "wss" { "https" } else { "http" };
        let origin = crate::broker::normalized_origin_with_scheme(&parsed, http_scheme);
        let category = Self::origin_category(state, &origin, http_scheme)?;
        self.approve(plugin, state, category, &origin).await?;

        let ssrf = self.ssrf.read().clone();
        let ips = ssrf
            .resolve_allowed(host)
            .await
            .map_err(|e| BrokerError::denied(format!("SSRF guard: {e}")))?;
        let Some(ip) = ips.first() else {
            return Err(BrokerError::denied("SSRF guard: no allowed address"));
        };

        let tcp = tokio::net::TcpStream::connect((*ip, port))
            .await
            .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?;
        let io: WsIo = if scheme == "wss" {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let domain = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|_| BrokerError::new(BrokerErrorCode::InvalidTarget, "invalid host"))?;
            Either::Right(
                connector
                    .connect(domain, tcp)
                    .await
                    .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?,
            )
        } else {
            Either::Left(tcp)
        };

        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                url.to_string(),
            )
            .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?;
        // The Host header must carry the exact authority from the URL,
        // including a non-default port and IPv6 brackets.
        let host_header: http::header::HeaderValue = Self::host_authority(&parsed)
            .parse::<http::header::HeaderValue>()
            .map_err(|e| BrokerError::new(BrokerErrorCode::InvalidTarget, e.to_string()))?;
        request
            .headers_mut()
            .insert(http::header::HOST, host_header);
        for (key, value) in headers {
            if crate::broker::is_forbidden_request_header(key) || key.eq_ignore_ascii_case("host") {
                continue;
            }
            if let (Ok(key), Ok(value)) = (
                http::header::HeaderName::try_from(key),
                http::header::HeaderValue::try_from(value),
            ) {
                request.headers_mut().insert(key, value);
            }
        }
        if let Some(key) = credential {
            self.approve(plugin, state, crate::ApprovalCategory::CredentialUse, key)
                .await?;
            let value = state.credentials.get(key).ok_or_else(|| {
                BrokerError::new(
                    BrokerErrorCode::NotFound,
                    format!("credential '{key}' not found"),
                )
            })?;
            request.headers_mut().insert(
                http::header::AUTHORIZATION,
                http::header::HeaderValue::try_from(format!("Bearer {value}"))
                    .map_err(|e| BrokerError::new(BrokerErrorCode::Internal, e.to_string()))?,
            );
        }

        let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        config.max_message_size = Some(MAX_MESSAGE_BYTES);
        config.max_frame_size = Some(MAX_MESSAGE_BYTES);
        let (ws, _response) =
            tokio_tungstenite::client_async_with_config(request, io, Some(config))
                .await
                .map_err(|e| Self::ws_handshake_error(&e))?;
        Ok(ws)
    }

    /// Serializes a URL's authority for the Host header: IPv6 hosts
    /// bracketed, an explicit port preserved.
    fn host_authority(url: &url::Url) -> String {
        let authority = url.host().map_or_else(String::new, |host| host.to_string());
        match url.port() {
            Some(port) => format!("{authority}:{port}"),
            None => authority,
        }
    }

    async fn ws_relay(
        ws: WebSocketStream<WsIo>,
        stream: ene_plugin_proto::transport::IpcStream,
        final_url: String,
    ) -> std::io::Result<()> {
        let (mut ipc_read, mut ipc_write) = tokio::io::split(stream);
        let (mut ws_sink, mut ws_stream) = ws.split();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<WsCommand>(32);
        write_framed_json(&mut ipc_write, &WebSocketResponse::OpenOk { final_url }).await?;

        let push = tokio::spawn(async move {
            let mut result: std::io::Result<()> = Ok(());
            'relay: loop {
                tokio::select! {
                    incoming = ws_stream.next() => {
                        match incoming {
                            Some(Ok(Message::Text(text))) => {
                                if let Err(e) = write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::MessageText {
                                        data: text.to_string(),
                                    },
                                ).await {
                                    result = Err(e);
                                    break 'relay;
                                }
                            }
                            Some(Ok(Message::Binary(binary))) => {
                                if let Err(e) = write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::MessageBinary { data: binary.to_vec() },
                                ).await {
                                    result = Err(e);
                                    break 'relay;
                                }
                            }
                            Some(Ok(Message::Close(frame))) => {
                                let (code, reason) = frame.map_or(
                                    (
                                        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Away,
                                        "closed".to_string(),
                                    ),
                                    |f| (f.code, f.reason.to_string()),
                                );
                                drop(write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::Closed {
                                        code: code.into(),
                                        reason,
                                    },
                                ).await);
                                break 'relay;
                            }
                            Some(Ok(Message::Ping(payload))) => {
                                if ws_sink.send(Message::Pong(payload)).await.is_err() {
                                    break 'relay;
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                drop(write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::Closed {
                                        code: 1006,
                                        reason: e.to_string(),
                                    },
                                ).await);
                                break 'relay;
                            }
                            None => {
                                drop(write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::Closed {
                                        code: 1006,
                                        reason: "connection closed".to_string(),
                                    },
                                ).await);
                                break 'relay;
                            }
                        }
                    }
                    command = cmd_rx.recv() => {
                        let Some(command) = command else { break 'relay };
                        let is_close = matches!(command, WsCommand::Close { .. });
                        let protocol_error = match &command {
                            WsCommand::ProtocolError(message) => Some(message.clone()),
                            _ => None,
                        };
                        let message = match command {
                            WsCommand::Text(data) => Message::Text(data.into()),
                            WsCommand::Binary(data) => Message::Binary(data.into()),
                            WsCommand::Close { code, reason } => {
                                Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::from(code),
                                    reason: reason.into(),
                                }))
                            }
                            WsCommand::ProtocolError(message) => {
                                drop(write_framed_json(
                                    &mut ipc_write,
                                    &WebSocketResponse::Error {
                                        status: None,
                                        message,
                                    },
                                )
                                .await);
                                break 'relay;
                            }
                        };
                        if ws_sink.send(message).await.is_err() {
                            drop(write_framed_json(
                                &mut ipc_write,
                                &WebSocketResponse::Error {
                                    status: None,
                                    message: "failed to send on the WebSocket".to_string(),
                                },
                            ).await);
                            break 'relay;
                        }
                        if let Some(message) = protocol_error {
                            drop(write_framed_json(
                                &mut ipc_write,
                                &WebSocketResponse::Error {
                                    status: None,
                                    message: message.clone(),
                                },
                            )
                            .await);
                            break 'relay;
                        }
                        if is_close {
                            // Wait for the peer's close reply; it arrives as
                            // the next `ws_stream.next()` item.
                        }
                    }
                }
            }
            result
        });

        loop {
            let Some(request) = read_framed_json::<_, WebSocketRequest>(&mut ipc_read).await?
            else {
                break;
            };
            match request {
                WebSocketRequest::SendText { data } => {
                    if cmd_tx.send(WsCommand::Text(data)).await.is_err() {
                        break;
                    }
                }
                WebSocketRequest::SendBinary { data } => {
                    if cmd_tx.send(WsCommand::Binary(data)).await.is_err() {
                        break;
                    }
                }
                WebSocketRequest::Close { code, reason } => {
                    if cmd_tx
                        .send(WsCommand::Close { code, reason })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    // Give the relay a moment to complete the close
                    // handshake, then tear the session down.
                    drop(tokio::time::timeout(std::time::Duration::from_secs(5), push).await);
                    return Ok(());
                }
                WebSocketRequest::Open { .. } => {
                    if cmd_tx
                        .send(WsCommand::ProtocolError(
                            "Open must be the first frame of a session".to_string(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    break;
                }
            }
        }
        drop(cmd_tx);
        drop(push.await);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginEntry;
    use crate::broker::PluginState;
    use crate::config::PluginConfig;
    use ene_approval::{ApprovalCategory, ApprovalMode, ApprovalPolicy, OriginDecl};
    use ene_plugin_proto::transport::IpcStream;
    use futures::{SinkExt, StreamExt};
    use std::collections::HashMap;
    use tokio::net::UnixStream;
    use tokio_tungstenite::tungstenite::Message;

    fn test_config() -> PluginConfig {
        let mut policy = ApprovalPolicy::default();
        policy
            .categories
            .insert(ApprovalCategory::FixedOriginNetwork, ApprovalMode::Allow);
        PluginConfig {
            enabled: true,
            approval: policy,
            list: HashMap::from([("edge-tts".to_string(), PluginEntry::default())]),
            ..PluginConfig::default()
        }
    }

    fn test_hub() -> Arc<BrokerHub> {
        let mut full = ene_config::EneConfig::default();
        full.set_section(&test_config())
            .expect("set plugin section");
        BrokerHub::from_config(&full).expect("hub")
    }

    /// The production edge-tts URL normalizes to the manifest's exact
    /// origin and is approved as a fixed origin (regression: the old
    /// unconditional `:443` never matched).
    #[test]
    fn edge_tts_production_origin_passes_builtin_manifest() {
        let hub = test_hub();
        let state = hub.plugins.get("edge-tts").expect("builtin state");
        assert_eq!(
            crate::manifest::builtin_manifest("edge-tts")
                .expect("edge-tts manifest")
                .fixed_origins,
            vec![OriginDecl {
                origin: "https://speech.platform.bing.com".into(),
            }]
        );

        let parsed = url::Url::parse(
            "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1",
        )
        .expect("production URL");
        let origin = crate::broker::normalized_origin_with_scheme(&parsed, "https");
        assert_eq!(origin, "https://speech.platform.bing.com");
        let category = BrokerHub::origin_category(state, &origin, "https").expect("category");
        assert_eq!(category, ApprovalCategory::FixedOriginNetwork);
    }

    #[test]
    fn origins_and_host_authorities_match_http_normalization() {
        let wss = url::Url::parse("wss://speech.platform.bing.com:8443/edge/v1").unwrap();
        assert_eq!(
            crate::broker::normalized_origin_with_scheme(&wss, "https"),
            "https://speech.platform.bing.com:8443"
        );
        assert_eq!(
            BrokerHub::host_authority(&wss),
            "speech.platform.bing.com:8443"
        );

        let ipv6 = url::Url::parse("ws://[::1]:8080/echo").unwrap();
        assert_eq!(
            crate::broker::normalized_origin_with_scheme(&ipv6, "http"),
            "http://[::1]:8080"
        );
        assert_eq!(BrokerHub::host_authority(&ipv6), "[::1]:8080");

        let plain = url::Url::parse("ws://[::1]/echo").unwrap();
        assert_eq!(
            crate::broker::normalized_origin_with_scheme(&plain, "http"),
            "http://[::1]"
        );
        assert_eq!(BrokerHub::host_authority(&plain), "[::1]");
    }

    #[tokio::test]
    async fn ws_session_relays_through_local_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake");
            let (mut sink, mut incoming) = ws.split();
            while let Some(message) = incoming.next().await {
                match message.expect("message") {
                    Message::Text(text) => {
                        sink.send(Message::text(text)).await.expect("echo");
                    }
                    Message::Close(_) => {
                        // tungstenite replies to the close automatically;
                        // flushing sends that reply before the socket drops.
                        drop(sink.flush().await);
                        break;
                    }
                    Message::Ping(payload) => {
                        sink.send(Message::Pong(payload)).await.expect("pong");
                    }
                    _ => {}
                }
            }
        });

        let mut hub = test_hub();
        hub.allow_loopback_for_tests();
        let mut manifest = crate::manifest::builtin_manifest("edge-tts").expect("edge-tts");
        manifest.fixed_origins = vec![OriginDecl {
            origin: format!("http://{addr}"),
        }];
        Arc::get_mut(&mut hub)
            .expect("sole hub owner")
            .plugins
            .insert(
                "edge-tts".to_string(),
                PluginState {
                    manifest: Some(manifest),
                    digest: None,
                    fs_grants: Vec::new(),
                    credentials: std::collections::BTreeMap::new(),
                    processes: parking_lot::Mutex::new(HashMap::new()),
                },
            );

        let (client_io, server_io) = UnixStream::pair().expect("socket pair");
        let session = tokio::spawn({
            let hub = Arc::clone(&hub);
            async move {
                hub.serve_ws_session("edge-tts", IpcStream::Unix(server_io))
                    .await
            }
        });
        let mut io = IpcStream::Unix(client_io);
        let url = format!("ws://{addr}/echo");
        write_framed_json(
            &mut io,
            &WebSocketRequest::Open {
                url: url.clone(),
                headers: Vec::new(),
                credential: None,
            },
        )
        .await
        .expect("open");
        assert_eq!(
            read_framed_json::<_, WebSocketResponse>(&mut io)
                .await
                .expect("read")
                .expect("frame"),
            WebSocketResponse::OpenOk { final_url: url }
        );
        write_framed_json(
            &mut io,
            &WebSocketRequest::SendText {
                data: "ping".to_string(),
            },
        )
        .await
        .expect("send");
        assert_eq!(
            read_framed_json::<_, WebSocketResponse>(&mut io)
                .await
                .expect("read")
                .expect("frame"),
            WebSocketResponse::MessageText {
                data: "ping".to_string(),
            }
        );
        write_framed_json(
            &mut io,
            &WebSocketRequest::Close {
                code: 1000,
                reason: "done".to_string(),
            },
        )
        .await
        .expect("close");
        let closed = read_framed_json::<_, WebSocketResponse>(&mut io)
            .await
            .expect("read")
            .expect("frame");
        assert!(matches!(
            closed,
            WebSocketResponse::Closed { code: 1000, .. }
        ));
        session.await.expect("session task").expect("session ok");
        server.abort();
    }
}
