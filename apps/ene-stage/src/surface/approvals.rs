//! Tool approval modal state. Rendering lives in the Chat Slint surface.

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub id: String,
    pub tool: String,
    pub target: String,
}
