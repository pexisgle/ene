use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One activation of a profile row. Never reused after disable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FiberUid(Uuid);

impl FiberUid {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for FiberUid {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FiberUid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Fiber lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberState {
    Inactive,
    Loading,
    Active,
    Unloading,
    Failed,
}

/// Reversible host-context mutation. Inverse runs LIFO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    RegisterTool { name: String },
    BrokerGrant { op: String },
    SpawnProcess { pid: u32 },
}

/// One profile-row activation.
#[derive(Debug, Clone)]
pub struct Fiber {
    pub row_id: String,
    pub uid: FiberUid,
    pub plugin: String,
    pub state: FiberState,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
    pub dispose: Vec<Effect>,
    pub sandbox_required: bool,
}

impl Fiber {
    #[must_use]
    pub fn new(row_id: impl Into<String>, plugin: impl Into<String>) -> Self {
        Self {
            row_id: row_id.into(),
            uid: FiberUid::new(),
            plugin: plugin.into(),
            state: FiberState::Inactive,
            provides: Vec::new(),
            requires: Vec::new(),
            dispose: Vec::new(),
            sandbox_required: true,
        }
    }

    pub fn push_effect(&mut self, effect: Effect) {
        self.dispose.push(effect);
    }
}
