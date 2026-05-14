/// パーミッションレベル（Phase 1: 自動承認）
#[derive(Debug, Clone)]
pub enum PermissionLevel {
    Allow,
    RequiresApproval { action: String, target: String },
    Deny { reason: String },
}

/// パーミッション要求（Phase 2 で UI 連携用）
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: uuid::Uuid,
    pub level: PermissionLevel,
    pub description: String,
}

/// Phase 1: すべて自動承認
pub struct PermissionGate;

impl PermissionGate {
    pub fn check(_request: &PermissionRequest) -> PermissionLevel {
        // Phase 1: すべて自動承認
        PermissionLevel::Allow
    }
}
