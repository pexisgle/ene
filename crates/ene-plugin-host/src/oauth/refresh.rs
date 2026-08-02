//! Concrete [`TokenRefresher`]: single-flight OAuth refresh with a failure
//! cooldown.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use ene_connector::CredentialStore;
use ene_connector::declaration::CredentialKind;
use ene_connector::error::ConnectorError;
use ene_connector::vault::TokenRefresher;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

use crate::credential_registry::CredentialRegistry;
use crate::oauth::exchange;
use crate::oauth::persist::CredentialPersister;

/// After a failed refresh, further refresh attempts for the same key are
/// answered with [`ConnectorError::RefreshRequired`] for this long, so one
/// revoked refresh token cannot hammer the token endpoint.
const REFRESH_COOLDOWN: Duration = Duration::from_mins(1);

/// Outcome shared with the callers coalesced onto one refresh, made
/// clone-able because [`ConnectorError`] is not.
#[derive(Debug, Clone)]
enum RefreshFailure {
    /// The token cannot be refreshed; the user must re-authorize.
    RefreshRequired(String),
    /// The host cannot refresh this key (no declaration, wrong kind).
    Internal(String),
}

impl RefreshFailure {
    fn into_connector(self) -> ConnectorError {
        match self {
            Self::RefreshRequired(id) => ConnectorError::refresh_required(id),
            Self::Internal(message) => ConnectorError::internal(message),
        }
    }
}

/// Shared in-flight refresh slot: one HTTP refresh per storage key, with all
/// concurrent callers for that key awaiting the same result.
type RefreshSlot = Arc<AsyncMutex<Option<Result<CredentialStore, RefreshFailure>>>>;

/// Refreshes `OAuth2` tokens through each credential's declared token
/// endpoint, coalescing concurrent refreshes for the same key onto one HTTP
/// call and persisting rotation.
pub struct OAuthRefresher {
    client: reqwest::Client,
    registry: Arc<CredentialRegistry>,
    persister: Arc<dyn CredentialPersister>,
    /// storage key → in-flight refresh slot (single-flight).
    in_flight: AsyncMutex<HashMap<String, RefreshSlot>>,
    /// storage key → time until which refreshes are refused after a failure.
    cooldown: Mutex<HashMap<String, Instant>>,
}

impl OAuthRefresher {
    /// Creates a refresher resolving declarations through `registry` and
    /// persisting rotation through `persister`.
    #[must_use]
    pub fn new(registry: Arc<CredentialRegistry>, persister: Arc<dyn CredentialPersister>) -> Self {
        Self {
            client: reqwest::Client::new(),
            registry,
            persister,
            in_flight: AsyncMutex::new(HashMap::new()),
            cooldown: Mutex::new(HashMap::new()),
        }
    }

    fn in_cooldown(&self, id: &str) -> bool {
        let mut guard = self.cooldown.lock();
        match guard.get(id) {
            Some(deadline) if *deadline > Instant::now() => true,
            Some(_) => {
                guard.remove(id);
                false
            }
            None => false,
        }
    }

    async fn refresh_inner(
        &self,
        id: &str,
        current: &CredentialStore,
    ) -> Result<CredentialStore, RefreshFailure> {
        let Some(refresh_token) = current.refresh_token() else {
            // No refresh token issued: the only way forward is a fresh
            // authorization, so the credential is marked refresh-required.
            return Err(RefreshFailure::RefreshRequired(id.to_string()));
        };
        let declaration = self
            .registry
            .declaration_for_storage_key(id)
            .ok_or_else(|| {
                RefreshFailure::Internal(format!("no credential declaration for '{id}'"))
            })?;
        let CredentialKind::OAuth2 {
            client_id,
            token_url,
            ..
        } = &declaration.kind
        else {
            return Err(RefreshFailure::Internal(format!(
                "credential '{id}' is not an OAuth2 credential"
            )));
        };
        let response = exchange::refresh_token(&self.client, token_url, client_id, refresh_token)
            .await
            .map_err(|_| RefreshFailure::RefreshRequired(id.to_string()))?;
        // Token rotation: the server replaces the refresh token when it
        // issues a new one; otherwise the current one stays valid.
        let rotated = response
            .refresh_token
            .or_else(|| current.refresh_token().map(str::to_owned));
        let expires_at = response
            .expires_in
            .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
        let store = CredentialStore::oauth2(response.access_token, rotated, expires_at);
        // Persisting rotation is best-effort: the vault holds the fresh
        // value for this process either way, and a restart reads whatever
        // was last written.
        if let Err(e) = self.persist(id, &store) {
            tracing::warn!(
                component = "OAuthRefresher",
                credential_id = %id,
                error = %e,
                "Failed to persist a refreshed credential"
            );
        }
        Ok(store)
    }

    fn persist(&self, id: &str, store: &CredentialStore) -> Result<(), crate::oauth::FlowError> {
        let mut entries = self.persister.load();
        entries.insert(id.to_string(), store.expose_for_persistence());
        self.persister.save(&entries)?;
        Ok(())
    }
}

#[async_trait]
impl TokenRefresher for OAuthRefresher {
    async fn refresh(
        &self,
        id: &str,
        current: &CredentialStore,
    ) -> Result<CredentialStore, ConnectorError> {
        if self.in_cooldown(id) {
            return Err(ConnectorError::refresh_required(id));
        }
        let slot = {
            let mut in_flight = self.in_flight.lock().await;
            in_flight
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(None)))
                .clone()
        };
        let mut guard = slot.lock().await;
        if let Some(result) = guard.as_ref() {
            return match result {
                Ok(store) => Ok(store.clone()),
                Err(failure) => Err(failure.clone().into_connector()),
            };
        }
        let result = self.refresh_inner(id, current).await;
        if result.is_err() {
            self.cooldown
                .lock()
                .insert(id.to_string(), Instant::now() + REFRESH_COOLDOWN);
        } else {
            self.cooldown.lock().remove(id);
        }
        *guard = Some(result.clone());
        drop(guard);
        self.in_flight.lock().await.remove(id);
        match result {
            Ok(store) => Ok(store),
            Err(failure) => Err(failure.into_connector()),
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;
    use ene_connector::CredentialStore;
    use serde_json::json;

    /// Runs one refresh call for a credential whose declaration points at a
    /// mock token endpoint, returning the number of hits on that endpoint.
    async fn refresh_count_hits(
        persisted_token: &str,
    ) -> (Result<CredentialStore, ConnectorError>, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_server = Arc::clone(&hits);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            // Serve up to three sequential refresh POSTs.
            for _ in 0..3 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                hits_for_server.fetch_add(1, Ordering::SeqCst);
                let mut buf = Vec::new();
                let mut chunk = [0_u8; 2048];
                loop {
                    match socket.read(&mut chunk).await.unwrap() {
                        0 => break,
                        n => buf.extend_from_slice(&chunk[..n]),
                    }
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = r#"{"access_token":"refreshed-at","refresh_token":"rotated-rt","expires_in":3600}"#;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
                socket.flush().await.unwrap();
            }
        });

        let registry = Arc::new(CredentialRegistry::new());
        registry.register_from_schema(
            "mock",
            Some(&json!({
                "x-ene-credentials": [{
                    "id": "google.calendar",
                    "kind": "oauth2",
                    "client_id": "client-id",
                    "auth_url": "https://auth.example.com",
                    "token_url": format!("http://{addr}/token")
                }]
            })),
        );
        let dir = tempfile::tempdir().unwrap();
        let persister = Arc::new(crate::oauth::persist::FileCredentialPersister::new(
            dir.path().join("credentials.json"),
        ));
        let refresher = OAuthRefresher::new(registry, persister);
        let store = CredentialStore::oauth2("expired-at", Some(persisted_token), None);
        let result = refresher.refresh("google.calendar", &store).await;
        // The server task parks on accept for its remaining iterations; drop
        // it so the test does not hang on the join.
        server.abort();
        (result, hits.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn refresh_rotates_and_persists() {
        let (result, _) = refresh_count_hits("rt-1").await;
        let store = result.unwrap();
        assert_eq!(store.access_token(), Some("refreshed-at"));
        assert_eq!(store.refresh_token(), Some("rotated-rt"));
        assert!(store.expires_at().is_some());
    }

    #[tokio::test]
    async fn concurrent_refreshes_coalesce_to_one_call() {
        // Two concurrent refreshes for the same key must hit the token
        // endpoint once; the second shares the first's result.
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits_for_server = Arc::clone(&hits);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            hits_for_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                match socket.read(&mut chunk).await.unwrap() {
                    0 => break,
                    n => buf.extend_from_slice(&chunk[..n]),
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"access_token":"refreshed-at","expires_in":3600}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
        });
        let registry = Arc::new(CredentialRegistry::new());
        registry.register_from_schema(
            "mock",
            Some(&json!({
                "x-ene-credentials": [{
                    "id": "google.calendar",
                    "kind": "oauth2",
                    "client_id": "client-id",
                    "auth_url": "https://auth.example.com",
                    "token_url": format!("http://{addr}/token")
                }]
            })),
        );
        let dir = tempfile::tempdir().unwrap();
        let persister = Arc::new(crate::oauth::persist::FileCredentialPersister::new(
            dir.path().join("credentials.json"),
        ));
        let refresher = Arc::new(OAuthRefresher::new(registry, persister));
        let store = CredentialStore::oauth2("expired-at", Some("rt-1"), None);
        let a = {
            let refresher = Arc::clone(&refresher);
            tokio::spawn(async move { refresher.refresh("google.calendar", &store).await })
        };
        let b = {
            let refresher = Arc::clone(&refresher);
            tokio::spawn(async move { refresher.refresh("google.calendar", &store).await })
        };
        let (ra, rb) = tokio::join!(a, b);
        server.await.unwrap();
        assert_eq!(ra.unwrap().unwrap().access_token(), Some("refreshed-at"));
        assert_eq!(rb.unwrap().unwrap().access_token(), Some("refreshed-at"));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
