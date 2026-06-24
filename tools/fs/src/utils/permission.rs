use crate::utils::sandbox::SandboxConfig;

/// Permission level
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionLevel {
    Allow,
    RequiresApproval { action: String, target: String },
    Deny { reason: String },
}

/// Permission request
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: uuid::Uuid,
    pub level: PermissionLevel,
    pub description: String,
}

impl PermissionRequest {
    pub fn new(action: &str, target: &str, description: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            level: PermissionLevel::RequiresApproval {
                action: action.to_string(),
                target: target.to_string(),
            },
            description: description.to_string(),
        }
    }
}

/// Types of destructive operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructiveAction {
    FileDelete,
    FileOverwrite,
    ShellCommand,
    BrowserNavigation,
    AppInteraction,
}

/// Permission gate based on sandbox configuration
pub struct PermissionGate {
    auto_approve: bool,
}

impl PermissionGate {
    pub fn new(auto_approve: bool) -> Self {
        Self { auto_approve }
    }

    /// Build a `PermissionGate` whose behavior reflects the sandbox state.
    ///
    /// Fail-closed: when the sandbox is disabled, destructive operations
    /// still require explicit approval. The previous behavior was
    /// `auto_approve = !sandbox.enabled`, which made a disabled sandbox
    /// silently auto-approve shell, file delete, and similar destructive
    /// actions — i.e. fail-open. The whole point of a permission gate is
    /// to ask the human when the sandbox is *not* there to enforce
    /// boundaries, so the disabled-sandbox branch now keeps
    /// `auto_approve = false`.
    ///
    /// The `_sandbox` argument is intentionally unused: keeping it in the
    /// signature preserves the call sites that pass a sandbox handle
    /// (so a future change can re-introduce sandbox-aware policy without
    /// touching every caller) and documents intent.
    pub fn default_with_sandbox(_sandbox: &SandboxConfig) -> Self {
        Self {
            auto_approve: false,
        }
    }

    /// Checks destructive operations
    pub fn check_destructive(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), PermissionRequest> {
        if self.auto_approve {
            return Ok(());
        }

        let req = PermissionRequest::new(&format!("{action:?}"), target, description);
        Err(req)
    }

    /// Simple permission check
    pub fn check_simple(request: &PermissionRequest) -> PermissionLevel {
        match request.level {
            PermissionLevel::Allow => PermissionLevel::Allow,
            PermissionLevel::RequiresApproval { .. } => PermissionLevel::Deny {
                reason: "Approval UI not yet implemented".to_string(),
            },
            PermissionLevel::Deny { ref reason } => PermissionLevel::Deny {
                reason: reason.clone(),
            },
        }
    }
}

impl Default for PermissionGate {
    /// Fail-closed default: a `PermissionGate::default()` requires approval
    /// for destructive ops. Callers that intentionally want auto-approval
    /// must opt in via [`PermissionGate::new`] with `true` or
    /// [`PermissionGate::default_with_sandbox`] (which is now also
    /// fail-closed — see its rustdoc for rationale).
    fn default() -> Self {
        Self {
            auto_approve: false,
        }
    }
}
