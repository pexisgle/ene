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
    /// Credential id as plugins request it (a plugin name such as `anthropic`
    /// or a `namespace.name` connector id such as `google.calendar`).
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
/// A snapshot built from configuration at startup: entries keyed by
/// credential id plus each plugin's declared scope (provisionally the
/// `x-ene-credentials` entry key). Persistence / keychain backing and live
/// updates land with the OAuth flow; until then a process restart re-reads
/// configuration.
pub struct CredentialVault {
    entries: RwLock<HashMap<String, CredentialStore>>,
    declared: RwLock<HashMap<String, Vec<String>>>,
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
            declared: RwLock::new(HashMap::new()),
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

    /// Registers the set of credential ids a plugin is allowed to request.
    ///
    /// Callers derive `ids` from the plugin's declared scope; a plugin with
    /// no declaration is allowed nothing (fail-closed).
    pub fn declare(&self, plugin: &str, ids: Vec<String>) {
        self.declared.write().insert(plugin.to_string(), ids);
    }

    /// All credential ids declared for `plugin` (empty when undeclared).
    #[must_use]
    pub fn declared_ids(&self, plugin: &str) -> Vec<String> {
        self.declared
            .read()
            .get(plugin)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns `true` when `id` is inside `plugin`'s declared scope.
    #[must_use]
    pub fn is_allowed(&self, plugin: &str, id: &str) -> bool {
        self.declared
            .read()
            .get(plugin)
            .is_some_and(|ids| ids.iter().any(|candidate| candidate == id))
    }

    /// Resolves a credential, refreshing it when expired and a refresher is
    /// installed, or failing with [`ConnectorError::RefreshRequired`].
    pub fn resolve(&self, id: &str) -> Result<CredentialStore, ConnectorError> {
        let Some(store) = self.entries.read().get(id).cloned() else {
            return Err(ConnectorError::credential_missing(id, id, None));
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
    fn scope_matching_is_fail_closed() {
        let vault = vault_with(&[("anthropic", "sk-test")]);
        vault.declare("anthropic", vec!["anthropic".to_string()]);
        assert!(vault.is_allowed("anthropic", "anthropic"));
        assert!(!vault.is_allowed("anthropic", "google.calendar"));
        // Undeclared plugin is allowed nothing.
        assert!(!vault.is_allowed("fs", "anthropic"));
        assert!(
            vault
                .declared_ids("anthropic")
                .contains(&"anthropic".to_string())
        );
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
        vault.declare("anthropic", vec!["anthropic".to_string()]);
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
}
