//! Host-owned `OAuth2` authorization flows (Authorization Code + PKCE).
//!
//! [`OAuthFlowManager`] runs the browser/redirect/token-exchange flow end to
//! end: it mints a `PKCE` verifier, binds an ephemeral loopback listener, opens
//! the authorization URL in the system browser, exchanges the returned code,
//! and stores the resulting token set in the vault and the credential
//! persistence file. Completion — success or failure — is pushed to every
//! connected plugin as an invalidation notice so the requesting plugin drops
//! its cached copies and re-resolves.
//!
//! The wire protocol is deliberately untouched: the flow is out-of-band and
//! the plugin observes it through the existing
//! `AuthorizationPending` → `Invalidated` → re-`Resolve` semantics.

pub(crate) mod exchange;
pub(crate) mod loopback;
pub(crate) mod persist;
pub(crate) mod pkce;
pub(crate) mod refresh;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ene_connector::CredentialStore;
use ene_connector::declaration::CredentialKind;
use ene_connector::identity::CredentialId;
use ene_connector::vault::{CredentialSummaryKind, CredentialVault};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::broadcast;

pub use persist::{CredentialPersister, FileCredentialPersister, PersistError};
pub use refresh::OAuthRefresher;

use crate::credential_registry::CredentialRegistry;

/// How long a flow waits for the authorization callback before giving up.
const FLOW_TIMEOUT: Duration = Duration::from_mins(5);

/// Errors produced by the OAuth authorization flow.
#[derive(Debug, Error)]
pub enum FlowError {
    /// The id is not declared by any plugin.
    #[error("credential '{0}' is not declared by any plugin")]
    NotDeclared(String),
    /// The declaration is not an `OAuth2` credential.
    #[error("credential '{0}' is not an OAuth2 credential")]
    UnsupportedKind(String),
    /// The system browser could not be opened.
    #[error("could not open the browser: {0}")]
    BrowserOpen(String),
    /// The authorization callback was rejected (state mismatch, server
    /// error, missing code).
    #[error("authorization callback rejected: {0}")]
    Callback(String),
    /// The token endpoint failed or returned a malformed response.
    #[error("token endpoint error: {0}")]
    TokenEndpoint(String),
    /// The token response could not be parsed.
    #[error("token response was malformed: {0}")]
    MalformedTokenResponse(String),
    /// The flow did not complete within [`FLOW_TIMEOUT`].
    #[error("the authorization flow timed out")]
    Timeout,
    /// Underlying socket error.
    #[error("authorization flow I/O error: {0}")]
    Io(#[source] std::io::Error),
    /// The credential persistence backend failed.
    #[error("credential persistence failed: {0}")]
    Persist(#[source] PersistError),
    /// An internal invariant was violated.
    #[error("authorization flow internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for FlowError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<PersistError> for FlowError {
    fn from(e: PersistError) -> Self {
        Self::Persist(e)
    }
}

/// The `OAuth2`-specific fields of a credential declaration, copied out so
/// the spawned flow task owns its data.
#[derive(Debug, Clone)]
struct OAuth2Endpoints {
    client_id: String,
    scopes: Vec<String>,
    auth_url: String,
    token_url: String,
}

/// Non-secret credential kind for list UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKindName {
    /// An `OAuth2` token set.
    OAuth2,
    /// A bare API key.
    ApiKey,
    /// No credential.
    None,
}

impl From<CredentialSummaryKind> for CredentialKindName {
    fn from(kind: CredentialSummaryKind) -> Self {
        match kind {
            CredentialSummaryKind::OAuth2 => Self::OAuth2,
            CredentialSummaryKind::ApiKey => Self::ApiKey,
            CredentialSummaryKind::None => Self::None,
        }
    }
}

/// Non-secret summary of one credential for list UIs.
///
/// Merges what the vault stores with what plugins currently declare as
/// `OAuth2`, so a declared-but-not-yet-authorized credential still shows up
/// with an "authorize" affordance. Never carries secret material.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CredentialInfo {
    /// Storage key (the plugin-visible id for shared declarations).
    pub id: String,
    /// Credential kind.
    pub kind: CredentialKindName,
    /// Access-token expiry, when known.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the vault holds a value for this credential.
    pub stored: bool,
    /// Whether the stored value is past its expiry (`false` when not stored).
    pub expired: bool,
}

/// Opens the authorization URL in the user's browser; injectable for tests.
type BrowserOpener = dyn Fn(&str) -> Result<(), String> + Send + Sync;

/// Drives OAuth authorization flows and credential revocation.
///
/// One flow per storage key: a second `start_*` while one is in flight is
/// coalesced onto the running flow instead of opening a second browser
/// window. The browser opener is injectable so tests can bypass the system
/// browser.
pub struct OAuthFlowManager {
    registry: Arc<CredentialRegistry>,
    /// Swappable vault snapshot: the runtime rebuilds the vault from
    /// configuration and swaps it in via [`Self::swap_vault`], which also
    /// updates the passenger. The read lock is held only for the duration of
    /// each call, never across an await point.
    vault: parking_lot::RwLock<Arc<CredentialVault>>,
    persister: Arc<dyn CredentialPersister>,
    invalidated_tx: broadcast::Sender<Vec<String>>,
    /// storage key → flow deadline; presence means a flow is in flight.
    pending: Mutex<HashMap<String, Instant>>,
    http: reqwest::Client,
    /// Opens the authorization URL in the user's browser.
    browser: Arc<BrowserOpener>,
}

impl OAuthFlowManager {
    /// Builds a flow manager sharing the passenger's invalidation channel.
    #[must_use]
    pub fn new(
        registry: Arc<CredentialRegistry>,
        vault: Arc<CredentialVault>,
        persister: Arc<dyn CredentialPersister>,
        invalidated_tx: broadcast::Sender<Vec<String>>,
    ) -> Self {
        Self {
            registry,
            vault: parking_lot::RwLock::new(vault),
            persister,
            invalidated_tx,
            pending: Mutex::new(HashMap::new()),
            http: reqwest::Client::new(),
            browser: Arc::new(|url| webbrowser::open(url).map_err(|e| e.to_string())),
        }
    }

    /// Swaps in the vault snapshot the runtime rebuilt from configuration.
    ///
    /// Called alongside the passenger's `replace_vault_and_broadcast` so a
    /// flow completing against the old snapshot cannot write its token into
    /// a vault the credential service no longer serves.
    pub fn swap_vault(&self, vault: Arc<CredentialVault>) {
        *self.vault.write() = vault;
    }

    /// Overrides the browser opener (tests inject a fake).
    #[must_use]
    pub fn with_browser(mut self, browser: Arc<BrowserOpener>) -> Self {
        self.browser = browser;
        self
    }

    /// Starts the authorization flow for `id` as requested by `plugin`.
    ///
    /// Returns `Ok(())` when a flow was started (or is already running for
    /// the same storage key). The flow completes out-of-band; completion is
    /// announced through the invalidation channel.
    pub fn start_authorization(self: &Arc<Self>, plugin: &str, id: &str) -> Result<(), FlowError> {
        let Ok(cred_id) = CredentialId::try_new(id) else {
            return Err(FlowError::NotDeclared(id.to_string()));
        };
        let declaration = self
            .registry
            .declaration(plugin, id)
            .ok_or_else(|| FlowError::NotDeclared(id.to_string()))?;
        let ScopeDecision::Allowed { storage_key } = self.registry.resolve_scope(plugin, &cred_id)
        else {
            return Err(FlowError::NotDeclared(id.to_string()));
        };
        let endpoints = oauth2_endpoints(&declaration, id)?;
        self.spawn_flow(plugin, id, &storage_key, endpoints);
        Ok(())
    }

    /// Starts the authorization flow for `id` without a requesting plugin
    /// (desktop settings page).
    ///
    /// Any plugin that declares `id` as a *shared* `OAuth2` credential
    /// qualifies; the storage key is the id itself. Private declarations
    /// (`shared: false`) stay reachable only through the owning plugin's own
    /// `RequestAuthorization`.
    pub fn start_authorization_by_id(self: &Arc<Self>, id: &str) -> Result<(), FlowError> {
        let Ok(cred_id) = CredentialId::try_new(id) else {
            return Err(FlowError::NotDeclared(id.to_string()));
        };
        let Some((declaration, plugin)) = self.registry.find_shared_declaration(&cred_id) else {
            return Err(FlowError::NotDeclared(id.to_string()));
        };
        let endpoints = oauth2_endpoints(&declaration, id)?;
        self.spawn_flow(&plugin, id, id, endpoints);
        Ok(())
    }

    /// Revokes credentials by storage key: drops the vault entries, removes
    /// them from the persistence file, and pushes an invalidation notice.
    ///
    /// Returns the number of entries removed from the persistence file.
    pub fn revoke(&self, ids: &[String]) -> Result<usize, FlowError> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.vault.read().invalidate(ids);
        let removed = self.persister.remove(ids)?;
        // Storage keys equal the plugin-visible ids for shared declarations
        // (the common case); private keys (`plugin:id`) never match a client's
        // declared scope, so that plugin is not pushed — it re-resolves to
        // Missing on its own.
        drop(self.invalidated_tx.send(ids.to_vec()));
        Ok(removed)
    }

    /// Lists every credential the vault stores plus every `OAuth2`
    /// declaration currently registered, as non-secret summaries.
    #[must_use]
    pub fn list_credentials(&self) -> Vec<CredentialInfo> {
        let stored = self.vault.read().list();
        let mut infos: Vec<CredentialInfo> = stored
            .into_iter()
            .map(|summary| CredentialInfo {
                id: summary.id,
                kind: CredentialKindName::from(summary.kind),
                expires_at: summary.expires_at,
                stored: true,
                expired: summary.expired,
            })
            .collect();
        let mut seen: std::collections::HashSet<String> =
            infos.iter().map(|i| i.id.clone()).collect();
        for declaration in self.registry.all_declarations() {
            let id = declaration.id.to_string();
            if seen.insert(id.clone()) && matches!(declaration.kind, CredentialKind::OAuth2 { .. })
            {
                infos.push(CredentialInfo {
                    id,
                    kind: CredentialKindName::OAuth2,
                    expires_at: None,
                    stored: false,
                    expired: false,
                });
            }
        }
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    /// Claims the flow slot for `storage_key`; returns `false` when a flow is
    /// already running (the caller coalesces).
    fn claim_flow(&self, storage_key: &str) -> bool {
        let mut pending = self.pending.lock();
        if pending.contains_key(storage_key) {
            return false;
        }
        pending.insert(storage_key.to_string(), Instant::now() + FLOW_TIMEOUT);
        true
    }

    fn release_flow(&self, storage_key: &str) {
        self.pending.lock().remove(storage_key);
    }

    fn spawn_flow(
        self: &Arc<Self>,
        plugin: &str,
        id: &str,
        storage_key: &str,
        endpoints: OAuth2Endpoints,
    ) {
        if !self.claim_flow(storage_key) {
            return;
        }
        let plugin = plugin.to_string();
        let id = id.to_string();
        let storage_key = storage_key.to_string();
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = this.run_flow(&plugin, &id, &storage_key, &endpoints).await;
            this.release_flow(&storage_key);
            // Success or failure both announce invalidation: a completed
            // credential must clear client caches, a failed one leaves the
            // vault empty so the next resolve reports Missing.
            drop(this.invalidated_tx.send(vec![id.clone()]));
            match &outcome {
                Ok(()) => tracing::info!(
                    component = "OAuthFlow",
                    credential_id = %id,
                    "OAuth authorization flow completed"
                ),
                Err(error) => tracing::warn!(
                    component = "OAuthFlow",
                    credential_id = %id,
                    error = %error,
                    "OAuth authorization flow failed"
                ),
            }
        });
    }

    async fn run_flow(
        &self,
        plugin: &str,
        id: &str,
        storage_key: &str,
        endpoints: &OAuth2Endpoints,
    ) -> Result<(), FlowError> {
        let verifier = pkce::verifier();
        let challenge = pkce::s256_challenge(&verifier);
        let server = loopback::LoopbackServer::bind().await?;
        let addr = server.local_addr()?;
        let redirect_uri = format!("http://127.0.0.1:{}/callback", addr.port());
        let state: u128 = rand::random();
        let state = format!("{state:x}");
        let auth_url = build_authorize_url(
            &endpoints.auth_url,
            &endpoints.client_id,
            &state,
            &challenge,
            &endpoints.scopes,
            &redirect_uri,
        )?;
        self.open_browser(&auth_url).await?;
        let (code, _) = server.wait_for_callback(&state, FLOW_TIMEOUT).await?;
        let tokens = exchange::exchange_code(
            &self.http,
            &endpoints.token_url,
            &endpoints.client_id,
            &code,
            &verifier,
            &redirect_uri,
        )
        .await?;
        let expires_at = tokens
            .expires_in
            .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
        let store = CredentialStore::oauth2(tokens.access_token, tokens.refresh_token, expires_at);
        self.vault.read().store(storage_key, store.clone());
        let mut entries = self.persister.load();
        entries.insert(storage_key.to_string(), store.expose_for_persistence());
        self.persister.save(&entries)?;
        self.vault.read().record_audit(plugin, id, true);
        Ok(())
    }

    async fn open_browser(&self, url: &str) -> Result<(), FlowError> {
        let url = url.to_string();
        let opener = Arc::clone(&self.browser);
        // webbrowser::open blocks on a spawned helper; run it off the async
        // worker so the flow task stays responsive.
        let result = tokio::task::spawn_blocking(move || opener(&url)).await;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(FlowError::BrowserOpen(error)),
            Err(_) => Err(FlowError::BrowserOpen(
                "the browser task was cancelled".to_string(),
            )),
        }
    }
}

/// Copies the `OAuth2` fields out of a declaration, rejecting non-`OAuth2`
/// kinds.
fn oauth2_endpoints(
    declaration: &ene_connector::CredentialDeclaration,
    id: &str,
) -> Result<OAuth2Endpoints, FlowError> {
    let CredentialKind::OAuth2 {
        client_id,
        scopes,
        auth_url,
        token_url,
    } = &declaration.kind
    else {
        return Err(FlowError::UnsupportedKind(id.to_string()));
    };
    Ok(OAuth2Endpoints {
        client_id: client_id.clone(),
        scopes: scopes.clone(),
        auth_url: auth_url.clone(),
        token_url: token_url.clone(),
    })
}

/// Builds the authorization URL with the `PKCE` challenge and CSRF state.
fn build_authorize_url(
    base: &str,
    client_id: &str,
    state: &str,
    challenge: &str,
    scopes: &[String],
    redirect_uri: &str,
) -> Result<String, FlowError> {
    let mut url =
        url::Url::parse(base).map_err(|e| FlowError::Internal(format!("invalid auth_url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scopes.join(" "));
    Ok(url.to_string())
}

use ene_connector::declaration::ScopeDecision;

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use expect/unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal OAuth authorization server for flow tests:
    /// `/authorize` redirects to the flow's loopback callback, `/token`
    /// issues tokens and verifies the `PKCE` verifier against the challenge
    /// it saw on `/authorize`.
    struct MockAuthServer {
        addr: String,
        _handle: tokio::task::JoinHandle<()>,
    }

    impl MockAuthServer {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                loop {
                    let Ok((socket, _)) = listener.accept().await else {
                        break;
                    };
                    let addr = addr.to_string();
                    tokio::spawn(async move {
                        let _ = serve_mock(socket, &addr).await;
                    });
                }
            });
            Self {
                addr: format!("http://{addr}"),
                _handle: handle,
            }
        }
    }

    async fn serve_mock(mut socket: tokio::net::TcpStream, addr: &str) -> Result<(), String> {
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 2048];
        loop {
            match socket.read(&mut chunk).await.map_err(|e| e.to_string())? {
                0 => return Ok(()),
                n => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if buf.len() > 16 * 1024 {
                        return Err("mock request too large".to_string());
                    }
                }
            }
        }
        let head_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("head end")
            + 4;
        let head = &buf[..head_end];
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut parsed = httparse::Request::new(&mut headers);
        let _ = parsed.parse(head).expect("parse mock request");
        let path = parsed.path.unwrap_or("/");
        let (path_only, query) = path.split_once('?').unwrap_or((path, ""));

        if path_only == "/authorize" {
            let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
            // Record the PKCE challenge so `/token` can verify the verifier
            // the flow later sends (the challenge is not re-sent to /token).
            *authorize_challenge() = params.get("code_challenge").cloned();
            let state = params.get("state").cloned().unwrap_or_default();
            let redirect_uri = params.get("redirect_uri").cloned().unwrap_or_default();
            let location = format!("{redirect_uri}?code=mock-code&state={state}");
            write_mock_response(
                &mut socket,
                &format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
                "",
            )
            .await?;
            return Ok(());
        }

        if path_only == "/token" {
            // The challenge the flow put on the wire is not re-sent to
            // `/token`, so `/authorize` must have recorded it for us.
            let challenge = authorize_challenge().clone();
            let content_length = headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("content-length"))
                .and_then(|h| std::str::from_utf8(h.value).ok())
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < head_end + content_length {
                let n = socket.read(&mut chunk).await.map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = &buf[head_end..head_end + content_length];
            let form: HashMap<String, String> =
                url::form_urlencoded::parse(body).into_owned().collect();
            let verifier_ok = form
                .get("code_verifier")
                .is_some_and(|v| Some(pkce::s256_challenge(v)) == challenge);
            if verifier_ok && form.get("code") == Some(&"mock-code".to_string()) {
                let body = r#"{"access_token":"mock-access","refresh_token":"mock-refresh","expires_in":3600}"#;
                write_mock_response(
                    &mut socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    body,
                )
                .await?;
            } else {
                let body = r#"{"error":"invalid_grant"}"#;
                write_mock_response(
                    &mut socket,
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    body,
                )
                .await?;
            }
            return Ok(());
        }

        write_mock_response(
            &mut socket,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n",
            "",
        )
        .await?;
        Ok(())
    }

    static AUTHORIZE_CHALLENGE: std::sync::OnceLock<parking_lot::Mutex<Option<String>>> =
        std::sync::OnceLock::new();

    fn authorize_challenge() -> parking_lot::MutexGuard<'static, Option<String>> {
        AUTHORIZE_CHALLENGE
            .get_or_init(|| parking_lot::Mutex::new(None))
            .lock()
    }

    async fn write_mock_response(
        socket: &mut tokio::net::TcpStream,
        head: &str,
        body: &str,
    ) -> Result<(), String> {
        let with_length = if head.to_ascii_lowercase().contains("content-length:") {
            head.to_string()
        } else {
            format!("{head}Content-Length: {}\r\n\r\n", body.len())
        };
        socket
            .write_all(with_length.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        socket
            .write_all(body.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        socket.flush().await.map_err(|e| e.to_string())
    }

    /// Issues a blocking `GET` (the browser seam is synchronous).
    fn http_get(url: &str) -> Result<String, String> {
        let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
        let host = parsed.host_str().ok_or("no host")?.to_string();
        let port = parsed.port().ok_or("no port")?;
        let path = match parsed.query() {
            Some(query) => format!("{}?{query}", parsed.path()),
            None => parsed.path().to_string(),
        };
        let mut stream =
            std::net::TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
        use std::io::{Read, Write};
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
        )
        .map_err(|e| e.to_string())?;
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|e| e.to_string())?;
        Ok(response)
    }

    fn extract_location(response: &str) -> Result<String, String> {
        response
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("location:"))
            .and_then(|line| line.split_once(':').map(|(_, v)| v.trim().to_string()))
            .ok_or_else(|| "no Location header in mock /authorize response".to_string())
    }

    fn oauth2_registry(base: &str) -> Arc<CredentialRegistry> {
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "mock",
            Some(&serde_json::json!({
                "x-ene-credentials": [{
                    "id": "google.calendar",
                    "kind": "oauth2",
                    "client_id": "client-id",
                    "scopes": ["calendar.readonly"],
                    "auth_url": format!("{base}/authorize"),
                    "token_url": format!("{base}/token")
                }]
            })),
        );
        Arc::new(registry)
    }

    fn fixtures(
        base: &str,
    ) -> (
        Arc<CredentialRegistry>,
        Arc<CredentialVault>,
        Arc<FileCredentialPersister>,
    ) {
        let registry = oauth2_registry(base);
        let vault = Arc::new(CredentialVault::new(Vec::new()));
        let dir = tempfile::tempdir().unwrap();
        let persister = Arc::new(FileCredentialPersister::new(
            dir.path().join("credentials.json"),
        ));
        (registry, vault, persister)
    }

    /// Browser seam that follows the mock `/authorize` redirect into the
    /// flow's loopback callback.
    fn redirecting_browser() -> Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync> {
        Arc::new(|url: &str| {
            let location = extract_location(&http_get(url)?)?;
            let _ = http_get(&location)?;
            Ok(())
        })
    }

    #[tokio::test]
    async fn full_flow_stores_credential_persists_and_broadcasts() {
        let server = MockAuthServer::start().await;
        let (registry, vault, persister) = fixtures(&server.addr);
        let (tx, mut rx) = broadcast::channel(8);
        let flow = Arc::new(
            OAuthFlowManager::new(registry, vault.clone(), persister.clone(), tx)
                .with_browser(redirecting_browser()),
        );
        flow.start_authorization("mock", "google.calendar").unwrap();

        let ids = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("flow must complete")
            .unwrap();
        assert_eq!(ids, vec!["google.calendar".to_string()]);

        let stored = vault.resolve("google.calendar").await.unwrap();
        assert_eq!(stored.access_token(), Some("mock-access"));
        assert_eq!(stored.refresh_token(), Some("mock-refresh"));
        let persisted = persister.load();
        assert!(persisted.contains_key("google.calendar"));
    }

    #[tokio::test]
    async fn mismatched_state_fails_flow_without_storing() {
        let server = MockAuthServer::start().await;
        let (registry, vault, persister) = fixtures(&server.addr);
        let (tx, mut rx) = broadcast::channel(8);
        let browser = Arc::new(|url: &str| -> Result<(), String> {
            let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
            let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            let redirect_uri = params.get("redirect_uri").ok_or("no redirect_uri")?;
            let _ = http_get(&format!("{redirect_uri}?code=stolen&state=wrong-state"))?;
            Ok(())
        });
        let flow = Arc::new(
            OAuthFlowManager::new(registry, vault.clone(), persister.clone(), tx)
                .with_browser(browser),
        );
        flow.start_authorization("mock", "google.calendar").unwrap();

        let ids = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("flow must fail")
            .unwrap();
        assert_eq!(ids, vec!["google.calendar".to_string()]);
        assert!(vault.resolve("google.calendar").await.is_err());
        assert!(persister.load().is_empty());
    }

    #[tokio::test]
    async fn concurrent_starts_coalesce_to_one_flow() {
        let server = MockAuthServer::start().await;
        let (registry, vault, persister) = fixtures(&server.addr);
        let (tx, mut rx) = broadcast::channel(8);
        let opens = Arc::new(AtomicUsize::new(0));
        let browser_opens = Arc::clone(&opens);
        let browser = Arc::new(move |url: &str| -> Result<(), String> {
            browser_opens.fetch_add(1, Ordering::SeqCst);
            let location = extract_location(&http_get(url)?)?;
            let _ = http_get(&location)?;
            Ok(())
        });
        let flow = Arc::new(
            OAuthFlowManager::new(registry, vault.clone(), persister.clone(), tx)
                .with_browser(browser),
        );
        flow.start_authorization("mock", "google.calendar").unwrap();
        flow.start_authorization("mock", "google.calendar").unwrap();

        let ids = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("flow must complete")
            .unwrap();
        assert_eq!(ids, vec!["google.calendar".to_string()]);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert!(vault.resolve("google.calendar").await.is_ok());
    }

    #[tokio::test]
    async fn start_authorization_rejects_non_oauth2_kinds() {
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "mock",
            Some(&serde_json::json!({
                "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key" }]
            })),
        );
        let vault = Arc::new(CredentialVault::new(Vec::new()));
        let dir = tempfile::tempdir().unwrap();
        let persister = Arc::new(FileCredentialPersister::new(
            dir.path().join("credentials.json"),
        ));
        let (tx, _rx) = broadcast::channel(8);
        let flow = Arc::new(OAuthFlowManager::new(
            Arc::new(registry),
            vault,
            persister,
            tx,
        ));
        let err = flow.start_authorization("mock", "anthropic").unwrap_err();
        assert!(matches!(err, FlowError::UnsupportedKind(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_flow_releases_the_flow_slot() {
        let server = MockAuthServer::start().await;
        let (registry, vault, persister) = fixtures(&server.addr);
        let (tx, mut rx) = broadcast::channel(8);
        // A browser that never delivers a callback.
        let flow = Arc::new(
            OAuthFlowManager::new(registry, vault, persister, tx)
                .with_browser(Arc::new(|_| Ok(()))),
        );
        flow.start_authorization("mock", "google.calendar").unwrap();
        // Step virtual time forward one second at a time; each advance lets
        // the spawned flow task run until it parks on its accept timeout,
        // and the advancing clock eventually fires that timeout.
        let mut elapsed = Duration::ZERO;
        let step = Duration::from_secs(1);
        while elapsed <= FLOW_TIMEOUT {
            tokio::time::advance(step).await;
            elapsed += step;
            if !flow.pending.lock().contains_key("google.calendar") {
                break;
            }
        }
        let ids = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("flow must time out")
            .unwrap();
        assert_eq!(ids, vec!["google.calendar".to_string()]);
        // The slot is free again: a fresh start would claim it.
        assert!(!flow.pending.lock().contains_key("google.calendar"));
    }
}
