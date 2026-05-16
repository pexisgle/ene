use crate::sandbox::SandboxConfig;

/// パーミッションレベル
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionLevel {
    Allow,
    RequiresApproval { action: String, target: String },
    Deny { reason: String },
}

/// パーミッション要求
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

/// 破壊的操作の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructiveAction {
    FileDelete,
    FileOverwrite,
    ShellCommand,
    BrowserNavigation,
    AppInteraction,
}

/// サンドボックス設定に基づくパーミッションゲート
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

    /// 破壊的操作をチェック
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

    /// シンプルな許可チェック（Phase 1 互換）
    pub fn check_simple(_request: &PermissionRequest) -> PermissionLevel {
        // Phase 1: すべて自動承認
        // Phase 2 で UI 連携を実装
        PermissionLevel::Allow
    }
}

impl Default for PermissionGate {
    fn default() -> Self {
        Self { auto_approve: true }
    }
}
