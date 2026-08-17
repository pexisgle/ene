use serde::{Deserialize, Serialize};

ene_config::define_config!(
    settings,
    "approval",
    /// Runtime-true approval mode and popup timeout (P-903).
    pub struct ApprovalSettings {
        pub mode: ApprovalMode,
        pub popup: PopupSettings,
        pub policy_file: String = "policy.json".to_owned(),
        pub audit_db: String = "audit.db".to_owned(),
    }
);

/// Coarse approval mode. Runtime truth is `approval.mode`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum ApprovalMode {
    AskAll,
    #[default]
    Policy,
    AiAuto,
    Auto,
}

/// Popup delivery timeout.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct PopupSettings {
    pub timeout_ms: u64,
}

impl Default for PopupSettings {
    fn default() -> Self {
        Self { timeout_ms: 30_000 }
    }
}
