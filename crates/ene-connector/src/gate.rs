//! Fail-closed per-action permission gate for connectors.
//!
//! Mirrors the plugin permission contract: an operation proceeds only when
//! the user already approved its deterministic request id or a standing
//! `(action, target-prefix)` pattern covers it. Approvals are scoped to the
//! turn they were granted in; patterns are scoped to the conversation, so
//! nothing outlives the context that granted it.

use crate::connector::PermissionGrant;
use crate::error::ConnectorError;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Standing `(action, target-prefix)` grants keyed to their grant time.
type AllowedPatterns = HashMap<(String, String), DateTime<Utc>>;

/// Last call context seen by the gate: conversation id and turn id.
type CallContext = Option<(String, Option<String>)>;

/// Per-connector permission gate.
///
/// Each registered connector owns one gate; the registry checks lifecycle
/// operations against it and routes approvals from the permission center
/// into it, so connector permissions share the tool permission model.
#[derive(Debug, Clone, Default)]
pub struct PermissionGate {
    approved_requests: Arc<RwLock<HashSet<String>>>,
    allowed_patterns: Arc<RwLock<AllowedPatterns>>,
    context: Arc<RwLock<CallContext>>,
}

impl PermissionGate {
    /// Creates an empty gate (deny-by-default).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow-once.
    pub fn approve_request(&self, request_id: &str) {
        self.approved_requests
            .write()
            .insert(request_id.to_string());
    }

    pub fn remove_approval(&self, request_id: &str) {
        self.approved_requests.write().remove(request_id);
    }

    /// Conversation-scoped.
    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.allowed_patterns
            .write()
            .insert((action.to_string(), target_pattern.to_string()), Utc::now());
    }

    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) -> bool {
        self.allowed_patterns
            .write()
            .remove(&(action.to_string(), target_pattern.to_string()))
            .is_some()
    }

    /// Used by the unified permission center to revoke a scope that may
    /// have been recorded with a different action spelling.
    pub fn revoke_prefix(&self, action: &str, target_prefix: &str) {
        self.allowed_patterns
            .write()
            .retain(|(a, t), _| a != action || !t.starts_with(target_prefix));
    }

    /// Used for permission-status display.
    #[must_use]
    pub fn patterns(&self) -> Vec<PermissionGrant> {
        self.allowed_patterns
            .read()
            .iter()
            .map(|((action, target), granted_at)| PermissionGrant {
                action: action.clone(),
                target_pattern: target.clone(),
                granted_at: *granted_at,
            })
            .collect()
    }

    /// Tracks the host's per-call context, expiring turn-scoped approvals.
    ///
    /// A new turn clears approved request ids; a new conversation also drops
    /// the standing patterns. Mirrors the plugin gate so both sides of the
    /// permission model expire identically.
    pub fn on_call_context(&self, conversation_id: &str, turn_id: Option<&str>) {
        let mut context = self.context.write();
        let previous = context.clone();
        *context = Some((conversation_id.to_string(), turn_id.map(str::to_string)));
        drop(context);
        let (prev_conversation, prev_turn) = match previous {
            Some((conversation, turn)) => (Some(conversation), turn),
            None => (None, None),
        };
        if prev_conversation.as_deref() != Some(conversation_id) {
            self.approved_requests.write().clear();
            self.allowed_patterns.write().clear();
        } else if prev_turn.as_deref() != turn_id {
            self.approved_requests.write().clear();
        }
    }

    /// Returns `Ok(())` when the operation may proceed, or a
    /// `PermissionRequired` error carrying the approval preview otherwise.
    ///
    /// `target` is a stable identifier without private content (e.g.
    /// `connector:github` or `connector:github#account:octocat`); the
    /// human-readable `description` is shown to the user only and is not
    /// echoed by the error's `Display` impl.
    ///
    /// The request id is a deterministic hash of the action/target/
    /// description triple, so a retried call after approval reproduces the
    /// same id — the same contract the plugin approval flow relies on.
    pub fn check(
        &self,
        action: &str,
        target: &str,
        description: &str,
    ) -> Result<(), ConnectorError> {
        if self
            .allowed_patterns
            .read()
            .keys()
            .any(|(a, t)| a == action && target.starts_with(t.as_str()))
        {
            return Ok(());
        }

        let request_id = request_id_for(action, target, description);
        if self.approved_requests.read().contains(&request_id) {
            return Ok(());
        }

        Err(ConnectorError::permission_required(
            request_id,
            action,
            target,
            description,
        ))
    }
}

/// Deterministic request id for an `(action, target, description)` triple.
///
/// Namespaced under the URL namespace; the digest is one-way, so the
/// description content never becomes readable from the id.
fn request_id_for(action: &str, target: &str, description: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("{action}:{target}:{description}").as_bytes(),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_err(gate: &PermissionGate, target: &str, description: &str) -> ConnectorError {
        gate.check("connector.connect", target, description)
            .expect_err("fresh gate denies by default")
    }

    #[test]
    fn fresh_gate_denies_by_default() {
        let gate = PermissionGate::new();
        let err = check_err(&gate, "connector:mock", "Connect mock");
        let ConnectorError::PermissionRequired {
            request_id,
            action,
            target,
            description,
        } = err
        else {
            panic!("expected PermissionRequired");
        };
        assert!(!request_id.is_empty());
        assert_eq!(action, "connector.connect");
        assert_eq!(target, "connector:mock");
        assert_eq!(description, "Connect mock");
    }

    #[test]
    fn approved_request_passes_and_retries_match() {
        let gate = PermissionGate::new();
        let first = check_err(&gate, "connector:mock", "Connect mock");
        let second = check_err(&gate, "connector:mock", "Connect mock");
        let (
            ConnectorError::PermissionRequired { request_id: a, .. },
            ConnectorError::PermissionRequired { request_id: b, .. },
        ) = (first, second)
        else {
            panic!("expected PermissionRequired twice");
        };
        assert_eq!(a, b, "request id must be deterministic");
        gate.approve_request(&a);
        gate.check("connector.connect", "connector:mock", "Connect mock")
            .expect("approved request passes");
    }

    #[test]
    fn approvals_expire_at_turn_boundary() {
        let gate = PermissionGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        let ConnectorError::PermissionRequired { request_id, .. } =
            check_err(&gate, "connector:mock", "Connect mock")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.check("connector.connect", "connector:mock", "Connect mock")
            .expect("approval passes within the granting turn");
        gate.on_call_context("conv-1", Some("turn-2"));
        assert!(
            gate.check("connector.connect", "connector:mock", "Connect mock")
                .is_err(),
            "approval must not survive into the next turn"
        );
    }

    #[test]
    fn patterns_expire_at_conversation_boundary() {
        let gate = PermissionGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.allow_pattern("connector.connect", "connector:mock");
        gate.check("connector.connect", "connector:mock", "Connect mock")
            .expect("pattern grants within the conversation");
        gate.on_call_context("conv-2", Some("turn-1"));
        assert!(
            gate.check("connector.connect", "connector:mock", "Connect mock")
                .is_err(),
            "patterns must not leak into another conversation"
        );
    }

    #[test]
    fn revoke_removes_pattern_and_prefix_revoke_covers_spellings() {
        let gate = PermissionGate::new();
        gate.allow_pattern("connector.connect", "connector:mock");
        gate.revoke_prefix("connector.connect", "connector:");
        assert!(
            gate.check("connector.connect", "connector:mock", "Connect mock")
                .is_err(),
            "revoked pattern must deny again"
        );

        gate.allow_pattern("connector.connect", "connector:mock");
        gate.revoke_pattern("connector.connect", "connector:mock");
        assert!(
            gate.check("connector.connect", "connector:mock", "Connect mock")
                .is_err(),
            "exact revoke must deny again"
        );
    }

    #[test]
    fn patterns_list_reflects_grants() {
        let gate = PermissionGate::new();
        assert!(gate.patterns().is_empty());
        gate.allow_pattern("connector.connect", "connector:mock");
        let patterns = gate.patterns();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].action, "connector.connect");
        assert_eq!(patterns[0].target_pattern, "connector:mock");
    }
}
