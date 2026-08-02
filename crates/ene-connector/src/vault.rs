//! In-memory credential vault and audit trail.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::credential::CredentialStore;
use crate::error::ConnectorError;

/// Capacity of the in-memory audit ring (most recent entries only).
const AUDIT_CAPACITY: usize = 1024;

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

/// Extension point for automatic OAuth refresh (a later issue).
///
/// Until a refresher is installed, expired credentials resolve to
/// [`ConnectorError::RefreshRequired`]. The follow-up installs a concrete
/// refresher (browser flow, token exchange) behind this seam.
pub trait TokenRefresher: Send + Sync {
    /// Returns a refreshed credential for `id`, or an error.
    fn refresh(
        &self,
        id: &str,
        current: &CredentialStore,
    ) -> Result<CredentialStore, ConnectorError>;
}

/// The host's in-memory credential vault.
///
/// A snapshot built from configuration at startup: entries keyed by the
/// storage key each plugin's declaration resolves to (see
/// [`VaultEntry`]). Scope enforcement is **not** the vault's job — the
/// credential passenger asks the declaration registry
/// ([`resolve_scope`](crate::declaration::resolve_scope)) and only hands the
/// vault the resolved storage key. Persistence / keychain backing and live
/// updates land with the OAuth flow; until then a process restart re-reads
/// configuration.
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

    /// Installs an automatic-refresh backend (OAuth flow follow-up).
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

    /// Resolves a credential by storage key, refreshing it when expired and a
    /// refresher is installed, or failing with
    /// [`ConnectorError::RefreshRequired`].
    ///
    /// `storage_key` is the key produced by the caller's scope resolution
    /// ([`resolve_scope`](crate::declaration::resolve_scope)), not the id the
    /// plugin asked for — the two differ for private declarations.
    pub fn resolve(&self, storage_key: &str) -> Result<CredentialStore, ConnectorError> {
        let Some(store) = self.entries.read().get(storage_key).cloned() else {
            let label = self
                .hints
                .read()
                .get(storage_key)
                .cloned()
                .unwrap_or_else(|| storage_key.to_string());
            return Err(ConnectorError::credential_missing(storage_key, label, None));
        };
        if store.is_expired() {
            let Some(refresher) = &self.refresher else {
                return Err(ConnectorError::refresh_required(id));
            };
            let fresh = refresher.refresh(id, &store)?;
            self.entries.write().insert(id.to_string(), fresh.clone());
            return Ok(fresh);
        }
        Ok(store)
    }

    /// Drops credentials from the vault on update/revocation.
    ///
    /// Clients are told to drop their cached copies via the invalidation
    /// notice; the entry is removed so a later resolve re-reads
    /// configuration or triggers refresh.
    pub fn invalidate(&self, ids: &[String]) {
        let mut guard = self.entries.write();
        for id in ids {
            guard.remove(id);
        }
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

    #[test]
    fn resolve_returns_cloned_credential() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        let store = vault.resolve("anthropic").unwrap();
        assert_eq!(store.api_key(), Some("sk-test"));
    }

    #[test]
    fn resolve_missing_credential_reports_missing_without_secret() {
        let vault = vault_with(&[("anthropic", "sk-super-secret")]);
        let err = vault.resolve("nope").unwrap_err();
        let message = err.to_string();
        assert!(!message.contains("sk-super-secret"));
        assert!(message.contains("nope"));
    }

    #[test]
    fn invalidate_drops_entries() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        vault.invalidate(&["anthropic".to_string()]);
        assert!(vault.resolve("anthropic").is_err());
    }

    #[test]
    fn expired_oauth_resolves_to_refresh_required_without_refresher() {
        use chrono::Duration;
        let past = Utc::now() - Duration::hours(1);
        let store = CredentialStore::oauth2("access", None::<&str>, Some(past));
        let vault = CredentialVault::new(vec![VaultEntry::new("google.calendar", store)]);
        let err = vault.resolve("google.calendar").unwrap_err();
        assert!(matches!(err, ConnectorError::RefreshRequired(_)));
    }

    #[test]
    fn audit_records_id_and_outcome_only() {
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

    #[test]
    fn audit_ring_is_bounded() {
        let vault = vault_with(&[]);
        for i in 0..(AUDIT_CAPACITY + 50) {
            vault.record_audit("plugin", &format!("id-{i}"), true);
        }
        assert_eq!(vault.drain_audit().len(), AUDIT_CAPACITY);
    }

    #[test]
    fn storage_key_is_opaque() {
        // Private-declaration storage keys are `<plugin>:<id>` and must be
        // addressable verbatim: the vault never splits or re-derives them.
        let vault = vault_with(&[("my-plugin:anthropic", "sk-test")]);
        let store = vault.resolve("my-plugin:anthropic").unwrap();
        assert_eq!(store.api_key(), Some("sk-test"));
    }

    #[test]
    fn missing_credential_surfaces_setup_hint() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        vault.set_hint(
            "anthropic",
            "set ai.providers.myanth.api_key (kind: anthropic)".to_string(),
        );
        let err = vault.resolve("anthropic").unwrap_err();
        match err {
            ConnectorError::CredentialMissing { label, .. } => {
                assert!(label.contains("ai.providers.myanth.api_key"));
            }
            other => panic!("expected missing, got {other:?}"),
        }
    }
}
