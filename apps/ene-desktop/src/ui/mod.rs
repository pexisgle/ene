//! GUI view-model: pages, IME composer, and the testable desktop runtime.
//!
//! Slint is a projection. Domain and secrets do not live in generated UI.

mod deletion;
mod memory;
mod runtime;
mod tasks;
mod usage;

use ene_api::v1::round::HistoryItem;
use ene_client::error::ClientError;

pub use memory::{MemoryPage, MemoryRevisionRow, MemoryRow};
pub use runtime::DesktopRuntime;

/// Visible page. Management remains reachable without Body.
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

/// Fresh-data-dir wizard. Consent is not stored here.
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

/// Chat input plus IME composition and a bounded undo stack. Composition is
/// never sent. Undo is a GUI copy of draft text and is wiped with InputDraft.
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

    /// Restores the previous committed draft. No-op while composing.
    pub fn undo(&mut self) {
        if self.composing {
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.draft = previous;
        }
    }

    /// Returns the committed draft when IME is not composing.
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

/// Serializable GUI projection. Must never contain a raw secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GuiSnapshot {
    pub locale: String,
    pub page: String,
    pub wizard_step: String,
    pub timeline: Vec<String>,
    pub history: Vec<String>,
    pub draft: String,
    pub composing: bool,
    pub connection: String,
    pub presence: String,
    pub deny_reason: String,
    pub challenge_target: Option<String>,
    pub tasks: Vec<String>,
    pub task_detail: String,
    pub about_slint: bool,
    pub body_status: String,
    pub ui_ticks: u64,
    pub setup_ready: bool,
    pub credential_present: bool,
    pub consent_assigned: bool,
    pub secret_visible: bool,
    pub wizard_body: String,
    pub memories: Vec<MemoryRow>,
    pub memory_revisions: Vec<MemoryRevisionRow>,
    pub memory_next: Option<String>,
    pub memory_revisions_of: Option<String>,
    pub memory_revisions_next: Option<u64>,
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
    #[error("confirmation seat is occupied")]
    SeatOccupied,
    #[error("denied by the control boundary")]
    DeniedByBoundary,
    #[error("{0}")]
    Protocol(String),
    #[error(transparent)]
    Client(#[from] ClientError),
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

#[cfg(test)]
mod tests {
    use super::Composer;

    #[test]
    fn ime_composition_does_not_send() {
        let mut composer = Composer::default();
        composer.set_draft(String::from("こんにちは"));
        composer.begin_composition();
        assert!(composer.take_sendable().is_none());
        composer.end_composition(Some(String::from("こんにちは")));
        assert_eq!(composer.take_sendable().as_deref(), Some("こんにちは"));
    }

    #[test]
    fn wipe_clears_draft_ime_and_undo() {
        let mut composer = Composer::default();
        composer.set_draft(String::from("one"));
        composer.set_draft(String::from("two"));
        assert!(composer.undo_len() > 0);
        composer.begin_composition();
        composer.wipe();
        assert!(composer.draft().is_empty());
        assert!(!composer.composing());
        assert_eq!(composer.undo_len(), 0);
    }
}
