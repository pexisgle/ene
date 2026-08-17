use thiserror::Error;

/// Approval-plane failures. Audit write failures refuse the operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlaneError {
    #[error("denied {tool}: {reason}")]
    Denied { tool: String, reason: String },
    #[error("audit: {0}")]
    Audit(#[from] crate::audit::AuditError),
    #[error("vault: {0}")]
    Vault(#[from] crate::vault::VaultError),
    #[error("popup timeout")]
    PopupTimeout,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
