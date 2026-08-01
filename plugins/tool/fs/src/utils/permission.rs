use crate::utils::sandbox::SandboxConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionLevel {
    Allow,
    RequiresApproval { action: String, target: String },
    Deny { reason: String },
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructiveAction {
    FileDelete,
    FileOverwrite,
    ShellCommand,
    BrowserNavigation,
    AppInteraction,
}

pub struct PermissionGate {
    auto_approve: bool,
}

impl PermissionGate {
    pub const fn new(auto_approve: bool) -> Self {
        Self { auto_approve }
    }

    /// Build a `PermissionGate` whose behavior reflects the sandbox state.
    ///
    /// Fail-closed: destructive operations still require explicit approval
    /// when the sandbox is disabled. Auto-approving whenever the sandbox is
    /// off would silently let shell, file delete, and similar destructive
    /// actions run without human consent exactly when no other boundary
    /// enforces them — the gate exists to ask the human in that case.
    ///
    /// The `_sandbox` argument is intentionally unused: keeping it in the
    /// signature preserves the call sites that pass a sandbox handle
    /// (so a future change can re-introduce sandbox-aware policy without
    /// touching every caller) and documents intent.
    pub const fn default_with_sandbox(_sandbox: &SandboxConfig) -> Self {
        Self {
            auto_approve: false,
        }
    }

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
}

impl Default for PermissionGate {
    /// Fail-closed default: a `PermissionGate::default()` requires approval
    /// for destructive ops. Callers that intentionally want auto-approval
    /// must opt in via [`PermissionGate::new`] with `true` or
    /// [`PermissionGate::default_with_sandbox`] (which is also
    /// fail-closed — see its rustdoc for rationale).
    fn default() -> Self {
        Self {
            auto_approve: false,
        }
    }
}
