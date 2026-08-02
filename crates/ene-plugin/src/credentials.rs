//! Credential client for the host's `credential` passenger.
//!
//! [`CredentialClient`] lets a plugin resolve host-held secrets over the
//! host-service channel without ever reading them from configuration itself:
//! the host resolves, scopes, and audits every access. Secrets returned to
//! plugin code are wrapped in [`CredentialSecret`], whose `Debug` and
//! `Serialize` always redact, so a key that reaches a log or a diagnostic
//! payload is redacted by construction.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use ene_plugin_proto::transport::IpcStream;
use ene_plugin_proto::{
    CredentialErrorCode, CredentialRequest, CredentialResponse, HostServiceErrorCode,
    HostServiceId, HostServiceRequest, HostServiceResponse, PluginError, ResolvedCredential,
    read_credential_response, read_host_service_response, write_credential_request,
    write_host_service_request,
};
use parking_lot::Mutex;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::oneshot;

#[cfg(feature = "http")]
use crate::policy::{RateLimiter, RetryPolicy, TimeoutPolicy};

/// How long a resolved API key is cached client-side before the next call
/// re-resolves through the host.
const API_KEY_CACHE_TTL: Duration = Duration::from_mins(1);

/// A secret returned to plugin code.
///
/// Wraps [`SecretString`] (zeroed on drop) and adds a `Serialize` impl that
/// always emits `<redacted>` — bare `SecretString` serializes its raw value,
/// which would leak a key into any payload that carries the credential. Raw
/// material is reachable only through the explicit [`expose_secret`](Self::expose_secret).
#[derive(Clone)]
pub struct CredentialSecret(SecretString);

impl CredentialSecret {
    /// Wraps a raw secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::new(value.into().into_boxed_str()))
    }

    /// Returns the raw value for SDK handoff (never log it).
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for CredentialSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl serde::Serialize for CredentialSecret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

/// Which credential variant a resolved value carries (drives the auth header
/// chosen by the HTTP helpers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialKind {
    /// Bare API key (`x-api-key`-style).
    ApiKey,
    /// OAuth access token (`Authorization: Bearer`-style).
    Bearer,
}

/// A cached resolved credential with its freshness deadline.
struct CachedCredential {
    kind: CredentialKind,
    value: CredentialSecret,
    deadline: Instant,
}

/// Endpoint parameters learned at handshake time.
#[derive(Debug, Clone)]
struct CredentialParams {
    socket_path: String,
    token: String,
}

/// One live session to the host's `credential` passenger.
struct CredentialConnection {
    /// Write half of the session stream; requests are framed here. The
    /// single-flight lock below serializes writers.
    writer: tokio::sync::Mutex<tokio::io::WriteHalf<IpcStream>>,
    /// Serializes requests: the wire protocol is single-flight, so only one
    /// request may be in flight per connection at a time.
    flight: tokio::sync::Mutex<()>,
    /// The pending response slot consumed by the reader task. Single-flight
    /// guarantees at most one occupant.
    pending: Mutex<Option<oneshot::Sender<CredentialResponse>>>,
    /// Set to `false` when the reader task exits (session died); requests
    /// then reconnect.
    alive: std::sync::atomic::AtomicBool,
}

/// Client for the host's `credential` service.
///
/// Connects lazily on first use (the handshake stays light), reconnects
/// automatically after a dropped session, and caches resolved credentials
/// with a short TTL. A server-initiated [`CredentialResponse::Invalidated`]
/// frame drops the matching cache entries immediately, so a revoked or
/// rotated credential is picked up on the next resolution.
pub struct CredentialClient {
    params: Mutex<Option<CredentialParams>>,
    conn: tokio::sync::Mutex<Option<Arc<CredentialConnection>>>,
    cache: Arc<Mutex<HashMap<String, CachedCredential>>>,
}

impl fmt::Debug for CredentialClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CredentialClient")
    }
}

impl Default for CredentialClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialClient {
    /// Creates a client with no endpoint; the host-service parameters are
    /// supplied by [`set_endpoint`](Self::set_endpoint) at handshake time.
    #[must_use]
    pub fn new() -> Self {
        Self {
            params: Mutex::new(None),
            conn: tokio::sync::Mutex::new(None),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Points the client at the host-service socket and `credential` token.
    ///
    /// Called by the plugin server once the handshake sandbox arrives. Until
    /// this is set (older hosts never send a credential token), every
    /// resolution fails with a "not configured" error rather than panicking.
    pub fn set_endpoint(&self, socket_path: String, token: String) {
        *self.params.lock() = Some(CredentialParams { socket_path, token });
    }

    /// Resolves `id` as an API key.
    ///
    /// Returns [`PluginError::CredentialMissing`] /
    /// [`PluginError::CredentialDenied`] /
    /// [`PluginError::AuthorizationRequired`] mapped from the host's
    /// structured error codes.
    pub async fn api_key(&self, id: &str) -> Result<CredentialSecret, PluginError> {
        match self.resolve_secret(id).await? {
            (CredentialKind::ApiKey, secret) => Ok(secret),
            (CredentialKind::Bearer, _) => Err(PluginError::provider(format!(
                "credential '{id}' is an OAuth bearer token, not an API key"
            ))),
        }
    }

    /// Resolves `id` as an OAuth access token.
    pub async fn bearer(&self, id: &str) -> Result<CredentialSecret, PluginError> {
        match self.resolve_secret(id).await? {
            (CredentialKind::Bearer, secret) => Ok(secret),
            (CredentialKind::ApiKey, _) => Err(PluginError::provider(format!(
                "credential '{id}' is an API key, not an OAuth bearer token"
            ))),
        }
    }

    /// Asks the host to start the authorization flow for `id`.
    ///
    /// The browser / redirect / token-exchange flow runs host-side; the host
    /// answers pending immediately and the credential arrives via a later
    /// invalidation.
    pub async fn request_authorization(&self, id: &str) -> Result<(), PluginError> {
        let resp = self
            .request(&CredentialRequest::RequestAuthorization { id: id.to_string() })
            .await?;
        match resp {
            CredentialResponse::AuthorizationPending => Ok(()),
            CredentialResponse::Error { code, message } => {
                Err(map_request_error(id, code, message))
            }
            other => Err(PluginError::provider(format!(
                "unexpected credential response: {other:?}"
            ))),
        }
    }

    /// Returns an HTTP client with the resolved credential's auth header
    /// baked in (`x-api-key` for API keys, `Authorization: Bearer` for
    /// OAuth). The client is cheap to build per call (reqwest pools
    /// connections internally) and re-resolves fresh credentials, so callers
    /// invoke it again after an OAuth refresh.
    #[cfg(feature = "http")]
    pub async fn http_client(&self, id: &str) -> Result<reqwest::Client, PluginError> {
        let (kind, secret) = self.resolve_secret(id).await?;
        let mut headers = reqwest::header::HeaderMap::new();
        match kind {
            CredentialKind::ApiKey => {
                headers.insert(
                    http::header::HeaderName::from_static("x-api-key"),
                    auth_header_value(secret.expose_secret())?,
                );
            }
            CredentialKind::Bearer => {
                headers.insert(
                    http::header::AUTHORIZATION,
                    auth_header_value(&format!("Bearer {}", secret.expose_secret()))?,
                );
            }
        }
        build_http_client(headers, TimeoutPolicy::default())
    }

    /// Returns a configurable caller (rate limit → retry → timeout) for
    /// `id`. `options.auth` overrides the header derived from the resolved
    /// credential.
    #[cfg(feature = "http")]
    pub async fn http_client_with(
        &self,
        id: &str,
        options: ClientOptions,
    ) -> Result<HttpCaller, PluginError> {
        let mut headers = reqwest::header::HeaderMap::new();
        match &options.auth {
            Some(HttpAuth::ApiKeyHeader(value)) => {
                headers.insert(
                    http::header::HeaderName::from_static("x-api-key"),
                    auth_header_value(value)?,
                );
            }
            Some(HttpAuth::Bearer) => {
                let (kind, secret) = self.resolve_secret(id).await?;
                if kind != CredentialKind::Bearer {
                    return Err(PluginError::provider(format!(
                        "credential '{id}' is not an OAuth bearer token"
                    )));
                }
                headers.insert(
                    http::header::AUTHORIZATION,
                    auth_header_value(&format!("Bearer {}", secret.expose_secret()))?,
                );
            }
            None => {
                let (kind, secret) = self.resolve_secret(id).await?;
                match kind {
                    CredentialKind::ApiKey => {
                        headers.insert(
                            http::header::HeaderName::from_static("x-api-key"),
                            auth_header_value(secret.expose_secret())?,
                        );
                    }
                    CredentialKind::Bearer => {
                        headers.insert(
                            http::header::AUTHORIZATION,
                            auth_header_value(&format!("Bearer {}", secret.expose_secret()))?,
                        );
                    }
                }
            }
        }
        let client = build_http_client(headers, options.timeout)?;
        Ok(HttpCaller {
            client,
            retry: options.retry,
            rate_limit: options.rate_limit,
            timeout: options.timeout,
        })
    }

    /// Resolves a credential through the cache or the wire, returning the
    /// variant alongside the secret.
    async fn resolve_secret(
        &self,
        id: &str,
    ) -> Result<(CredentialKind, CredentialSecret), PluginError> {
        if let Some(entry) = self.cache_get(id) {
            return Ok(entry);
        }
        let resolved = self.fetch(id).await?;
        let (kind, secret, deadline) = match resolved {
            ResolvedCredential::ApiKey { key } => (
                CredentialKind::ApiKey,
                CredentialSecret::new(key.expose().to_owned()),
                Instant::now() + API_KEY_CACHE_TTL,
            ),
            ResolvedCredential::Bearer { token, expires_at } => {
                let secret = CredentialSecret::new(token.expose().to_owned());
                let deadline = expires_at.map_or_else(
                    || Instant::now() + API_KEY_CACHE_TTL,
                    |expiry| {
                        let remaining = expiry
                            .signed_duration_since(Utc::now())
                            .to_std()
                            .unwrap_or(Duration::ZERO);
                        Instant::now() + remaining
                    },
                );
                (CredentialKind::Bearer, secret, deadline)
            }
        };
        self.cache_insert(id, kind, secret.clone(), deadline);
        Ok((kind, secret))
    }

    /// Fetches a credential from the host (bypassing the cache).
    async fn fetch(&self, id: &str) -> Result<ResolvedCredential, PluginError> {
        let resp = self
            .request(&CredentialRequest::Resolve { id: id.to_string() })
            .await?;
        match resp {
            CredentialResponse::Resolved { credential } => Ok(credential),
            CredentialResponse::Error { code, message } => {
                Err(map_request_error(id, code, message))
            }
            other => Err(PluginError::provider(format!(
                "unexpected credential response: {other:?}"
            ))),
        }
    }

    /// Sends one request over the connection, reconnecting once when the
    /// session has died.
    async fn request(&self, req: &CredentialRequest) -> Result<CredentialResponse, PluginError> {
        for _ in 0..2 {
            let conn = self.ensure_connected().await?;
            let flight = conn.flight.lock().await;
            let (tx, rx) = oneshot::channel();
            *conn.pending.lock() = Some(tx);
            let write_result = {
                let mut writer = conn.writer.lock().await;
                write_credential_request(&mut *writer, req).await
            };
            if write_result.is_err() {
                drop(flight);
                self.conn.lock().await.take();
                continue;
            }
            if let Ok(resp) = rx.await {
                return Ok(resp);
            }
            drop(flight);
            self.conn.lock().await.take();
        }
        Err(PluginError::transport("credential service connection lost"))
    }

    /// Returns the live connection, opening one on first use or after the
    /// previous session died.
    async fn ensure_connected(&self) -> Result<Arc<CredentialConnection>, PluginError> {
        let mut guard = self.conn.lock().await;
        if let Some(conn) = guard.as_ref()
            && conn.alive.load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(Arc::clone(conn));
        }
        let conn = self.open_connection().await?;
        *guard = Some(Arc::clone(&conn));
        Ok(conn)
    }

    /// Opens and authenticates a fresh credential session.
    async fn open_connection(&self) -> Result<Arc<CredentialConnection>, PluginError> {
        let params =
            self.params.lock().clone().ok_or_else(|| {
                PluginError::provider("credential service not configured by host")
            })?;
        let mut stream = IpcStream::connect(Path::new(&params.socket_path))
            .await
            .map_err(|e| {
                PluginError::transport(format!("failed to connect to host service: {e}"))
            })?;
        write_host_service_request(
            &mut stream,
            &HostServiceRequest::Open {
                service: HostServiceId::Credential,
                token: params.token,
            },
        )
        .await
        .map_err(PluginError::from)?;
        match read_host_service_response(&mut stream)
            .await
            .map_err(PluginError::from)?
        {
            Some(HostServiceResponse::OpenAck) => {}
            Some(HostServiceResponse::Error { code, message }) => {
                return Err(match code {
                    HostServiceErrorCode::UnknownService => {
                        PluginError::provider("credential service not implemented by host")
                    }
                    HostServiceErrorCode::AuthRejected => {
                        PluginError::provider("credential access denied by host")
                    }
                    HostServiceErrorCode::Internal => PluginError::provider(message),
                });
            }
            None => {
                return Err(PluginError::transport("host closed during credential open"));
            }
        }
        let (reader, writer) = tokio::io::split(stream);
        let conn = Arc::new(CredentialConnection {
            writer: tokio::sync::Mutex::new(writer),
            flight: tokio::sync::Mutex::new(()),
            pending: Mutex::new(None),
            alive: std::sync::atomic::AtomicBool::new(true),
        });
        tokio::spawn({
            let conn = Arc::clone(&conn);
            let cache = Arc::clone(&self.cache);
            async move {
                credential_reader_loop(reader, conn, cache).await;
            }
        });
        Ok(conn)
    }

    fn cache_get(&self, id: &str) -> Option<(CredentialKind, CredentialSecret)> {
        let guard = self.cache.lock();
        guard.get(id).and_then(|entry| {
            (entry.deadline > Instant::now()).then(|| (entry.kind, entry.value.clone()))
        })
    }

    fn cache_insert(
        &self,
        id: &str,
        kind: CredentialKind,
        value: CredentialSecret,
        deadline: Instant,
    ) {
        self.cache.lock().insert(
            id.to_string(),
            CachedCredential {
                kind,
                value,
                deadline,
            },
        );
    }
}

/// Forwards response frames and clears cache entries on invalidation pushes.
///
/// Exits when the stream ends (server restart / socket teardown); the next
/// request then reconnects.
async fn credential_reader_loop(
    mut reader: tokio::io::ReadHalf<IpcStream>,
    conn: Arc<CredentialConnection>,
    cache: Arc<Mutex<HashMap<String, CachedCredential>>>,
) {
    loop {
        match read_credential_response(&mut reader).await {
            Ok(Some(CredentialResponse::Invalidated { ids })) => {
                let mut guard = cache.lock();
                for id in ids {
                    guard.remove(&id);
                }
            }
            Ok(Some(resp)) => {
                if let Some(tx) = conn.pending.lock().take() {
                    drop(tx.send(resp));
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    conn.alive
        .store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Maps a host error code to the plugin-facing error, keeping secrets out of
/// the message.
fn map_request_error(id: &str, code: CredentialErrorCode, message: String) -> PluginError {
    match code {
        CredentialErrorCode::Missing { label, help_url } => {
            PluginError::credential_missing(id, label, help_url)
        }
        CredentialErrorCode::ScopeDenied => PluginError::credential_denied(id),
        CredentialErrorCode::RefreshRequired => PluginError::authorization_required(id),
        CredentialErrorCode::Unsupported
        | CredentialErrorCode::Internal
        | CredentialErrorCode::Unknown => PluginError::provider(message),
    }
}

/// Builds a reqwest client with the given default headers and timeouts.
#[cfg(feature = "http")]
fn build_http_client(
    headers: reqwest::header::HeaderMap,
    timeout: TimeoutPolicy,
) -> Result<reqwest::Client, PluginError> {
    reqwest::Client::builder()
        .default_headers(headers)
        .connect_timeout(timeout.connect_timeout)
        .timeout(timeout.request_timeout)
        .build()
        .map_err(|e| PluginError::provider(format!("failed to build HTTP client: {e}")))
}

/// Validates a header value without ever putting the secret in the error.
#[cfg(feature = "http")]
fn auth_header_value(value: &str) -> Result<http::header::HeaderValue, PluginError> {
    http::header::HeaderValue::from_str(value).map_err(|_| {
        PluginError::provider("credential contains characters invalid for an HTTP header")
    })
}

/// Auth header mode for [`ClientOptions`].
#[cfg(feature = "http")]
#[derive(Clone, PartialEq)]
pub enum HttpAuth {
    /// Send a fixed API key as the `x-api-key` header (bypasses resolution).
    ApiKeyHeader(String),
    /// Send the resolved access token as `Authorization: Bearer <token>`.
    Bearer,
}

impl fmt::Debug for HttpAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKeyHeader(_) => f.debug_tuple("ApiKeyHeader").field(&"<redacted>").finish(),
            Self::Bearer => f.debug_tuple("Bearer").finish(),
        }
    }
}

/// Options for [`CredentialClient::http_client_with`].
///
/// `auth` defaults to `None`, which derives the header from the resolved
/// credential (`api_key` → `x-api-key`, oauth → `Authorization: Bearer`).
#[cfg(feature = "http")]
#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    /// Retry policy applied by [`HttpCaller::execute`].
    pub retry: RetryPolicy,
    /// Optional token-bucket rate limiter applied before each request.
    pub rate_limit: Option<RateLimiter>,
    /// Connection/request timeouts applied by the built client.
    pub timeout: TimeoutPolicy,
    /// Auth header override; `None` derives it from the resolved credential.
    pub auth: Option<HttpAuth>,
}

/// HTTP caller combining a shared client with retry / rate-limit / timeout.
#[cfg(feature = "http")]
#[derive(Debug)]
pub struct HttpCaller {
    client: reqwest::Client,
    retry: RetryPolicy,
    rate_limit: Option<RateLimiter>,
    timeout: TimeoutPolicy,
}

#[cfg(feature = "http")]
impl HttpCaller {
    /// The underlying client (carries the credential's auth header); start
    /// requests from it so the header applies.
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Executes a request, applying rate limit → retry → timeout.
    ///
    /// The retry policy re-runs the request on any error; configure
    /// `RetryPolicy::max_retries` to bound it (0 disables retries). The
    /// timeout bounds each attempt.
    pub async fn execute(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, PluginError> {
        if let Some(limiter) = &self.rate_limit {
            limiter.acquire().await;
        }
        let request = builder
            .build()
            .map_err(|e| PluginError::provider(format!("failed to build request: {e}")))?;
        let client = self.client.clone();
        let per_attempt = self.timeout.request_timeout;
        self.retry
            .retry(
                move |_attempt| {
                    let client = client.clone();
                    let request = request.try_clone();
                    async move {
                        let Some(request) = request else {
                            return Err(PluginError::provider("request body is not cloneable"));
                        };
                        match tokio::time::timeout(per_attempt, client.execute(request)).await {
                            Ok(Ok(resp)) => Ok(resp),
                            Ok(Err(e)) => {
                                Err(PluginError::provider(format!("HTTP request failed: {e}")))
                            }
                            Err(_) => Err(PluginError::timeout("request exceeded timeout")),
                        }
                    }
                },
                |_: &PluginError| true,
            )
            .await
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "tests use expect for concise failure messages"
)]
mod tests {
    use super::*;
    use ene_plugin_proto::transport::IpcListener;
    use ene_plugin_proto::{
        read_credential_request, read_host_service_request, write_credential_response,
        write_host_service_response,
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Runs a minimal in-process credential host over a real socket: accepts
    /// one connection, answers `Open` with a fixed token, and serves `Resolve`
    /// with a fixed API key.
    fn spawn_mock_host() -> (String, String) {
        let n = SOCKET_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ene-cred-client-test-{}-{n}.sock",
            std::process::id()
        ));
        let mut listener = IpcListener::bind(&path).expect("bind listener");
        let token = "ene-cred-test-token".to_string();
        tokio::spawn(async move {
            let Ok(mut stream) = listener.accept().await else {
                return;
            };
            let Ok(Some(req)) = read_host_service_request(&mut stream).await else {
                return;
            };
            if !matches!(
                req,
                HostServiceRequest::Open {
                    service: HostServiceId::Credential,
                    ..
                }
            ) {
                return;
            }
            drop(write_host_service_response(&mut stream, &HostServiceResponse::OpenAck).await);
            loop {
                let Ok(Some(request)) = read_credential_request(&mut stream).await else {
                    break;
                };
                let resp = match request {
                    CredentialRequest::Resolve { id } if id == "anthropic" => {
                        CredentialResponse::Resolved {
                            credential: ResolvedCredential::ApiKey {
                                key: ene_plugin_proto::WireSecret::new("sk-mock-host-key"),
                            },
                        }
                    }
                    CredentialRequest::Resolve { .. } => CredentialResponse::Error {
                        code: CredentialErrorCode::Missing {
                            label: "missing".into(),
                            help_url: None,
                        },
                        message: "credential not configured".into(),
                    },
                    CredentialRequest::RequestAuthorization { .. } => {
                        CredentialResponse::AuthorizationPending
                    }
                    CredentialRequest::Ping => CredentialResponse::Pong,
                };
                if write_credential_response(&mut stream, &resp).await.is_err() {
                    break;
                }
            }
        });
        (path.to_string_lossy().to_string(), token)
    }

    fn client_with_endpoint() -> CredentialClient {
        let (path, token) = spawn_mock_host();
        let client = CredentialClient::new();
        client.set_endpoint(path, token);
        client
    }

    #[tokio::test]
    async fn api_key_resolves_through_host_and_redacts() {
        let client = client_with_endpoint();
        let key = client.api_key("anthropic").await.expect("resolve");
        assert_eq!(key.expose_secret(), "sk-mock-host-key");
        assert!(!format!("{key:?}").contains("sk-mock-host-key"));
        assert_eq!(
            serde_json::to_string(&key).expect("serialize"),
            "\"<redacted>\""
        );
    }

    #[tokio::test]
    async fn unconfigured_client_reports_not_configured() {
        let client = CredentialClient::new();
        let err = client.api_key("anthropic").await.expect_err("no endpoint");
        assert!(err.to_string().contains("not configured"));
        assert!(!err.to_string().contains("sk-mock-host-key"));
    }

    #[tokio::test]
    async fn missing_credential_maps_to_credential_missing() {
        let client = client_with_endpoint();
        let err = client
            .api_key("google.calendar")
            .await
            .expect_err("missing credential");
        assert!(matches!(err, PluginError::CredentialMissing { .. }));
        assert!(!err.to_string().contains("sk-mock-host-key"));
    }

    #[tokio::test]
    async fn request_authorization_returns_ok_on_pending() {
        let client = client_with_endpoint();
        client
            .request_authorization("anthropic")
            .await
            .expect("pending accepted");
    }
}
