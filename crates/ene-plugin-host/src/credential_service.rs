//! Host-side `credential` passenger: authentication, scope matching,
//! resolution, audit, and invalidation push.
//!
//! This is the wire layer that knows both [`ene_connector`] (the vault) and
//! [`ene_plugin_proto`] (the frames); it is the only place that link exists.
//! The passenger authenticates the `Open` token against its own map, matches
//! every requested id against the plugin's declared scope *server-side*
//! (through the declaration registry shared with
//! [`crate::manager::PluginHostManager`]), and never lets a secret reach a
//! log, audit record, or error message.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ene_connector::ConnectorError;
use ene_connector::declaration::ScopeDecision;
use ene_connector::identity::CredentialId;
use ene_connector::vault::CredentialVault;
use ene_plugin_proto::transport::IpcStream;
use ene_plugin_proto::{
    CredentialErrorCode, CredentialRequest, CredentialResponse, HostServiceErrorCode,
    HostServicePassenger, HostServiceResponse, ResolvedCredential, WireSecret,
    read_credential_request, write_credential_response, write_host_service_response,
};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tracing::warn;

use crate::credential_registry::CredentialRegistry;

/// Number of buffered invalidation broadcasts per subscriber before the
/// receiver lags (a slow client then drops its full declared scope).
const INVALIDATED_BUFFER: usize = 64;

/// Minimum interval between `warn`-level logs of rejected credential `Open`
/// attempts, so brute-force probing of the shared socket cannot flood the log
/// while the cumulative count stays measurable (mirrors the store's tracker
/// for the `db` service).
const OPEN_FAILURE_LOG_INTERVAL: Duration = Duration::from_secs(1);

/// Tracks rejected credential `Open` attempts for rate-limited logging.
#[derive(Default)]
struct OpenFailureTracker {
    count: u64,
    last_logged: Option<Instant>,
}

impl OpenFailureTracker {
    /// Records one rejection; returns `true` when the caller should log it
    /// (at most once per second).
    fn record(&mut self) -> bool {
        self.count = self.count.saturating_add(1);
        let now = Instant::now();
        let should_log = self
            .last_logged
            .is_none_or(|t| now.duration_since(t) >= OPEN_FAILURE_LOG_INTERVAL);
        if should_log {
            self.last_logged = Some(now);
        }
        should_log
    }

    fn count(&self) -> u64 {
        self.count
    }
}

/// Per-plugin registration for the `credential` passenger.
///
/// The plugin name is the only identity the passenger needs: it is derived
/// from the pre-shared token (unforgeable) and used for scope matching and
/// audit. The declared credential ids live in the shared
/// [`CredentialRegistry`].
#[derive(Debug, Clone)]
pub struct CredentialPluginRegistration {
    /// Plugin binary name.
    pub plugin: String,
}

/// Wire-layer `credential` passenger.
pub struct CredentialPassenger {
    vault: Arc<CredentialVault>,
    /// Declaration registry shared with
    /// [`crate::manager::PluginHostManager`], which
    /// populates it from each plugin's `config_schema()` at startup. The
    /// passenger is built before the manager registers anything, but no
    /// plugin can open a credential session until the manager has spawned it,
    /// so request-time scope resolution always sees the final declarations.
    registry: Arc<CredentialRegistry>,
    /// Pre-shared token → plugin registration.
    registrations: HashMap<String, CredentialPluginRegistration>,
    invalidated_tx: broadcast::Sender<Vec<String>>,
    failed_opens: Mutex<OpenFailureTracker>,
}

impl CredentialPassenger {
    /// Builds a passenger from the vault, the declaration registry, and the
    /// token registrations issued at host-service spawn time.
    #[must_use]
    pub fn new(
        vault: Arc<CredentialVault>,
        registry: Arc<CredentialRegistry>,
        registrations: HashMap<String, CredentialPluginRegistration>,
    ) -> Self {
        let (invalidated_tx, _) = broadcast::channel(INVALIDATED_BUFFER);
        Self {
            vault,
            registry,
            registrations,
            invalidated_tx,
            failed_opens: Mutex::new(OpenFailureTracker::default()),
        }
    }

    /// Pushes an invalidation notice to every connected client whose declared
    /// scope includes any of `ids`. The runtime calls this when a credential
    /// is updated or revoked (live config-change detection lands with the
    /// OAuth flow; tests exercise the path directly).
    pub fn broadcast_invalidated(&self, ids: Vec<String>) {
        drop(self.invalidated_tx.send(ids));
    }

    /// Runs the request/invalidation loop for one authenticated session.
    ///
    /// Single-flight by design: one request is in flight at a time, and the
    /// invalidation push shares the same stream.
    async fn serve_session(
        &self,
        stream: &mut IpcStream,
        plugin: &str,
        mut invalidated_rx: broadcast::Receiver<Vec<String>>,
    ) {
        loop {
            tokio::select! {
                frame = read_credential_request(stream) => {
                    let Ok(Some(request)) = frame else { break };
                    let response = self.handle_request(plugin, request);
                    if write_credential_response(stream, &response).await.is_err() {
                        break;
                    }
                }
                recv = invalidated_rx.recv() => {
                    let ids = match recv {
                        Ok(ids) => ids,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // A slow client missed one or more invalidation
                            // frames and cannot know which ids were revoked;
                            // invalidate its full declared scope.
                            self.registry
                                .declarations(plugin)
                                .into_iter()
                                .map(|decl| decl.id.as_str().to_string())
                                .collect()
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let allowed: Vec<String> = ids
                        .into_iter()
                        .filter(|id| self.scope_allows(plugin, id).is_some())
                        .collect();
                    if allowed.is_empty() {
                        continue;
                    }
                    if write_credential_response(
                        stream,
                        &CredentialResponse::Invalidated { ids: allowed },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
            }
        }
    }

    fn handle_request(&self, plugin: &str, request: CredentialRequest) -> CredentialResponse {
        match request {
            CredentialRequest::Ping => CredentialResponse::Pong,
            CredentialRequest::Resolve { id } => self.resolve(plugin, &id),
            CredentialRequest::RequestAuthorization { id } => {
                if self.scope_allows(plugin, &id).is_none() {
                    self.vault.record_audit(plugin, &id, false);
                    return Self::scope_denied(plugin, &id);
                }
                self.vault.record_audit(plugin, &id, true);
                // Stub until the OAuth flow implements the browser/redirect
                // exchange; the client treats this as "flow started".
                CredentialResponse::AuthorizationPending
            }
        }
    }

    /// Resolves the storage key for a requested id against the plugin's
    /// declared scope, or `None` when the id is undeclared or unparsable
    /// (fail-closed).
    fn scope_allows(&self, plugin: &str, id: &str) -> Option<String> {
        let id = CredentialId::try_new(id).ok()?;
        match self.registry.resolve_scope(plugin, &id) {
            ScopeDecision::Allowed { storage_key } => Some(storage_key),
            ScopeDecision::Undeclared => None,
        }
    }

    fn resolve(&self, plugin: &str, id: &str) -> CredentialResponse {
        let Some(storage_key) = self.scope_allows(plugin, id) else {
            self.vault.record_audit(plugin, id, false);
            return Self::scope_denied(plugin, id);
        };
        match self.vault.resolve(&storage_key) {
            Ok(store) => {
                self.vault.record_audit(plugin, id, true);
                if let Some(key) = store.api_key() {
                    return CredentialResponse::Resolved {
                        credential: ResolvedCredential::ApiKey {
                            key: WireSecret::new(key.to_owned()),
                        },
                    };
                }
                if let Some(token) = store.access_token() {
                    return CredentialResponse::Resolved {
                        credential: ResolvedCredential::Bearer {
                            token: WireSecret::new(token.to_owned()),
                            expires_at: store.expires_at(),
                        },
                    };
                }
                self.vault.record_audit(plugin, id, false);
                CredentialResponse::Error {
                    code: CredentialErrorCode::Missing {
                        label: id.to_string(),
                        help_url: None,
                    },
                    message: format!("credential '{id}' has no configured value"),
                }
            }
            Err(ConnectorError::CredentialMissing {
                id: _,
                label,
                help_url,
            }) => {
                // Audit the requested id (what the plugin asked for), which
                // differs from the storage key for private declarations.
                self.vault.record_audit(plugin, id, false);
                CredentialResponse::Error {
                    code: CredentialErrorCode::Missing { label, help_url },
                    message: format!("credential '{id}' is not configured"),
                }
            }
            Err(ConnectorError::RefreshRequired(_)) => {
                self.vault.record_audit(plugin, id, false);
                CredentialResponse::Error {
                    code: CredentialErrorCode::RefreshRequired,
                    message: format!("credential '{id}' expired and needs re-authorization"),
                }
            }
            Err(e) => {
                self.vault.record_audit(plugin, id, false);
                CredentialResponse::Error {
                    code: CredentialErrorCode::Internal,
                    message: e.to_string(),
                }
            }
        }
    }

    fn scope_denied(plugin: &str, id: &str) -> CredentialResponse {
        CredentialResponse::Error {
            code: CredentialErrorCode::ScopeDenied,
            message: format!("credential '{id}' is not declared for plugin '{plugin}'"),
        }
    }
}

#[async_trait]
impl HostServicePassenger for CredentialPassenger {
    async fn serve(&self, stream: IpcStream, token: String) {
        let mut stream = stream;
        // The passenger authenticates and writes the Open response itself
        // (see `ene-store`'s host_service routing comment): the store routes
        // the raw stream + token here without ever seeing the token map.
        let Some(reg) = self.registrations.get(&token).cloned() else {
            let (should_log, attempts) = {
                let mut tracker = self.failed_opens.lock();
                (tracker.record(), tracker.count())
            };
            if should_log {
                warn!(
                    component = "CredentialPassenger",
                    attempts, "Credential Open rejected: unknown token"
                );
            }
            drop(
                write_host_service_response(
                    &mut stream,
                    &HostServiceResponse::Error {
                        code: HostServiceErrorCode::AuthRejected,
                        message: "Invalid auth token".to_string(),
                    },
                )
                .await,
            );
            return;
        };
        // Subscribe before the OpenAck is observable: a client that has
        // confirmed the session must not miss an invalidation broadcast sent
        // between the OpenAck write and this task's subscription point.
        let invalidated_rx = self.invalidated_tx.subscribe();
        if let Err(e) =
            write_host_service_response(&mut stream, &HostServiceResponse::OpenAck).await
        {
            warn!(
                component = "CredentialPassenger",
                error = %e,
                "Credential service: failed to write OpenAck"
            );
            return;
        }
        self.serve_session(&mut stream, &reg.plugin, invalidated_rx)
            .await;
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "tests use expect/panic for concise failure messages"
)]
mod tests {
    use super::*;
    use ene_connector::CredentialStore;
    use ene_connector::vault::VaultEntry;
    use ene_plugin_proto::transport::IpcListener;
    use ene_plugin_proto::{
        read_credential_response, read_host_service_response, write_credential_request,
    };
    use serde_json::json;

    const SECRET: &str = "super-secret-api-key";

    /// Unique suffix per test: `#[tokio::test]` bodies run concurrently, and
    /// two listeners on the same socket path would collide at bind time.
    static SOCKET_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn test_socket_path(tag: &str) -> std::path::PathBuf {
        let n = SOCKET_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("ene-cred-{tag}-{}-{n}.sock", std::process::id()))
    }

    /// Registers the anthropic plugin's declarations (shared `anthropic`
    /// `api_key`) exactly as the manager would from its `config_schema()`.
    fn registry() -> Arc<CredentialRegistry> {
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "anthropic",
            Some(&json!({
                "x-ene-credentials": [{
                    "id": "anthropic",
                    "kind": "api_key",
                    "header": { "name": "x-api-key", "format": "{value}" }
                }]
            })),
        );
        Arc::new(registry)
    }

    fn vault() -> Arc<CredentialVault> {
        Arc::new(CredentialVault::new(vec![VaultEntry::new(
            "anthropic",
            CredentialStore::from_api_key(SECRET),
        )]))
    }

    fn registrations(token: &str) -> HashMap<String, CredentialPluginRegistration> {
        HashMap::from([(
            token.to_string(),
            CredentialPluginRegistration {
                plugin: "anthropic".to_string(),
            },
        )])
    }

    fn passenger(
        vault: Arc<CredentialVault>,
        registry: Arc<CredentialRegistry>,
        token: &str,
    ) -> Arc<CredentialPassenger> {
        Arc::new(CredentialPassenger::new(
            vault,
            registry,
            registrations(token),
        ))
    }

    /// Opens a credential session over a real socket pair, returning the
    /// client-side stream after the `OpenAck` is observed.
    async fn open_session(passenger: Arc<CredentialPassenger>, token: &str) -> IpcStream {
        let path = test_socket_path("session");
        let mut listener = IpcListener::bind(&path).expect("bind listener");
        let mut client = IpcStream::connect(&path).await.expect("connect");
        let server_stream = listener.accept().await.expect("accept");
        drop(listener);
        ene_plugin_proto::transport::cleanup_path(&path);
        let token = token.to_string();
        tokio::spawn({
            let passenger = Arc::clone(&passenger);
            async move {
                passenger.serve(server_stream, token).await;
            }
        });
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read open response")
            .expect("open frame");
        assert!(matches!(resp, HostServiceResponse::OpenAck));
        client
    }

    async fn send_request(client: &mut IpcStream, req: &CredentialRequest) {
        write_credential_request(client, req)
            .await
            .expect("write req");
    }

    async fn read_response(client: &mut IpcStream) -> CredentialResponse {
        read_credential_response(client)
            .await
            .expect("read response")
            .expect("response frame")
    }

    #[tokio::test]
    async fn open_with_valid_token_and_resolve() {
        let passenger = passenger(vault(), registry(), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "anthropic".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        match resp {
            CredentialResponse::Resolved {
                credential: ResolvedCredential::ApiKey { key },
            } => assert_eq!(key.expose(), SECRET),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn open_with_invalid_token_is_rejected() {
        let passenger = passenger(vault(), registry(), "ene-cred-good");
        let path = test_socket_path("reject");
        let mut listener = IpcListener::bind(&path).expect("bind listener");
        let mut client = IpcStream::connect(&path).await.expect("connect");
        let server_stream = listener.accept().await.expect("accept");
        drop(listener);
        ene_plugin_proto::transport::cleanup_path(&path);
        let bad_token = "ene-cred-bad".to_string();
        tokio::spawn({
            let passenger = Arc::clone(&passenger);
            async move {
                passenger.serve(server_stream, bad_token).await;
            }
        });
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read open error")
            .expect("open frame");
        assert!(matches!(
            resp,
            HostServiceResponse::Error {
                code: HostServiceErrorCode::AuthRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn undeclared_id_is_denied_and_audited() {
        let passenger = passenger(vault(), registry(), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "google.calendar".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        assert!(matches!(
            resp,
            CredentialResponse::Error {
                code: CredentialErrorCode::ScopeDenied,
                ..
            }
        ));
        let audit = passenger.vault.drain_audit();
        assert!(
            audit
                .iter()
                .any(|e| !e.allowed && e.id == "google.calendar"),
            "denial must be audited"
        );
    }

    #[tokio::test]
    async fn invalidated_broadcast_reaches_connected_client() {
        let passenger = passenger(vault(), registry(), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        passenger.broadcast_invalidated(vec!["anthropic".to_string()]);
        let resp = read_response(&mut client).await;
        assert_eq!(
            resp,
            CredentialResponse::Invalidated {
                ids: vec!["anthropic".to_string()],
            }
        );
    }

    #[tokio::test]
    async fn invalidated_outside_declared_scope_is_not_forwarded() {
        let passenger = passenger(vault(), registry(), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        passenger.broadcast_invalidated(vec!["google.calendar".to_string()]);
        // The client must NOT receive a frame for an undeclared id; a Ping
        // proves the connection is still live and no Invalidated was sent.
        send_request(&mut client, &CredentialRequest::Ping).await;
        let resp = read_response(&mut client).await;
        assert_eq!(resp, CredentialResponse::Pong);
    }

    #[tokio::test]
    async fn secret_never_reaches_error_message_or_audit() {
        // "missing-cred" is declared but has no vault entry: resolve must
        // reach the Missing path (not ScopeDenied) while keeping the secret
        // out of the error frame and the audit trail.
        let vault = CredentialVault::new(vec![VaultEntry::new(
            "anthropic",
            CredentialStore::from_api_key(SECRET),
        )]);
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "anthropic",
            Some(&json!({
                "x-ene-credentials": [
                    { "id": "anthropic", "kind": "api_key" },
                    { "id": "missing-cred", "kind": "api_key" }
                ]
            })),
        );
        let passenger = passenger(Arc::new(vault), Arc::new(registry), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "missing-cred".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        assert!(matches!(
            resp,
            CredentialResponse::Error {
                code: CredentialErrorCode::Missing { .. },
                ..
            }
        ));
        let all_frames = format!("{resp:?}");
        assert!(!all_frames.contains(SECRET));
        let audit = passenger.vault.drain_audit();
        assert!(
            !format!("{audit:?}").contains(SECRET),
            "audit must never carry the secret"
        );
    }

    #[tokio::test]
    async fn every_resolution_outcome_is_audited() {
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "anthropic",
            Some(&json!({
                "x-ene-credentials": [
                    { "id": "anthropic", "kind": "api_key" },
                    { "id": "missing-cred", "kind": "api_key" }
                ]
            })),
        );
        let passenger = passenger(vault(), Arc::new(registry), "ene-cred-good");
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;

        // ScopeDenied (undeclared).
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "google.calendar".into(),
            },
        )
        .await;
        read_response(&mut client).await;

        // Missing (declared but no entry).
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "missing-cred".into(),
            },
        )
        .await;
        read_response(&mut client).await;

        // Success.
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "anthropic".into(),
            },
        )
        .await;
        read_response(&mut client).await;

        let audit = passenger.vault.drain_audit();
        assert_eq!(audit.len(), 3, "every resolve outcome must be audited");
        assert!(
            audit
                .iter()
                .any(|e| !e.allowed && e.id == "google.calendar"),
            "ScopeDenied must be audited"
        );
        assert!(
            audit.iter().any(|e| !e.allowed && e.id == "missing-cred"),
            "Missing must be audited"
        );
        assert!(
            audit.iter().any(|e| e.allowed && e.id == "anthropic"),
            "success must be audited"
        );
    }

    #[test]
    fn resolved_frame_debug_redacts_secret() {
        let frame = CredentialResponse::Resolved {
            credential: ResolvedCredential::ApiKey {
                key: WireSecret::new(SECRET),
            },
        };
        let debug = format!("{frame:?}");
        assert!(!debug.contains(SECRET));
        assert!(debug.contains("<redacted>"));
    }
}
