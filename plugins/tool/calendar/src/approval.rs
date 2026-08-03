use ene_plugin_proto::ToolError;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Permission request identifiers the user approved for this turn.
///
/// The host delivers approvals via `ToolProvider::approve_permission` after
/// the user answered the `PermissionRequired` prompt, then re-invokes the
/// tool. Recording the request id here lets the retried call proceed without
/// prompting again — the same contract `ene-plugin-fs`'s sandbox uses.
/// [`ApprovalGate::on_call_context`] clears the set when the turn changes,
/// so an approval never outlives the turn it was granted in.
pub type ApprovedRequests = Arc<RwLock<HashSet<String>>>;

/// Session-wide allow patterns (`action`, `target_prefix`), granted via
/// `ToolProvider::allow_pattern` when the user picks "allow for this
/// session". Cleared when the conversation changes.
pub type AllowedPatterns = Arc<RwLock<HashSet<(String, String)>>>;

/// Conversation and turn of the most recent call context.
pub type CallContextState = Arc<RwLock<Option<(String, Option<String>)>>>;

/// Fail-closed gate for calendar write operations.
///
/// Every mutating calendar action passes its target and a human-readable
/// preview through [`Self::check`] *before* touching the store. The call
/// proceeds only when the user already approved the exact request id, or a
/// session-wide pattern covers the target; otherwise a
/// `PermissionRequired` error is returned and the host asks the user.
#[derive(Clone)]
pub struct ApprovalGate {
    approved_requests: ApprovedRequests,
    allowed_patterns: AllowedPatterns,
    context: CallContextState,
}

impl ApprovalGate {
    /// Creates an empty gate (deny-by-default).
    #[must_use]
    pub fn new() -> Self {
        Self {
            approved_requests: Arc::new(RwLock::new(HashSet::new())),
            allowed_patterns: Arc::new(RwLock::new(HashSet::new())),
            context: Arc::new(RwLock::new(None)),
        }
    }

    /// Records a user approval for the given request id.
    pub fn approve_request(&self, request_id: &str) {
        self.approved_requests
            .write()
            .insert(request_id.to_string());
    }

    /// Records a session-wide allow pattern.
    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.allowed_patterns
            .write()
            .insert((action.to_string(), target_pattern.to_string()));
    }

    /// Removes a session-wide allow pattern.
    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.allowed_patterns
            .write()
            .remove(&(action.to_string(), target_pattern.to_string()));
    }

    /// Tracks the host's per-call context, expiring turn-scoped approvals.
    ///
    /// The host calls this before every tool invocation with the current
    /// conversation and turn identifiers. A new turn clears the approved
    /// requests ("approved for this turn" must not outlive the turn); a new
    /// conversation additionally drops the session-wide patterns, which are
    /// scoped to the conversation they were granted in.
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
    /// `calendar:<account-id>` or `calendar:<account-id>#<event-id>`); the
    /// human-readable `description` (which may mention titles) is shown to
    /// the user only and is not logged by the host's audit trail.
    ///
    /// The request id is a **deterministic** hash of `action` + `target` +
    /// `description` rather than a random uuid: the host re-invokes the tool
    /// after an approval, and the retried call replays identical arguments,
    /// so it must produce the *same* request id for
    /// [`Self::approve_request`] to match it. A random id per call would
    /// make the user approve the same operation again on every retry.
    /// Hashing the description additionally scopes one approval to one
    /// concrete operation: a retry with different arguments (e.g. a changed
    /// event title) yields a different id and requires a fresh prompt.
    pub fn check(&self, action: &str, target: &str, description: &str) -> Result<(), ToolError> {
        if self
            .allowed_patterns
            .read()
            .iter()
            .any(|(a, t)| a == action && target.starts_with(t.as_str()))
        {
            return Ok(());
        }

        let request_id = request_id_for(action, target, description);
        if self.approved_requests.read().contains(&request_id) {
            return Ok(());
        }

        Err(ToolError::PermissionRequired {
            request_id,
            action: action.to_string(),
            target: target.to_string(),
            description: description.to_string(),
        })
    }
}

/// Stable request id for an (action, target, description) triple.
///
/// Namespaced under the URL namespace to keep ids globally unique; the
/// SHA-1 digest is deterministic, which is the whole point (see
/// [`ApprovalGate::check`]). The id is a one-way digest, so the event
/// content in the description never becomes readable from the id.
fn request_id_for(action: &str, target: &str, description: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("{action}:{target}:{description}").as_bytes(),
    )
    .to_string()
}

impl Default for ApprovalGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonical action names used as the `action` field of
/// `PermissionRequired` and as allow-pattern keys.
pub mod actions {
    /// Creating a new calendar account.
    pub const CALENDAR_ADD: &str = "CalendarAdd";
    /// Creating or updating an event.
    pub const CALENDAR_WRITE: &str = "CalendarWrite";
    /// Cancelling an event or removing an account.
    pub const CALENDAR_DELETE: &str = "CalendarDelete";
    /// Changing a calendar's read/write permission flags.
    pub const CALENDAR_PERMISSION: &str = "CalendarPermission";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_err(gate: &ApprovalGate, description: &str) -> ToolError {
        gate.check(actions::CALENDAR_WRITE, "calendar:a1", description)
            .expect_err("expected PermissionRequired")
    }

    #[test]
    fn fresh_gate_denies_by_default() {
        let gate = ApprovalGate::new();
        let err = gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview");
        let err = match err {
            Err(ToolError::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                assert!(!request_id.is_empty());
                assert_eq!(action, actions::CALENDAR_WRITE);
                assert_eq!(target, "calendar:a1");
                assert_eq!(description, "preview");
                true
            }
            _ => false,
        };
        assert!(err, "expected PermissionRequired");
    }

    #[test]
    fn approved_request_passes() {
        let gate = ApprovalGate::new();
        let Err(ToolError::PermissionRequired { request_id, .. }) =
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect("approved request must pass");
    }

    #[test]
    fn retried_call_gets_the_same_request_id() {
        // The host re-invokes the tool after approval; the retried call must
        // carry the identical request id so the recorded approval matches.
        let gate = ApprovalGate::new();
        let first = check_err(&gate, "preview");
        let second = check_err(&gate, "preview");
        match (first, second) {
            (
                ToolError::PermissionRequired { request_id: a, .. },
                ToolError::PermissionRequired { request_id: b, .. },
            ) => assert_eq!(
                a, b,
                "request id must be deterministic per (action, target, description)"
            ),
            _ => panic!("expected PermissionRequired"),
        }
    }

    #[test]
    fn changed_description_changes_the_request_id() {
        // One approval = one concrete operation: an identical retry matches,
        // a changed description (different event content) must not.
        let gate = ApprovalGate::new();
        let first = check_err(&gate, "Create event 'standup' ...");
        let second = check_err(&gate, "Create event 'retro' ...");
        match (first, second) {
            (
                ToolError::PermissionRequired { request_id: a, .. },
                ToolError::PermissionRequired { request_id: b, .. },
            ) => assert_ne!(a, b, "different content must yield different ids"),
            _ => panic!("expected PermissionRequired"),
        }
    }

    #[test]
    fn approvals_expire_at_turn_boundary() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        let Err(ToolError::PermissionRequired { request_id, .. }) =
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect("approval passes within the granting turn");
        gate.on_call_context("conv-1", Some("turn-2"));
        assert!(
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
                .is_err(),
            "approval must not survive into the next turn"
        );
    }

    #[test]
    fn approvals_survive_repeated_calls_in_the_same_turn() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        let Err(ToolError::PermissionRequired { request_id, .. }) =
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect("same turn keeps the approval");
    }

    #[test]
    fn session_pattern_expires_at_conversation_boundary() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.allow_pattern(actions::CALENDAR_WRITE, "calendar:a1");
        gate.check(actions::CALENDAR_WRITE, "calendar:a1#evt1", "preview")
            .expect("pattern grants within the conversation");
        gate.on_call_context("conv-2", Some("turn-1"));
        assert!(
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
                .is_err(),
            "patterns must not leak into another conversation"
        );
    }

    #[test]
    fn session_pattern_matches_prefix() {
        let gate = ApprovalGate::new();
        gate.allow_pattern(actions::CALENDAR_WRITE, "calendar:a1");
        gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect("exact target matches");
        gate.check(actions::CALENDAR_WRITE, "calendar:a1#evt1", "preview")
            .expect("event target under the account matches");
    }

    #[test]
    fn session_pattern_is_action_scoped() {
        let gate = ApprovalGate::new();
        gate.allow_pattern(actions::CALENDAR_WRITE, "calendar:a1");
        assert!(
            gate.check(actions::CALENDAR_DELETE, "calendar:a1", "preview")
                .is_err(),
            "allow pattern for CalendarWrite must not cover CalendarDelete"
        );
    }

    #[test]
    fn revoke_removes_session_pattern() {
        let gate = ApprovalGate::new();
        gate.allow_pattern(actions::CALENDAR_WRITE, "calendar:a1");
        gate.revoke_pattern(actions::CALENDAR_WRITE, "calendar:a1");
        assert!(
            gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
                .is_err(),
            "revoked pattern must deny again"
        );
    }
}
