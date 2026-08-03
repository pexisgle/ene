use ene_plugin_proto::ToolError;
use std::collections::HashSet;
use std::sync::Arc;

/// Permission request identifiers the user has approved for this turn.
///
/// The host delivers approvals via `ToolProvider::approve_permission` after
/// the user answered the `PermissionRequired` prompt, then re-invokes the
/// tool. Recording the request id here lets the retried call proceed without
/// prompting again — the same contract `ene-plugin-fs`'s sandbox uses.
pub type ApprovedRequests = Arc<std::sync::RwLock<HashSet<String>>>;

/// Session-wide allow patterns (`action`, `target_prefix`), granted via
/// `ToolProvider::allow_pattern` when the user picks "allow for this
/// session".
pub type AllowedPatterns = Arc<std::sync::RwLock<HashSet<(String, String)>>>;

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
}

impl ApprovalGate {
    /// Creates an empty gate (deny-by-default).
    #[must_use]
    pub fn new() -> Self {
        Self {
            approved_requests: Arc::new(std::sync::RwLock::new(HashSet::new())),
            allowed_patterns: Arc::new(std::sync::RwLock::new(HashSet::new())),
        }
    }

    /// Records a user approval for the given request id.
    pub fn approve_request(&self, request_id: &str) {
        if let Ok(mut guard) = self.approved_requests.write() {
            guard.insert(request_id.to_string());
        }
    }

    /// Records a session-wide allow pattern.
    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.insert((action.to_string(), target_pattern.to_string()));
        }
    }

    /// Removes a session-wide allow pattern.
    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.remove(&(action.to_string(), target_pattern.to_string()));
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
    /// The request id is a **deterministic** hash of `action` + `target`
    /// rather than a random uuid: the host re-invokes the tool after an
    /// approval, and the retried call must produce the *same* request id for
    /// [`Self::approve_request`] to match it. A random id per call would
    /// make the user approve the same operation again on every retry.
    pub fn check(&self, action: &str, target: &str, description: &str) -> Result<(), ToolError> {
        if let Ok(guard) = self.allowed_patterns.read() {
            for (allowed_action, allowed_target) in guard.iter() {
                if allowed_action == action && target.starts_with(allowed_target) {
                    return Ok(());
                }
            }
        }

        let request_id = request_id_for(action, target);
        if let Ok(guard) = self.approved_requests.read()
            && guard.contains(&request_id)
        {
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

/// Stable request id for an (action, target) pair.
///
/// Namespaced under the URL namespace to keep ids globally unique; the
/// SHA-1 digest is deterministic, which is the whole point (see
/// [`ApprovalGate::check`]).
fn request_id_for(action: &str, target: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("{action}:{target}").as_bytes(),
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
        let err = gate.check(actions::CALENDAR_WRITE, "calendar:a1", "preview");
        let Err(ToolError::PermissionRequired { request_id, .. }) = err else {
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
        let first = gate
            .check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect_err("denied on first call");
        let second = gate
            .check(actions::CALENDAR_WRITE, "calendar:a1", "preview")
            .expect_err("denied again (not yet approved)");
        match (first, second) {
            (
                ToolError::PermissionRequired { request_id: a, .. },
                ToolError::PermissionRequired { request_id: b, .. },
            ) => assert_eq!(
                a, b,
                "request id must be deterministic per (action, target)"
            ),
            _ => panic!("expected PermissionRequired"),
        }
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
