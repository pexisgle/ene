//! In-memory credential vault and audit trail.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::credential::CredentialStore;
use crate::error::ConnectorError;

/// Capacity of the in-memory audit ring (most recent entries only).
const AUDIT_CAPACITY: usize = 1024;

/// How long before an `OAuth2` access token lapses [`CredentialVault::resolve`]
/// starts a refresh, so a request never trips over an expiring token.
pub const REFRESH_LEAD_TIME: Duration = Duration::from_secs(60);

/// One credential held by the vault.
#[derive(Debug, Clone)]
pub struct VaultEntry {
    /// Storage key as resolved by the plugin's credential declaration
    /// (shared declarations key on the credential id, private ones on
    /// `<plugin>:<id>`). The vault treats it as an opaque key: it is
    /// produced by [`resolve_scope`](crate::declaration::resolve_scope) and
    /// consumed verbatim by [`CredentialVault::resolve`].
    pub id: String,
    /// The stored credential.
    pub credential: CredentialStore,
}

impl VaultEntry {
    /// Creates a vault entry.
    #[must_use]
    pub fn new(id: impl Into<String>, credential: CredentialStore) -> Self {
        Self {
            id: id.into(),
            credential,
        }
    }
}

/// One resolved/denied credential request, recorded for audit.
///
/// Carries only the id and the outcome — never the secret value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    /// When the request was handled.
    pub ts: DateTime<Utc>,
    /// Plugin that made the request (derived from its auth token).
    pub plugin: String,
    /// Requested credential id.
    pub id: String,
    /// Whether the request was permitted and resolved.
    pub allowed: bool,
}

/// Bounded in-memory audit trail.
///
/// Records the plugin, id, and outcome of every credential request and
/// mirrors it as a structured [`tracing`] event. Persistence to the DB
/// `audit_log` table is a later scope; [`CredentialAuditLog::drain`] gives
/// that backend a readout.
#[derive(Default)]
pub struct CredentialAuditLog {
    entries: RwLock<VecDeque<AuditEntry>>,
}

impl CredentialAuditLog {
    /// Creates an empty audit log.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(AUDIT_CAPACITY)),
        }
    }

    /// Records one credential request. Never stores the secret value.
    pub fn record(&self, plugin: &str, id: &str, allowed: bool) {
        let entry = AuditEntry {
            ts: Utc::now(),
            plugin: plugin.to_string(),
            id: id.to_string(),
            allowed,
        };
        let mut guard = self.entries.write();
        if guard.len() == AUDIT_CAPACITY {
            guard.pop_front();
        }
        guard.push_back(entry);
        drop(guard);
        tracing::info!(
            plugin = %plugin,
            id = %id,
            allowed,
            "Credential access audited"
        );
    }

    /// Returns and clears the recorded entries (for a persistence backend).
    #[must_use]
    pub fn drain(&self) -> Vec<AuditEntry> {
        let mut guard = self.entries.write();
        guard.drain(..).collect()
    }

    /// Number of entries currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Extension point for automatic OAuth refresh.
///
/// Until a refresher is installed, expired credentials resolve to
/// [`ConnectorError::RefreshRequired`]. The concrete refresher (browser
/// flow, token exchange) is installed by the host behind this seam.
#[async_trait]
pub trait TokenRefresher: Send + Sync {
    /// Returns a refreshed credential for `id`, or an error.
    ///
    /// The current credential is passed so the impl can read the refresh
    /// token without reaching into the vault. Implementations must not hold
    /// the vault's locks across an await point.
    async fn refresh(
        &self,
        id: &str,
        current: &CredentialStore,
    ) -> Result<CredentialStore, ConnectorError>;
}

/// Kind of a stored credential, for non-secret list UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSummaryKind {
    /// An `OAuth2` token set.
    OAuth2,
    /// A bare API key.
    ApiKey,
    /// No credential.
    None,
}

/// Non-secret readout of one vault entry for list UIs.
///
/// Carries only the storage key, kind, and expiry — never a secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VaultEntrySummary {
    /// Storage key the entry lives under.
    pub id: String,
    /// Credential kind.
    pub kind: CredentialSummaryKind,
    /// Access-token expiry, when known.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the credential is currently past its expiry.
    pub expired: bool,
}

/// The host's in-memory credential vault.
///
/// A snapshot built from configuration and the credential persistence file
/// at startup: entries keyed by the storage key each plugin's declaration
/// resolves to (see [`VaultEntry`]). Scope enforcement is **not** the
/// vault's job — the credential passenger asks the declaration registry
/// ([`resolve_scope`](crate::declaration::resolve_scope)) and only hands the
/// vault the resolved storage key. Live updates land via
/// [`CredentialVault::store`] (OAuth flow completion, token refresh) and
/// [`CredentialVault::invalidate`] (revocation).
pub struct CredentialVault {
    entries: RwLock<HashMap<String, CredentialStore>>,
    /// Storage key → non-secret setup guidance shown when the entry is
    /// missing (which configuration path to fill in). Populated by the
    /// runtime's config-driven vault builder; the vault only stores and
    /// re-surfaces the text.
    hints: RwLock<HashMap<String, String>>,
    refresher: Option<Arc<dyn TokenRefresher>>,
    audit: CredentialAuditLog,
}

impl CredentialVault {
    /// Builds a vault from startup configuration.
    #[must_use]
    pub fn new(entries: Vec<VaultEntry>) -> Self {
        Self {
            entries: RwLock::new(
                entries
                    .into_iter()
                    .map(|entry| (entry.id, entry.credential))
                    .collect(),
            ),
            hints: RwLock::new(HashMap::new()),
            refresher: None,
            audit: CredentialAuditLog::new(),
        }
    }

    /// Installs an automatic-refresh backend.
    #[must_use]
    pub fn with_refresher(mut self, refresher: Arc<dyn TokenRefresher>) -> Self {
        self.refresher = Some(refresher);
        self
    }

    /// Records non-secret setup guidance for `storage_key`, surfaced as the
    /// missing-credential label so an operator knows which configuration
    /// path to fill in.
    pub fn set_hint(&self, storage_key: &str, hint: String) {
        self.hints.write().insert(storage_key.to_string(), hint);
    }

    /// Stores or replaces the credential for `storage_key`.
    ///
    /// Used by the OAuth flow and the token refresher to write freshly
    /// obtained credentials without rebuilding the vault. Never rejects.
    pub fn store(&self, storage_key: &str, credential: CredentialStore) {
        self.entries
            .write()
            .insert(storage_key.to_string(), credential);
    }

    /// Resolves a credential by storage key, refreshing it when it is
    /// expired (or within [`REFRESH_LEAD_TIME`] of expiry) and a refresher
    /// is installed, or failing with [`ConnectorError::RefreshRequired`].
    ///
    /// `storage_key` is the key produced by the caller's scope resolution
    /// ([`resolve_scope`](crate::declaration::resolve_scope)), not the id the
    /// plugin asked for — the two differ for private declarations.
    ///
    /// Lock-ordering invariant: this method never holds a vault lock across
    /// an `.await`. The entry is cloned under a read lock, the lock is
    /// released, the refresher runs, and only then is the result written
    /// back under a write lock. A concurrent `store`/`invalidate` during the
    /// refresh therefore wins instead of deadlocking or blocking the refresh
    /// — and the refresh write-back may overwrite a concurrently stored
    /// fresher value, which the single-flight refresher avoids in practice.
    pub async fn resolve(&self, storage_key: &str) -> Result<CredentialStore, ConnectorError> {
        let current = {
            let entries = self.entries.read();
            match entries.get(storage_key).cloned() {
                Some(store) => store,
                None => {
                    let label = self
                        .hints
                        .read()
                        .get(storage_key)
                        .cloned()
                        .unwrap_or_else(|| storage_key.to_string());
                    return Err(ConnectorError::credential_missing(storage_key, label, None));
                }
            }
        };
        if !current.is_expired() && !current.expires_within(REFRESH_LEAD_TIME) {
            return Ok(current);
        }
        let Some(refresher) = &self.refresher else {
            // No refresher installed: only a *lapsed* credential is unusable;
            // one merely inside the lead window still serves.
            return if current.is_expired() {
                Err(ConnectorError::refresh_required(storage_key))
            } else {
                Ok(current)
            };
        };
        let fresh = refresher.refresh(storage_key, &current).await?;
        self.entries
            .write()
            .insert(storage_key.to_string(), fresh.clone());
        Ok(fresh)
    }

    /// Drops credentials from the vault on update/revocation.
    ///
    /// Clients are told to drop their cached copies via the invalidation
    /// notice; the entry is removed so a later resolve re-reads
    /// configuration or triggers refresh. Not the production invalidation
    /// path — the credential passenger's
    /// `replace_vault_and_broadcast` is — but kept for direct tests.
    pub fn invalidate(&self, ids: &[String]) {
        let mut guard = self.entries.write();
        for id in ids {
            guard.remove(id);
        }
    }

    /// Returns the storage keys currently held, for invalidation notices
    /// after a vault swap.
    #[must_use]
    pub fn storage_keys(&self) -> Vec<String> {
        self.entries.read().keys().cloned().collect()
    }

    /// Lists every stored entry as a non-secret summary.
    ///
    /// Deliberately carries no secret material: the list feeds settings UIs
    /// and management commands, never the wire.
    #[must_use]
    pub fn list(&self) -> Vec<VaultEntrySummary> {
        let mut summaries: Vec<VaultEntrySummary> = self
            .entries
            .read()
            .iter()
            .map(|(id, store)| VaultEntrySummary {
                id: id.clone(),
                kind: match store {
                    CredentialStore::OAuth2 { .. } => CredentialSummaryKind::OAuth2,
                    CredentialStore::ApiKey(_) => CredentialSummaryKind::ApiKey,
                    CredentialStore::None => CredentialSummaryKind::None,
                },
                expires_at: store.expires_at(),
                expired: store.is_expired(),
            })
            .collect();
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        summaries
    }

    /// Records one credential request in the audit trail.
    pub fn record_audit(&self, plugin: &str, id: &str, allowed: bool) {
        self.audit.record(plugin, id, allowed);
    }

    /// Readout for a future persistence backend (clears the ring).
    #[must_use]
    pub fn drain_audit(&self) -> Vec<AuditEntry> {
        self.audit.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::CredentialStore;

    fn vault_with(entries: &[(&str, &str)]) -> CredentialVault {
        let entries = entries
            .iter()
            .map(|(id, key)| VaultEntry::new(*id, CredentialStore::from_api_key(*key)))
            .collect();
        CredentialVault::new(entries)
    }

    /// Records whether a mock refresher was called, without any secret
    /// material, so tests can assert the call happened.
    #[derive(Default)]
    struct CountingRefresher {
        calls: parking_lot::Mutex<usize>,
        /// When set, the refresher returns this error instead of a token.
        fail: Option<ConnectorError>,
        refresh_token: Option<String>,
    }

    #[async_trait]
    impl TokenRefresher for CountingRefresher {
        async fn refresh(
            &self,
            _id: &str,
            current: &CredentialStore,
        ) -> Result<CredentialStore, ConnectorError> {
            *self.calls.lock() += 1;
            if let Some(err) = &self.fail {
                return Err(err.clone());
            }
            let fresh = current.refresh_token().map(str::to_owned);
            Ok(CredentialStore::oauth2(
                "fresh-token",
                fresh,
                Some(Utc::now() + chrono::Duration::hours(1)),
            ))
        }
    }

    #[tokio::test]
    async fn resolve_returns_cloned_credential() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        let store = vault.resolve("anthropic").await.unwrap();
        assert_eq!(store.api_key(), Some("sk-test"));
    }

    #[tokio::test]
    async fn resolve_missing_credential_reports_missing_without_secret() {
        let vault = vault_with(&[("anthropic", "sk-super-secret")]);
        let err = vault.resolve("nope").await.unwrap_err();
        let message = err.to_string();
        assert!(!message.contains("sk-super-secret"));
        assert!(message.contains("nope"));
    }

    #[tokio::test]
    async fn invalidate_drops_entries() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        vault.invalidate(&["anthropic".to_string()]);
        assert!(vault.resolve("anthropic").await.is_err());
    }

    #[tokio::test]
    async fn store_inserts_and_replaces_entries() {
        let vault = vault_with(&[]);
        let oauth = CredentialStore::oauth2("access", Some("refresh"), None);
        vault.store("google.calendar", oauth.clone());
        let resolved = vault.resolve("google.calendar").await.unwrap();
        assert_eq!(resolved.access_token(), Some("access"));
        vault.store("google.calendar", CredentialStore::from_api_key("key"));
        let resolved = vault.resolve("google.calendar").await.unwrap();
        assert_eq!(resolved.api_key(), Some("key"));
    }

    #[tokio::test]
    async fn expired_oauth_resolves_to_refresh_required_without_refresher() {
        use chrono::Duration;
        let past = Utc::now() - Duration::hours(1);
        let store = CredentialStore::oauth2("access", None::<&str>, Some(past));
        let vault = CredentialVault::new(vec![VaultEntry::new("google.calendar", store)]);
        let err = vault.resolve("google.calendar").await.unwrap_err();
        assert!(matches!(err, ConnectorError::RefreshRequired(_)));
    }

    #[tokio::test]
    async fn refresh_runs_for_expired_credential_and_writes_back() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let store = CredentialStore::oauth2("expired", Some("refresh-tok"), Some(past));
        let refresher = Arc::new(CountingRefresher::default());
        let vault = CredentialVault::new(vec![VaultEntry::new("google.calendar", store)])
            .with_refresher(Arc::clone(&refresher));
        let resolved = vault.resolve("google.calendar").await.unwrap();
        assert_eq!(resolved.access_token(), Some("fresh-token"));
        assert_eq!(*refresher.calls.lock(), 1);
        // The write-back is visible to the next resolve without a refresh.
        assert_eq!(
            vault
                .resolve("google.calendar")
                .await
                .unwrap()
                .access_token(),
            Some("fresh-token")
        );
        assert_eq!(*refresher.calls.lock(), 1);
    }

    #[tokio::test]
    async fn refresh_runs_within_lead_time_before_expiry() {
        let soon = Utc::now() + chrono::Duration::seconds(30);
        let store = CredentialStore::oauth2("about-to-expire", Some("refresh-tok"), Some(soon));
        let refresher = Arc::new(CountingRefresher::default());
        let vault = CredentialVault::new(vec![VaultEntry::new("google.calendar", store)])
            .with_refresher(Arc::clone(&refresher));
        let resolved = vault.resolve("google.calendar").await.unwrap();
        assert_eq!(resolved.access_token(), Some("fresh-token"));
        assert_eq!(*refresher.calls.lock(), 1);
    }

    #[tokio::test]
    async fn refresh_failure_returns_refresh_required_and_keeps_entry() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let store = CredentialStore::oauth2("expired", Some("refresh-tok"), Some(past));
        let refresher = Arc::new(CountingRefresher {
            fail: Some(ConnectorError::refresh_required("google.calendar")),
            ..Default::default()
        });
        let vault = CredentialVault::new(vec![VaultEntry::new("google.calendar", store)])
            .with_refresher(Arc::clone(&refresher));
        let err = vault.resolve("google.calendar").await.unwrap_err();
        assert!(matches!(err, ConnectorError::RefreshRequired(_)));
        // The entry survives so a later successful refresh can replace it.
        let resolved = vault.resolve("google.calendar").await.unwrap_err();
        assert!(matches!(resolved, ConnectorError::RefreshRequired(_)));
    }

    #[tokio::test]
    async fn list_never_exposes_secrets() {
        let vault = vault_with(&[("anthropic", "sk-super-secret")]);
        vault.store(
            "google.calendar",
            CredentialStore::oauth2("access-secret", Some("refresh-secret"), None),
        );
        let summaries = vault.list();
        assert_eq!(summaries.len(), 2);
        let serialized = serde_json::to_string(&summaries).unwrap();
        assert!(!serialized.contains("sk-super-secret"));
        assert!(!serialized.contains("access-secret"));
        assert!(!serialized.contains("refresh-secret"));
        assert!(summaries.iter().any(|s| s.id == "anthropic"));
        assert!(summaries.iter().any(|s| s.id == "google.calendar"));
    }

    #[tokio::test]
    async fn concurrent_store_during_refresh_is_not_blocked() {
        // The lock-ordering invariant: a `store` issued while a refresh is
        // awaiting must complete instead of waiting on the refresh's lock.
        // The mock parks inside `refresh` until the store has run.
        #[derive(Default)]
        struct ParkingRefresher {
            started: tokio::sync::oneshot::Sender<()>,
            resume: tokio::sync::oneshot::Receiver<()>,
        }
        #[async_trait]
        impl TokenRefresher for ParkingRefresher {
            async fn refresh(
                &self,
                _id: &str,
                _current: &CredentialStore,
            ) -> Result<CredentialStore, ConnectorError> {
                let _ = self.started.send(());
                let _ = self.resume.await;
                Ok(CredentialStore::oauth2(
                    "fresh-token",
                    None::<&str>,
                    Some(Utc::now() + chrono::Duration::hours(1)),
                ))
            }
        }

        let past = Utc::now() - chrono::Duration::hours(1);
        let store = CredentialStore::oauth2("expired", Some("refresh-tok"), Some(past));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let refresher = Arc::new(ParkingRefresher {
            started: started_tx,
            resume: resume_rx,
        });
        let vault = Arc::new(
            CredentialVault::new(vec![VaultEntry::new("google.calendar", store)])
                .with_refresher(Arc::clone(&refresher)),
        );
        let vault_for_refresh = Arc::clone(&vault);
        let refresh_task =
            tokio::spawn(async move { vault_for_refresh.resolve("google.calendar").await });
        started_rx.await.unwrap();
        // The store completes while the refresh is parked on `resume`.
        vault.store(
            "google.calendar",
            CredentialStore::from_api_key("concurrent-key"),
        );
        let _ = resume_tx.send(());
        let resolved = refresh_task.await.unwrap().unwrap();
        assert_eq!(resolved.access_token(), Some("fresh-token"));
    }

    #[tokio::test]
    async fn audit_records_id_and_outcome_only() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        vault.record_audit("anthropic", "anthropic", true);
        vault.record_audit("anthropic", "nope", false);
        let entries = vault.drain_audit();
        assert_eq!(entries.len(), 2);
        let serialized = serde_json::to_string(&entries).unwrap();
        assert!(!serialized.contains("sk-test"));
        assert!(entries.iter().any(|e| e.allowed));
        assert!(entries.iter().any(|e| !e.allowed));
    }

    #[tokio::test]
    async fn audit_ring_is_bounded() {
        let vault = vault_with(&[]);
        for i in 0..(AUDIT_CAPACITY + 50) {
            vault.record_audit("plugin", &format!("id-{i}"), true);
        }
        assert_eq!(vault.drain_audit().len(), AUDIT_CAPACITY);
    }

    #[tokio::test]
    async fn storage_key_is_opaque() {
        // Private-declaration storage keys are `<plugin>:<id>` and must be
        // addressable verbatim: the vault never splits or re-derives them.
        let vault = vault_with(&[("my-plugin:anthropic", "sk-test")]);
        let store = vault.resolve("my-plugin:anthropic").await.unwrap();
        assert_eq!(store.api_key(), Some("sk-test"));
    }

    #[tokio::test]
    async fn missing_credential_surfaces_setup_hint() {
        let vault = vault_with(&[]);
        vault.set_hint(
            "anthropic",
            "set ai.providers.myanth.api_key (kind: anthropic)".to_string(),
        );
        let err = vault.resolve("anthropic").await.unwrap_err();
        assert!(
            matches!(
                &err,
                ConnectorError::CredentialMissing { label, .. }
                    if label.contains("ai.providers.myanth.api_key")
            ),
            "expected a guided missing error, got {err:?}"
        );
    }
}
