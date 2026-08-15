use ene_plugin_proto::ToolError;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// Permission request identifiers the user approved for this turn.
///
/// The host delivers approvals via `ToolProvider::approve_permission`
/// after the user answered the `PermissionRequired` prompt, then
/// re-invokes the tool. Recording the request id here lets the retried
/// call proceed without prompting again. [`ApprovalGate::on_call_context`]
/// clears the set when the turn changes, so an approval never outlives
/// the turn it was granted in.
pub type ApprovedRequests = Arc<RwLock<HashSet<String>>>;

/// Session-wide allow patterns (`action`, `target_prefix`), granted via
/// `ToolProvider::allow_pattern` when the user picks "allow for this
/// session". Cleared when the conversation changes.
pub type AllowedPatterns = Arc<RwLock<HashSet<(String, String)>>>;

pub type CallContextState = Arc<RwLock<Option<(String, Option<String>)>>>;

/// Fail-closed gate for permission-gated actions.
///
/// [`Self::check`] runs *before* the guarded operation. The call proceeds
/// only when the user already approved the exact request id, or a
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

    pub fn approve_request(&self, request_id: &str) {
        self.approved_requests
            .write()
            .insert(request_id.to_string());
    }

    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.allowed_patterns
            .write()
            .insert((action.to_string(), target_pattern.to_string()));
    }

    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.allowed_patterns
            .write()
            .remove(&(action.to_string(), target_pattern.to_string()));
    }

    /// Tracks the host's per-call context, expiring turn-scoped approvals.
    ///
    /// A new turn clears the approved requests ("approved for this turn"
    /// must not outlive the turn); a new conversation additionally drops
    /// the session-wide patterns, which are scoped to the conversation
    /// they were granted in.
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
    /// `target` is a stable identifier without private content; the
    /// human-readable `description` is shown to the user only.
    ///
    /// The request id is a deterministic hash of `action` + `target` +
    /// `description`: the host re-invokes the tool after an approval, and
    /// the retried call replays identical arguments, so it must produce
    /// the *same* request id for [`Self::approve_request`] to match it.
    /// A random id per call would make the user approve the same
    /// operation again on every retry.
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
/// The SHA-1 digest is deterministic, which is the whole point (see
/// [`ApprovalGate::check`]), and one-way, so the description never
/// becomes readable from the id.
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
    pub const COUNTER_RESET: &str = "CounterReset";
}
