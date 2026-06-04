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

    pub fn default_with_sandbox(sandbox: &SandboxConfig) -> Self {
        Self {
            auto_approve: !sandbox.enabled,
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

        let req = PermissionRequest::new(&format!("{:?}", action), target, description);
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
    fn default() -> Self {
        Self { auto_approve: true }
    }
}
