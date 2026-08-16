//! Connector handle: the consumer-facing half of the connector framework.
//!
//! Read-only queries (`list` / `status` / `permissions`) read the registry's
//! cached snapshots directly and never block on connector I/O. Operations
//! that touch the external service or mutate grants (`check` / `connect` /
//! `disconnect` / `grant` / `revoke`) cross the actor mailbox so they
//! serialize with turns, resolve permission prompts through the shared
//! permission center, and land in the audit trail.

use crate::EneEvent;
use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use crate::streaming::{PermissionDecision, PermissionScope};
use crate::types::{RequestId, TurnId};
use ene_connector::{
    AccountCredentials, AuthenticatedAccount, Connector, ConnectorError, ConnectorId,
    ConnectorRegistry, ConnectorStatus, ConnectorSummary, HealthStatus, PermissionGrant,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

#[derive(Debug, thiserror::Error)]
pub enum ConnectorHandleError {
    #[error("actor unavailable")]
    ActorDead,
    #[error(transparent)]
    Connector(#[from] ConnectorError),
}

impl From<PublicApiError> for ConnectorHandleError {
    fn from(_: PublicApiError) -> Self {
        Self::ActorDead
    }
}

/// Obtained from [`crate::EneHandle::connectors`]. Clone freely.
#[derive(Clone)]
pub struct ConnectorHandle {
    registry: Arc<ConnectorRegistry>,
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
}

impl ConnectorHandle {
    pub(crate) fn new(
        registry: Arc<ConnectorRegistry>,
        cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    ) -> Self {
        Self { registry, cmd_tx }
    }

    /// # Errors
    /// Returns [`ConnectorError::Internal`] when the id is already
    /// registered.
    pub fn register(&self, connector: Arc<dyn Connector>) -> Result<(), ConnectorError> {
        self.registry.register(connector)
    }

    pub fn unregister(&self, id: &ConnectorId) -> bool {
        self.registry.unregister(id)
    }

    #[must_use]
    pub fn list(&self) -> Vec<ConnectorSummary> {
        self.registry.list()
    }

    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown id.
    pub fn status(&self, id: &ConnectorId) -> Result<ConnectorStatus, ConnectorError> {
        self.registry.status(id)
    }

    /// # Errors
    /// Returns [`ConnectorError::NotFound`] for an unknown id.
    pub fn permissions(&self, id: &ConnectorId) -> Result<Vec<PermissionGrant>, ConnectorError> {
        self.registry.permission_status(id)
    }

    /// Runs a connectivity check (read-only, audited).
    ///
    /// # Errors
    /// Returns [`ConnectorHandleError::ActorDead`] when the actor is gone or
    /// a [`ConnectorError`] from the check.
    pub async fn check(&self, id: &ConnectorId) -> Result<HealthStatus, ConnectorHandleError> {
        let (reply, rx) = oneshot::channel();
        self.send(EneCommand::ConnectorCheck {
            id: id.clone(),
            reply,
        })?;
        rx.await
            .map_err(|_| ConnectorHandleError::ActorDead)?
            .map_err(ConnectorHandleError::from)
    }

    /// Authenticates with the service; prompts through the permission center
    /// when the connect action is not yet approved.
    ///
    /// # Errors
    /// Returns a [`ConnectorError`] from the connector or the permission
    /// flow.
    pub async fn connect(
        &self,
        id: &ConnectorId,
        credential: AccountCredentials,
    ) -> Result<Vec<AuthenticatedAccount>, ConnectorHandleError> {
        let (reply, rx) = oneshot::channel();
        self.send(EneCommand::ConnectorConnect {
            id: id.clone(),
            credential,
            reply,
        })?;
        rx.await
            .map_err(|_| ConnectorHandleError::ActorDead)?
            .map_err(ConnectorHandleError::from)
    }

    /// Tears down one account's session; prompts through the permission
    /// center when the disconnect action is not yet approved.
    ///
    /// # Errors
    /// Returns a [`ConnectorError`] from the connector or the permission
    /// flow.
    pub async fn disconnect(
        &self,
        id: &ConnectorId,
        account: &str,
    ) -> Result<(), ConnectorHandleError> {
        let (reply, rx) = oneshot::channel();
        self.send(EneCommand::ConnectorDisconnect {
            id: id.clone(),
            account: account.to_string(),
            reply,
        })?;
        rx.await
            .map_err(|_| ConnectorHandleError::ActorDead)?
            .map_err(ConnectorHandleError::from)
    }

    /// Records a per-action grant (explicit user command, audited).
    ///
    /// # Errors
    /// Returns a [`ConnectorError`] for unknown connectors or undeclared
    /// actions.
    pub async fn grant(
        &self,
        id: &ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<(), ConnectorHandleError> {
        let (reply, rx) = oneshot::channel();
        self.send(EneCommand::ConnectorGrant {
            id: id.clone(),
            action: action.to_string(),
            target_pattern: target_pattern.to_string(),
            reply,
        })?;
        rx.await
            .map_err(|_| ConnectorHandleError::ActorDead)?
            .map_err(ConnectorHandleError::from)
    }

    /// Removes a per-action grant (explicit user command, audited).
    ///
    /// # Errors
    /// Returns a [`ConnectorError`] for unknown connectors.
    pub async fn revoke(
        &self,
        id: &ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<bool, ConnectorHandleError> {
        let (reply, rx) = oneshot::channel();
        self.send(EneCommand::ConnectorRevoke {
            id: id.clone(),
            action: action.to_string(),
            target_pattern: target_pattern.to_string(),
            reply,
        })?;
        rx.await
            .map_err(|_| ConnectorHandleError::ActorDead)?
            .map_err(ConnectorHandleError::from)
    }

    fn send(&self, command: EneCommand) -> Result<(), ConnectorHandleError> {
        self.cmd_tx
            .send(command)
            .map_err(|_| ConnectorHandleError::ActorDead)
    }
}

/// Runs a connector lifecycle operation with permission resolution, then
/// records one audit row.
///
/// Must run outside the actor loop (spawned task): the resolution loop
/// awaits a `PermissionDecision` that only the actor loop can deliver, so
/// executing it inline would deadlock the mailbox — the same reason tool
/// prompt resolution runs in stream tasks.
///
/// Mirrors the tool resolution loop: register the pending decision before
/// emitting the event, apply allow-once / allow-session / deny, and retry
/// with the same inputs after an approval.
pub(crate) async fn run_connector_operation<T, F, Fut>(
    registry: &Arc<ConnectorRegistry>,
    id: &ConnectorId,
    op: &str,
    event_tx: &broadcast::Sender<EneEvent>,
    pending_permissions: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    permission_scopes: &Arc<Mutex<Vec<PermissionScope>>>,
    prompt_timeout_ms: u64,
    active_turn: Option<TurnId>,
    audit_store: Option<Arc<ene_store::MemoryStore>>,
    session_id: String,
    mut run: F,
) -> Result<T, ConnectorError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ConnectorError>>,
{
    const MAX_PENDING_ROUNDS: usize = 8;
    let mut audit_decision = ene_store::AuditDecision::NotRequired;
    // Ops that never prompt (check) or fail before prompting still record
    // their canonical action and target, so failed-op audit rows carry
    // context instead of empty fields.
    let mut audit_action = match op {
        "connect" => ene_connector::actions::CONNECT.to_string(),
        "disconnect" => ene_connector::actions::DISCONNECT.to_string(),
        _ => format!("connector.{op}"),
    };
    let mut audit_target = format!("connector:{id}");
    let tool_name = format!("connector.{id}.{op}");
    let mut result = run().await;
    for _ in 0..MAX_PENDING_ROUNDS {
        let Err(ConnectorError::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) = &result
        else {
            break;
        };
        let request_id = request_id.clone();
        audit_action.clone_from(action);
        audit_target.clone_from(target);
        let req_id = RequestId::from(request_id.clone());
        let (decide_tx, decide_rx) = oneshot::channel();
        {
            let mut guard = pending_permissions.lock().await;
            guard.insert(req_id.clone(), decide_tx);
        }
        drop(event_tx.send(EneEvent::PermissionRequired {
            turn: active_turn.clone().unwrap_or_default(),
            origin: crate::types::TurnOrigin::User,
            request_id: req_id.clone(),
            action: action.clone(),
            target: target.clone(),
            description: description.clone(),
        }));
        let decision = crate::streaming::await_permission_decision(
            decide_rx,
            &tokio_util::sync::CancellationToken::new(),
            prompt_timeout_ms,
            &req_id,
        )
        .await;
        match decision {
            Some(PermissionDecision::AllowOnce) => {
                audit_decision = ene_store::AuditDecision::AllowOnce;
                if let Err(error) = registry.approve_request(id, &request_id) {
                    record_connector_audit(
                        audit_store.as_ref(),
                        &session_id,
                        active_turn.as_ref(),
                        &tool_name,
                        &audit_action,
                        &audit_target,
                        audit_decision,
                        false,
                    );
                    return Err(error);
                }
                result = run().await;
                // Allow-once is one-shot: outside a turn there is no later
                // turn boundary to expire it, so drop it as soon as the
                // retried operation finishes.
                if let Err(error) = registry.expire_request(id, &request_id) {
                    tracing::warn!(
                        component = "connectors",
                        connector = %id,
                        error = %error,
                        "failed to expire connector approval"
                    );
                }
            }
            Some(PermissionDecision::AllowSession) => {
                audit_decision = ene_store::AuditDecision::AllowSession;
                if let Err(error) = registry.grant(id, action, target) {
                    record_connector_audit(
                        audit_store.as_ref(),
                        &session_id,
                        active_turn.as_ref(),
                        &tool_name,
                        &audit_action,
                        &audit_target,
                        audit_decision,
                        false,
                    );
                    return Err(error);
                }
                let mut guard = permission_scopes.lock().await;
                let next_id = guard
                    .iter()
                    .map(|scope| scope.id)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
                if let Some(existing) = guard
                    .iter_mut()
                    .find(|scope| scope.action == *action && scope.target_pattern == *target)
                {
                    existing.granted_at = chrono::Utc::now();
                } else {
                    guard.push(PermissionScope {
                        id: next_id,
                        action: action.clone(),
                        target_pattern: target.clone(),
                        grant_type: crate::streaming::GrantType::Session,
                        granted_at: chrono::Utc::now(),
                    });
                }
                drop(guard);
                result = run().await;
            }
            Some(PermissionDecision::Deny) | None => {
                record_connector_audit(
                    audit_store.as_ref(),
                    &session_id,
                    active_turn.as_ref(),
                    &tool_name,
                    &audit_action,
                    &audit_target,
                    ene_store::AuditDecision::Denied,
                    false,
                );
                return Err(ConnectorError::permission_required(
                    request_id,
                    action.clone(),
                    target.clone(),
                    description.clone(),
                ));
            }
        }
    }
    record_connector_audit(
        audit_store.as_ref(),
        &session_id,
        active_turn.as_ref(),
        &tool_name,
        &audit_action,
        &audit_target,
        audit_decision,
        result.is_ok(),
    );
    result
}

/// Arguments are empty by construction — connector ops carry no payload —
/// and the store redacts argument JSON as a second layer. Out-of-band ops
/// (no active turn) record an empty `turn_id`; session linkage is preserved
/// via `session_id`.
pub(crate) fn record_connector_audit(
    audit_store: Option<&Arc<ene_store::MemoryStore>>,
    session_id: &str,
    active_turn: Option<&TurnId>,
    tool_name: &str,
    action: &str,
    target: &str,
    decision: ene_store::AuditDecision,
    success: bool,
) {
    let Some(store) = audit_store else {
        return;
    };
    ene_store::MemoryStore::spawn_insert_audit_entry(
        store,
        ene_store::NewAuditEntry {
            turn_id: active_turn.map_or_else(String::new, ToString::to_string),
            session_id: Some(session_id.to_string()),
            tool_name: tool_name.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            decision,
            success,
            arguments: "{}".to_string(),
        },
    );
}
