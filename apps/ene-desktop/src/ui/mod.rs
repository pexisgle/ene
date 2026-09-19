//! GUI view-model: pages, IME composer, and the testable desktop runtime.
//!
//! Slint is a projection. Domain and secrets do not live in generated UI.

mod memory;
mod runtime;

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
    Settings,
    About,
    Confirm,
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

/// Chat input plus IME composition. Composition is never sent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Composer {
    draft: String,
    composing: bool,
}

impl Composer {
    pub fn set_draft(&mut self, text: String) {
        if !self.composing {
            self.draft = text;
        }
    }

    pub fn begin_composition(&mut self) {
        self.composing = true;
    }

    pub fn end_composition(&mut self, committed: Option<String>) {
        self.composing = false;
        if let Some(text) = committed {
            self.draft = text;
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
        self.draft.clear();
        Some(taken)
    }

    pub fn wipe(&mut self) {
        self.draft.clear();
        self.composing = false;
    }

    #[must_use]
    pub fn draft(&self) -> &str {
        &self.draft
    }

    #[must_use]
    pub fn composing(&self) -> bool {
        self.composing
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
}
