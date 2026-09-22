pub(crate) mod deletion;
mod memory;
pub mod presentation;
mod runtime;
pub(crate) mod tasks;
pub(crate) mod usage;

use ene_api::v1::round::HistoryItem;
use ene_client::error::ClientError;

pub use memory::{MemoryPage, MemoryRevisionRow, MemoryRow};
pub use runtime::DesktopRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Wizard,
    Chat,
    History,
    Memory,
    Tasks,
    Settings,
    About,
    Confirm,
    Usage,
    Deletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Language,
    BundledEne,
    CloudCost,
    Credential,
    Assignment,
}

impl WizardStep {
    #[must_use]
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Language => Some(Self::BundledEne),
            Self::BundledEne => Some(Self::CloudCost),
            Self::CloudCost => Some(Self::Credential),
            Self::Credential => Some(Self::Assignment),
            Self::Assignment => None,
        }
    }

    #[must_use]
    pub fn back(self) -> Option<Self> {
        match self {
            Self::Language => None,
            Self::BundledEne => Some(Self::Language),
            Self::CloudCost => Some(Self::BundledEne),
            Self::Credential => Some(Self::CloudCost),
            Self::Assignment => Some(Self::Credential),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Composer {
    draft: String,
    composing: bool,
    undo: Vec<String>,
}

impl Composer {
    const UNDO_CAP: usize = 32;

    pub fn set_draft(&mut self, text: String) {
        if !self.composing {
            if self.draft != text {
                self.push_undo(self.draft.clone());
            }
            self.draft = text;
        }
    }

    pub fn begin_composition(&mut self) {
        self.composing = true;
    }

    pub fn end_composition(&mut self, committed: Option<String>) {
        self.composing = false;
        if let Some(text) = committed {
            if self.draft != text {
                self.push_undo(self.draft.clone());
            }
            self.draft = text;
        }
    }

    pub fn undo(&mut self) {
        if self.composing {
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.draft = previous;
        }
    }

    pub fn take_sendable(&mut self) -> Option<String> {
        if self.composing {
            return None;
        }
        let text = self.draft.trim();
        if text.is_empty() {
            return None;
        }
        let taken = self.draft.clone();
        self.push_undo(taken.clone());
        self.draft.clear();
        Some(taken)
    }

    pub fn wipe(&mut self) {
        self.draft.clear();
        self.composing = false;
        self.undo.clear();
    }

    #[must_use]
    pub fn draft(&self) -> &str {
        &self.draft
    }

    #[must_use]
    pub fn composing(&self) -> bool {
        self.composing
    }

    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    fn push_undo(&mut self, previous: String) {
        if self.undo.len() >= Self::UNDO_CAP {
            self.undo.remove(0);
        }
        self.undo.push(previous);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GuiSnapshot {
    pub locale: String,
    pub page: String,
    pub timeline: Vec<String>,
    pub history: Vec<String>,
    pub draft: String,
    pub composing: bool,
    pub deny_reason: String,
    pub tasks: Vec<String>,
    pub task_detail: String,
    pub body_status: String,
    pub ui_ticks: u64,
    pub setup_ready: bool,
    pub credential_present: bool,
    pub consent_assigned: bool,
    pub secret_visible: bool,
    pub memories: Vec<MemoryRow>,
    pub memory_revisions: Vec<MemoryRevisionRow>,
    pub memory_panel: String,
    pub usage_body: String,
    pub deletion_body: String,
    pub search_draft: String,
}

impl GuiSnapshot {
    #[must_use]
    pub fn contains_secret(&self, secret: &str) -> bool {
        if secret.is_empty() {
            return false;
        }
        let blob = serde_json::to_string(self).unwrap_or_default();
        blob.contains(secret)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("control: {0}")]
    Control(String),
    #[error("host launch: {0}")]
    HostLaunch(String),
    #[error("denied by the control boundary")]
    DeniedByBoundary,
    #[error("{0}")]
    Protocol(String),
    /// The Host could not answer technically; the request changed nothing and
    /// is retryable. Distinct from [`DesktopError::Protocol`], which reports a
    /// peer that violated the shape of the exchange.
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Client(#[from] ClientError),
}

/// Sends one Client request under a caller-chosen budget. The only shared
/// difference between panels is the timeout, so it stays a parameter.
pub(crate) async fn request_with_timeout(
    client: &mut ene_client::Client,
    payload: ene_api::v1::payload::WirePayload,
    timeout: std::time::Duration,
) -> Result<ene_api::v1::payload::WirePayload, DesktopError> {
    tokio::time::timeout(timeout, client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
}

pub(crate) fn history_lines(items: &[HistoryItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            let role = match item.role {
                ene_api::v1::round::HistoryRole::Owner => "owner",
                ene_api::v1::round::HistoryRole::Companion => "companion",
            };
            format!("[{role}] {}", item.text)
        })
        .collect()
}
