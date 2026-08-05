//! Connector registry: registration, lifecycle operations, permission routing.
//!
//! The registry is the common API the acceptance criteria target: register,
//! connectivity check, connect, disconnect, and permission-status display all
//! flow through it. It adds the framework guarantees around a bare
//! [`Connector`] implementation:
//!
//! - every lifecycle operation is wrapped in the connector's
//!   [`ConnectorPolicy::timeout`],
//! - `connect` / `disconnect` are gated deny-by-default through the
//!   connector's [`PermissionGate`],
//! - state changes and failures are emitted as [`ConnectorEvent`]s whose
//!   payloads are structurally scrubbed of secrets,
//! - status reads are I/O-free (cached snapshots only).

use crate::connector::{
    AuthenticatedAccount, ConnectionState, Connector, ConnectorStatus, ConnectorSummary,
    HealthStatus, PermissionGrant, actions,
};
use crate::credential::AccountCredentials;
use crate::error::ConnectorError;
use crate::gate::PermissionGate;
use crate::identity::ConnectorId;
use crate::redaction::scrub_secrets;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A secret-free reference to an authenticated account, safe for events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    /// Stable account id.
    pub id: String,
    /// Human-readable account label.
    pub label: String,
}

/// An audit-worthy connector state change or failure.
///
/// Payloads carry identifiers and scrubbed text only — never secret
/// material. Consumers persist these to the audit trail.
#[derive(Debug, Clone)]
pub struct ConnectorEvent {
    /// Connector that changed.
    pub connector: ConnectorId,
    /// When the event occurred.
    pub at: DateTime<Utc>,
    /// What changed.
    pub kind: ConnectorEventKind,
}

/// Discriminator for a [`ConnectorEvent`].
#[derive(Debug, Clone)]
pub enum ConnectorEventKind {
    /// A connector was registered.
    Registered,
    /// A connector was unregistered.
    Unregistered,
    /// A connectivity check completed.
    Checked {
        /// Whether the service was reachable.
        healthy: bool,
    },
    /// An authenticated session was established.
    Connected {
        /// Accounts exposed by the session.
        accounts: Vec<AccountRef>,
    },
    /// An authenticated session was torn down.
    Disconnected {
        /// Account id that was disconnected.
        account: String,
    },
    /// A per-action grant was recorded.
    Granted {
        /// Granted action.
        action: String,
        /// Covered target prefix.
        target_pattern: String,
    },
    /// A per-action grant was removed.
    Revoked {
        /// Revoked action.
        action: String,
        /// Revoked target prefix.
        target_pattern: String,
    },
    /// A lifecycle operation failed (permission denials are not failures).
    Failed {
        /// Operation that failed.
        action: String,
        /// Scrubbed error detail.
        error: String,
    },
}

struct RegisteredConnector {
    connector: Arc<dyn Connector>,
    gate: PermissionGate,
    status: RwLock<ConnectorStatus>,
}

/// Callback receiving scrubbed connector events.
type EventSink = Arc<dyn Fn(ConnectorEvent) + Send + Sync>;

/// Thread-safe registry of connectors keyed by [`ConnectorId`].
///
/// Clone freely: the registry is `Arc`-backed internally and all methods
/// operate on shared state.
#[derive(Default)]
pub struct ConnectorRegistry {
    connectors: RwLock<HashMap<ConnectorId, Arc<RegisteredConnector>>>,
    event_sink: RwLock<Option<EventSink>>,
}

impl fmt::Debug for ConnectorRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.connectors.read();
        let ids: Vec<&str> = guard.keys().map(ConnectorId::as_str).collect();
        f.debug_struct("ConnectorRegistry")
            .field("connectors", &ids)
            .field("event_sink", &self.event_sink.read().is_some())
            .finish()
    }
}

impl ConnectorRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the event sink; `None` clears it.
    ///
    /// The sink receives every state change and failure event, scrubbed.
    pub fn set_event_sink(&self, sink: Option<EventSink>) {
        *self.event_sink.write() = sink;
    }

    /// Registers a connector; a duplicate id is rejected.
    ///
    /// Registration is synchronous and performs no I/O: the connector's
    /// initial status is `Disconnected` until a check or connect runs.
    ///
    /// # Errors
    /// Returns [`ConnectorError::Internal`] when the id is already
    /// registered.
    pub fn register(&self, connector: Arc<dyn Connector>) -> Result<(), ConnectorError> {
        let identity = connector.identity().clone();
        let registered = Arc::new(RegisteredConnector {
            connector,
            gate: PermissionGate::new(),
            status: RwLock::new(ConnectorStatus {
                identity: identity.clone(),
                connection: ConnectionState::Disconnected,
                health: None,
                accounts: Vec::new(),
            }),
        });
        let mut guard = self.connectors.write();
        if guard.contains_key(&identity.id) {
            return Err(ConnectorError::internal(format!(
                "connector already registered: {}",
                identity.id
            )));
        }
        guard.insert(identity.id.clone(), registered);
        drop(guard);
        self.emit(ConnectorEvent {
            connector: identity.id,
            at: Utc::now(),
            kind: ConnectorEventKind::Registered,
        });
        Ok(())
    }

    /// Removes a registered connector; returns `true` when one was removed.
    pub fn unregister(&self, id: &ConnectorId) -> bool {
        let removed = self.connectors.write().remove(id).is_some();
        if removed {
            self.emit(ConnectorEvent {
                connector: id.clone(),
                at: Utc::now(),
                kind: ConnectorEventKind::Unregistered,
            });
        }
        removed
    }

    /// Returns the underlying connector, for direct use by hosts.
    #[must_use]
    pub fn get(&self, id: &ConnectorId) -> Option<Arc<dyn Connector>> {
        self.connectors
            .read()
            .get(id)
            .map(|registered| registered.connector.clone())
    }

    /// Lists cached summaries; performs no connector I/O.
    #[must_use]
    pub fn list(&self) -> Vec<ConnectorSummary> {
        let guard = self.connectors.read();
        let mut summaries: Vec<_> = guard
            .values()
            .map(|registered| ConnectorSummary {
                identity: registered.status.read().identity.clone(),
                connection: registered.status.read().connection.clone(),
                account_count: registered.status.read().accounts.len(),
                action_count: registered.connector.actions().len(),
            })
            .collect();
        summaries.sort_by(|a, b| a.identity.id.as_str().cmp(b.identity.id.as_str()));
        summaries
    }

    /// Returns the cached status snapshot; performs no connector I/O.
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown id.
    pub fn status(&self, id: &ConnectorId) -> Result<ConnectorStatus, ConnectorError> {
        self.lookup(id)
            .map(|registered| registered.status.read().clone())
    }

    /// Runs a connectivity check, updates the cache, and emits an event.
    ///
    /// Read-only: never prompts for permission.
    ///
    /// # Errors
    /// Returns a transport or timeout error when the check fails.
    pub async fn check_connectivity(
        &self,
        id: &ConnectorId,
    ) -> Result<HealthStatus, ConnectorError> {
        let registered = self.lookup(id)?;
        let timeout = registered.connector.policy().timeout;
        let result = tokio::time::timeout(timeout, registered.connector.check_connectivity())
            .await
            .map_err(|_| ConnectorError::timeout(format!("connectivity check for {id}")))??;
        let health = HealthStatus {
            healthy: result.healthy,
            message: result.message.as_deref().map(scrub_secrets),
            checked_at: result.checked_at,
        };
        self.update(id, |status| status.health = Some(health.clone()));
        self.emit(ConnectorEvent {
            connector: id.clone(),
            at: Utc::now(),
            kind: ConnectorEventKind::Checked {
                healthy: health.healthy,
            },
        });
        Ok(health)
    }

    /// Authenticates with the service (permission-gated) and caches the
    /// exposed accounts.
    ///
    /// # Errors
    /// Returns [`ConnectorError::PermissionRequired`] until the user approves
    /// the connect action, then [`ConnectorError::Auth`] when the credential
    /// is rejected.
    pub async fn connect(
        &self,
        id: &ConnectorId,
        credential: &AccountCredentials,
    ) -> Result<Vec<AuthenticatedAccount>, ConnectorError> {
        let registered = self.lookup(id)?;
        let target = connector_target(id);
        let description = format!("Connect {}", registered.connector.identity().display_name);
        registered
            .gate
            .check(actions::CONNECT, &target, &description)?;

        let timeout = registered.connector.policy().timeout;
        let accounts =
            match tokio::time::timeout(timeout, registered.connector.connect(credential)).await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(ConnectorError::timeout(format!("connect for {id}")));
                }
            };
        let refs: Vec<AccountRef> = accounts
            .iter()
            .map(|account| AccountRef {
                id: account.id.clone(),
                label: account.label.clone(),
            })
            .collect();
        self.update(id, |status| {
            status.connection = ConnectionState::Connected { at: Utc::now() };
            status.accounts.clone_from(&accounts);
            status.health = None;
        });
        self.emit(ConnectorEvent {
            connector: id.clone(),
            at: Utc::now(),
            kind: ConnectorEventKind::Connected { accounts: refs },
        });
        Ok(accounts)
    }

    /// Tears down one account's session (permission-gated).
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown connector or
    /// account, and [`ConnectorError::PermissionRequired`] until the user
    /// approves the disconnect action.
    pub async fn disconnect(
        &self,
        id: &ConnectorId,
        account_id: &str,
    ) -> Result<(), ConnectorError> {
        let registered = self.lookup(id)?;
        let account = registered
            .status
            .read()
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or_else(|| {
                ConnectorError::not_found(format!("account {account_id} on connector {id}"))
            })?;
        let target = account_target(id, account_id);
        let description = format!(
            "Disconnect {} ({})",
            registered.connector.identity().display_name,
            account.label
        );
        registered
            .gate
            .check(actions::DISCONNECT, &target, &description)?;

        let timeout = registered.connector.policy().timeout;
        match tokio::time::timeout(timeout, registered.connector.disconnect(&account)).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(ConnectorError::timeout(format!("disconnect for {id}")));
            }
        }
        self.update(id, |status| {
            status.accounts.retain(|a| a.id != account_id);
            if status.accounts.is_empty() {
                status.connection = ConnectionState::Disconnected;
            }
        });
        self.emit(ConnectorEvent {
            connector: id.clone(),
            at: Utc::now(),
            kind: ConnectorEventKind::Disconnected {
                account: account_id.to_string(),
            },
        });
        Ok(())
    }

    /// Records a per-action grant (explicit user command, no prompt).
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown connector and
    /// [`ConnectorError::Internal`] for an undeclared action.
    pub fn grant(
        &self,
        id: &ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<(), ConnectorError> {
        let registered = self.lookup(id)?;
        let declared = matches!(action, actions::CONNECT | actions::DISCONNECT)
            || registered
                .connector
                .actions()
                .iter()
                .any(|declared| declared.name == action);
        if !declared {
            return Err(ConnectorError::internal(format!(
                "action {action} is not declared by connector {id}"
            )));
        }
        registered.gate.allow_pattern(action, target_pattern);
        self.emit(ConnectorEvent {
            connector: id.clone(),
            at: Utc::now(),
            kind: ConnectorEventKind::Granted {
                action: action.to_string(),
                target_pattern: target_pattern.to_string(),
            },
        });
        Ok(())
    }

    /// Removes a per-action grant (explicit user command).
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown connector.
    pub fn revoke(
        &self,
        id: &ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<bool, ConnectorError> {
        let registered = self.lookup(id)?;
        let removed = registered.gate.revoke_pattern(action, target_pattern);
        if removed {
            self.emit(ConnectorEvent {
                connector: id.clone(),
                at: Utc::now(),
                kind: ConnectorEventKind::Revoked {
                    action: action.to_string(),
                    target_pattern: target_pattern.to_string(),
                },
            });
        }
        Ok(removed)
    }

    /// Broadcasts an exact pattern revocation to every connector gate.
    ///
    /// Used by the unified permission center when a session grant recorded
    /// there is revoked; each gate matches only its own patterns.
    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        for registered in self.connectors.read().values() {
            registered.gate.revoke_pattern(action, target_pattern);
        }
    }

    /// Lists the standing grants of a connector for permission-status display.
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown connector.
    pub fn permission_status(
        &self,
        id: &ConnectorId,
    ) -> Result<Vec<PermissionGrant>, ConnectorError> {
        Ok(self.lookup(id)?.gate.patterns())
    }

    /// Records an allow-once approval for a deterministic request id.
    ///
    /// Called by the permission center after the user approved the
    /// `PermissionRequired` prompt; the retried operation reproduces the
    /// same request id and passes the gate.
    ///
    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown connector.
    pub fn approve_request(
        &self,
        id: &ConnectorId,
        request_id: &str,
    ) -> Result<(), ConnectorError> {
        self.lookup(id)?.gate.approve_request(request_id);
        Ok(())
    }

    /// Forwards the host's call context to every gate so approvals expire
    /// exactly like tool approvals (turn boundary for allow-once,
    /// conversation boundary for patterns).
    pub fn on_call_context(&self, conversation_id: &str, turn_id: Option<&str>) {
        for registered in self.connectors.read().values() {
            registered.gate.on_call_context(conversation_id, turn_id);
        }
    }

    fn lookup(&self, id: &ConnectorId) -> Result<Arc<RegisteredConnector>, ConnectorError> {
        self.connectors
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| ConnectorError::not_found(format!("connector {id}")))
    }

    fn update(&self, id: &ConnectorId, update: impl FnOnce(&mut ConnectorStatus)) {
        if let Ok(registered) = self.lookup(id) {
            let mut status = registered.status.write();
            update(&mut status);
        }
    }

    fn emit(&self, event: ConnectorEvent) {
        if let Some(sink) = self.event_sink.read().clone() {
            sink(event);
        }
    }
}

/// Stable permission target for a connector's lifecycle operations.
fn connector_target(id: &ConnectorId) -> String {
    format!("connector:{id}")
}

/// Stable permission target for one account of a connector.
fn account_target(id: &ConnectorId, account_id: &str) -> String {
    format!("connector:{id}#account:{account_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::{AccountAuthKind, ConnectorAction};
    use crate::identity::ConnectorIdentity;
    use crate::policy::ConnectorPolicy;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// In-memory mock connector exercising the full framework surface.
    struct MockConnector {
        identity: ConnectorIdentity,
        connected: AtomicBool,
        last_credential: Mutex<Option<String>>,
    }

    const MOCK_ACTIONS: &[ConnectorAction] =
        &[ConnectorAction::side_effecting("ping", "Send a test ping")];

    impl MockConnector {
        fn new() -> Self {
            Self {
                identity: ConnectorIdentity::new(
                    ConnectorId::try_new("mock.demo").unwrap(),
                    "Mock Demo",
                ),
                connected: AtomicBool::new(false),
                last_credential: Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Connector for MockConnector {
        fn identity(&self) -> &ConnectorIdentity {
            &self.identity
        }

        fn actions(&self) -> &'static [ConnectorAction] {
            MOCK_ACTIONS
        }

        fn policy(&self) -> ConnectorPolicy {
            ConnectorPolicy::default().with_timeout(Duration::from_secs(2))
        }

        async fn check_connectivity(&self) -> Result<HealthStatus, ConnectorError> {
            Ok(HealthStatus {
                healthy: true,
                message: Some("reachable".to_string()),
                checked_at: Utc::now(),
            })
        }

        async fn connect(
            &self,
            credential: &AccountCredentials,
        ) -> Result<Vec<AuthenticatedAccount>, ConnectorError> {
            if credential.credential.api_key() == Some("reject-me") {
                return Err(ConnectorError::auth("credential rejected"));
            }
            *self.last_credential.lock().unwrap() = Some(
                credential
                    .credential
                    .api_key()
                    .unwrap_or_default()
                    .to_string(),
            );
            self.connected.store(true, Ordering::SeqCst);
            Ok(vec![AuthenticatedAccount::new(
                "demo@example.com",
                "demo@example.com",
                AccountAuthKind::ApiKey,
                vec!["read".to_string()],
                Utc::now(),
            )])
        }

        async fn disconnect(&self, account: &AuthenticatedAccount) -> Result<(), ConnectorError> {
            assert_eq!(account.id, "demo@example.com");
            self.connected.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    fn demo_connector_id() -> ConnectorId {
        ConnectorId::try_new("mock.demo").unwrap()
    }

    #[tokio::test]
    async fn register_list_check_connect_disconnect_lifecycle() {
        let registry = ConnectorRegistry::new();
        let mock = Arc::new(MockConnector::new());
        registry.register(mock.clone()).expect("register succeeds");
        assert!(registry.register(mock).is_err(), "duplicate id rejected");

        let summaries = registry.list();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].identity.id.as_str(), "mock.demo");
        assert_eq!(summaries[0].account_count, 0);
        assert_eq!(summaries[0].action_count, 1);

        let health = registry
            .check_connectivity(&demo_connector_id())
            .await
            .expect("check succeeds");
        assert!(health.healthy);
        assert!(
            registry
                .status(&demo_connector_id())
                .expect("status exists")
                .health
                .is_some()
        );

        let credential = AccountCredentials::new(
            "demo@example.com",
            crate::credential::CredentialStore::from_api_key("sk-test"),
        );
        let err = registry
            .connect(&demo_connector_id(), &credential)
            .await
            .expect_err("fresh gate denies connect");
        assert!(matches!(err, ConnectorError::PermissionRequired { .. }));

        registry
            .grant(
                &demo_connector_id(),
                actions::CONNECT,
                "connector:mock.demo",
            )
            .expect("grant records");
        let accounts = registry
            .connect(&demo_connector_id(), &credential)
            .await
            .expect("granted connect succeeds");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "demo@example.com");

        let permissions = registry
            .permission_status(&demo_connector_id())
            .expect("permission status");
        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].action, actions::CONNECT);

        let status = registry.status(&demo_connector_id()).expect("status");
        assert!(matches!(
            status.connection,
            ConnectionState::Connected { .. }
        ));

        let err = registry
            .disconnect(&demo_connector_id(), "demo@example.com")
            .await
            .expect_err("fresh gate denies disconnect");
        assert!(matches!(err, ConnectorError::PermissionRequired { .. }));
        registry
            .grant(
                &demo_connector_id(),
                actions::DISCONNECT,
                "connector:mock.demo",
            )
            .expect("grant records");
        registry
            .disconnect(&demo_connector_id(), "demo@example.com")
            .await
            .expect("granted disconnect succeeds");
        let status = registry.status(&demo_connector_id()).expect("status");
        assert!(matches!(status.connection, ConnectionState::Disconnected));
    }

    #[tokio::test]
    async fn unknown_connector_ops_return_not_found() {
        let registry = ConnectorRegistry::new();
        let ghost = ConnectorId::try_new("ghost.missing").unwrap();
        assert!(registry.status(&ghost).is_err());
        assert!(registry.check_connectivity(&ghost).await.is_err());
        assert!(
            registry
                .grant(&ghost, "connector.connect", "connector:ghost")
                .is_err()
        );
        assert!(registry.permission_status(&ghost).is_err());
    }

    #[tokio::test]
    async fn approve_request_unblocks_retried_connect() {
        let registry = ConnectorRegistry::new();
        registry
            .register(Arc::new(MockConnector::new()))
            .expect("register succeeds");
        let credential = AccountCredentials::new(
            "demo@example.com",
            crate::credential::CredentialStore::from_api_key("sk-test"),
        );
        let id = demo_connector_id();
        let ConnectorError::PermissionRequired { request_id, .. } = registry
            .connect(&id, &credential)
            .await
            .expect_err("fresh gate denies")
        else {
            panic!("expected PermissionRequired");
        };
        registry
            .approve_request(&id, &request_id)
            .expect("approval records");
        registry
            .connect(&id, &credential)
            .await
            .expect("approved retry succeeds");
    }

    #[tokio::test]
    async fn auth_failure_surfaces_without_the_secret() {
        let registry = ConnectorRegistry::new();
        let mock = Arc::new(MockConnector::new());
        registry.register(mock).expect("register succeeds");
        registry
            .grant(
                &demo_connector_id(),
                actions::CONNECT,
                "connector:mock.demo",
            )
            .expect("grant records");
        let credential = AccountCredentials::new(
            "demo@example.com",
            crate::credential::CredentialStore::from_api_key("reject-me"),
        );
        let err = registry
            .connect(&demo_connector_id(), &credential)
            .await
            .expect_err("credential rejected");
        assert!(!err.to_string().contains("reject-me"));
    }

    #[tokio::test]
    async fn events_carry_no_secrets() {
        let registry = ConnectorRegistry::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        registry.set_event_sink(Some(Arc::new(move |event| {
            sink_events.lock().unwrap().push(event);
        })));
        let mock = Arc::new(MockConnector::new());
        registry.register(mock).expect("register succeeds");
        registry
            .check_connectivity(&demo_connector_id())
            .await
            .expect("check succeeds");
        registry
            .grant(
                &demo_connector_id(),
                actions::CONNECT,
                "connector:mock.demo",
            )
            .expect("grant records");
        let credential = AccountCredentials::new(
            "demo@example.com",
            crate::credential::CredentialStore::from_api_key("sk-top-secret"),
        );
        registry
            .connect(&demo_connector_id(), &credential)
            .await
            .expect("connect succeeds");

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 4);
        for event in events.iter() {
            let payload = format!("{event:?}");
            assert!(!payload.contains("sk-top-secret"));
            assert!(!payload.contains("reject-me"));
        }
    }

    #[tokio::test]
    async fn check_timeout_is_bounded() {
        struct SlowConnector(MockConnector);

        #[async_trait::async_trait]
        impl Connector for SlowConnector {
            fn identity(&self) -> &ConnectorIdentity {
                self.0.identity()
            }

            fn actions(&self) -> &'static [ConnectorAction] {
                self.0.actions()
            }

            fn policy(&self) -> ConnectorPolicy {
                ConnectorPolicy::default().with_timeout(Duration::from_millis(50))
            }

            async fn check_connectivity(&self) -> Result<HealthStatus, ConnectorError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(HealthStatus {
                    healthy: true,
                    message: None,
                    checked_at: Utc::now(),
                })
            }

            async fn connect(
                &self,
                _credential: &AccountCredentials,
            ) -> Result<Vec<AuthenticatedAccount>, ConnectorError> {
                self.0.connect(_credential).await
            }

            async fn disconnect(
                &self,
                account: &AuthenticatedAccount,
            ) -> Result<(), ConnectorError> {
                self.0.disconnect(account).await
            }
        }

        let registry = ConnectorRegistry::new();
        registry
            .register(Arc::new(SlowConnector(MockConnector::new())))
            .expect("register succeeds");
        let err = registry
            .check_connectivity(&demo_connector_id())
            .await
            .expect_err("slow check times out");
        assert!(matches!(err, ConnectorError::Timeout(_)));
    }
}
