use ene_plugin_proto::ToolError;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Permission request identifiers the user approved for this turn.
///
/// The host delivers approvals via `ToolProvider::approve_permission` after
/// the user answered the `PermissionRequired` prompt, then re-invokes the
/// tool. Recording the request id here lets the retried call proceed without
/// prompting again. [`ApprovalGate::on_call_context`] clears the set when the
/// turn changes, so an approval never outlives the turn it was granted in.
pub type ApprovedRequests = Arc<RwLock<HashSet<String>>>;

/// Session-wide allow patterns (`action`, `target_prefix`), granted via
/// `ToolProvider::allow_pattern` when the user picks "allow for this
/// session". Cleared when the conversation changes.
pub type AllowedPatterns = Arc<RwLock<HashSet<(String, String)>>>;

/// Conversation and turn of the most recent call context.
pub type CallContextState = Arc<RwLock<Option<(String, Option<String>)>>>;

/// Fail-closed gate for physical state-changing home automation actions.
///
/// `homeassistant.turn_on`, `homeassistant.turn_off`, and
/// `homeassistant.set_temperature` pass their entity and a human-readable
/// preview through [`Self::check`] *before* any HTTP request. The call
/// proceeds only when the user already approved the exact request id, or a
/// session-wide pattern covers the target; otherwise a `PermissionRequired`
/// error is returned and the host asks the user.
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
    /// `homeassistant:entity:light.living_room`); the human-readable
    /// `description` is shown to the user only and is not logged by the
    /// host's audit trail.
    ///
    /// The request id is a **deterministic** hash of `action` + `target` +
    /// `description` rather than a random uuid: the host re-invokes the tool
    /// after an approval, and the retried call replays identical arguments,
    /// so it must produce the *same* request id for
    /// [`Self::approve_request`] to match it. A random id per call would
    /// make the user approve the same operation again on every retry.
    /// Hashing the description additionally scopes one approval to one
    /// concrete operation: a retry with different arguments yields a
    /// different id and requires a fresh prompt.
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
/// [`ApprovalGate::check`]). The id is a one-way digest, so the description
/// never becomes readable from the id.
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
    /// Turning a Home Assistant entity on.
    pub const HOMEASSISTANT_TURN_ON: &str = "HomeAssistantTurnOn";
    /// Turning a Home Assistant entity off.
    pub const HOMEASSISTANT_TURN_OFF: &str = "HomeAssistantTurnOff";
    /// Setting the target temperature of a Home Assistant climate entity.
    pub const HOMEASSISTANT_SET_TEMPERATURE: &str = "HomeAssistantSetTemperature";
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "homeassistant:entity:light.living_room";

    fn check_err(gate: &ApprovalGate, action: &str, description: &str) -> ToolError {
        gate.check(action, TARGET, description)
            .expect_err("expected PermissionRequired")
    }

    #[test]
    fn fresh_gate_denies_by_default() {
        let gate = ApprovalGate::new();
        let err = gate.check(
            actions::HOMEASSISTANT_TURN_ON,
            TARGET,
            "Turn on light.living_room in Home Assistant",
        );
        let err = match err {
            Err(ToolError::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                assert!(!request_id.is_empty());
                assert_eq!(action, actions::HOMEASSISTANT_TURN_ON);
                assert_eq!(target, TARGET);
                assert_eq!(description, "Turn on light.living_room in Home Assistant");
                true
            }
            _ => false,
        };
        assert!(err, "expected PermissionRequired");
    }

    #[test]
    fn approved_request_passes() {
        let gate = ApprovalGate::new();
        let Err(ToolError::PermissionRequired { request_id, .. }) = gate.check(
            actions::HOMEASSISTANT_TURN_ON,
            TARGET,
            "Turn on light.living_room in Home Assistant",
        ) else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.check(
            actions::HOMEASSISTANT_TURN_ON,
            TARGET,
            "Turn on light.living_room in Home Assistant",
        )
        .expect("approved request must pass");
    }

    #[test]
    fn retried_call_gets_the_same_request_id() {
        // The host re-invokes the tool after approval; the retried call must
        // carry the identical request id so the recorded approval matches.
        let gate = ApprovalGate::new();
        let first = check_err(&gate, actions::HOMEASSISTANT_TURN_OFF, "preview");
        let second = check_err(&gate, actions::HOMEASSISTANT_TURN_OFF, "preview");
        match (first, second) {
            (
                ToolError::PermissionRequired {
                    request_id: first_id,
                    ..
                },
                ToolError::PermissionRequired {
                    request_id: second_id,
                    ..
                },
            ) => assert_eq!(first_id, second_id),
            _ => panic!("expected PermissionRequired"),
        }
    }

    #[test]
    fn changed_description_changes_the_request_id() {
        let gate = ApprovalGate::new();
        let first = check_err(&gate, actions::HOMEASSISTANT_TURN_ON, "preview one");
        let second = check_err(&gate, actions::HOMEASSISTANT_TURN_ON, "preview two");
        match (first, second) {
            (
                ToolError::PermissionRequired {
                    request_id: first_id,
                    ..
                },
                ToolError::PermissionRequired {
                    request_id: second_id,
                    ..
                },
            ) => assert_ne!(first_id, second_id),
            _ => panic!("expected PermissionRequired"),
        }
    }

    #[test]
    fn approvals_expire_at_turn_boundary() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        let Err(ToolError::PermissionRequired { request_id, .. }) =
            gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview")
            .expect("approved in turn 1 must pass");

        gate.on_call_context("conv-1", Some("turn-2"));
        let err = gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview");
        assert!(matches!(err, Err(ToolError::PermissionRequired { .. })));
    }

    #[test]
    fn approvals_survive_repeated_calls_in_the_same_turn() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        let Err(ToolError::PermissionRequired { request_id, .. }) =
            gate.check(actions::HOMEASSISTANT_SET_TEMPERATURE, TARGET, "preview")
        else {
            panic!("expected PermissionRequired");
        };
        gate.approve_request(&request_id);
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.check(actions::HOMEASSISTANT_SET_TEMPERATURE, TARGET, "preview")
            .expect("same-turn repeated call must keep the approval");
    }

    #[test]
    fn session_pattern_expires_at_conversation_boundary() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.allow_pattern(actions::HOMEASSISTANT_TURN_ON, "homeassistant:entity:");
        gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview")
            .expect("session pattern must pass within the conversation");

        gate.on_call_context("conv-2", Some("turn-1"));
        let err = gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview");
        assert!(matches!(err, Err(ToolError::PermissionRequired { .. })));
    }

    #[test]
    fn session_pattern_matches_target_prefix() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.allow_pattern(
            actions::HOMEASSISTANT_TURN_ON,
            "homeassistant:entity:light.",
        );
        gate.check(actions::HOMEASSISTANT_TURN_ON, TARGET, "preview")
            .expect("pattern must match the target prefix");
        let err = gate.check(
            actions::HOMEASSISTANT_TURN_ON,
            "homeassistant:entity:switch.plug",
            "preview",
        );
        assert!(matches!(err, Err(ToolError::PermissionRequired { .. })));
    }

    #[test]
    fn revoked_pattern_denies_again() {
        let gate = ApprovalGate::new();
        gate.on_call_context("conv-1", Some("turn-1"));
        gate.allow_pattern(actions::HOMEASSISTANT_TURN_OFF, "homeassistant:entity:");
        gate.revoke_pattern(actions::HOMEASSISTANT_TURN_OFF, "homeassistant:entity:");
        let err = gate.check(actions::HOMEASSISTANT_TURN_OFF, TARGET, "preview");
        assert!(matches!(err, Err(ToolError::PermissionRequired { .. })));
    }
}
