//! Approval plane, append-only audit hash chain, and credential vault (W2).

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

mod ai;
mod audit;
mod config;
mod error;
mod plane;
mod policy;
mod popup;
mod request;
mod risk;
mod vault;

pub use ai::{AiJudgement, ApproveModel, ScriptedAi};
pub use audit::{AuditError, AuditLog, AuditRecord};
pub use config::{ApprovalMode, ApprovalSettings, PopupSettings};
pub use error::PlaneError;
pub use plane::{ApprovalPlane, Decision};
pub use policy::{PolicyDecision, PolicyFile, PolicyRule};
pub use popup::{PopupDecision, PopupSink, ScriptedPopup};
pub use request::{AuthzRequest, Sensitivity};
pub use risk::Risk;
pub use vault::{InjectRef, Vault, VaultError};

#[cfg(test)]
mod tests;
