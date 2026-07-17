//! Memory Journal presenter: browse rows, action gating, and recall debug mapping.

use ene_store::{MemoryItem, MemoryStatus};

use crate::settings::{MemoryJournalRecallRow, MemoryJournalRow};

/// User-facing journal actions mapped to store APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryJournalAction {
    /// Pin a memory against natural decay.
    Pin,
    /// Remove pin.
    Unpin,
    /// Archive a faded memory.
    Archive,
    /// User-driven forget (`Active` → `UserDeleted`).
    Forget,
    /// Mark as disputed.
    Dispute,
    /// User-driven restore to active.
    Restore,
}

/// Business rules for the desktop memory journal.
pub struct MemoryJournalPresenter;

impl MemoryJournalPresenter {
    /// Build a browse-mode row from a typed memory item.
    pub fn row_from_item(item: &MemoryItem) -> MemoryJournalRow {
        MemoryJournalRow {
            id: item.id.unwrap_or_default(),
            title: item.title.clone(),
            kind: item.kind.as_str().to_string(),
            status: item.status.as_str().to_string(),
            confidence: item.confidence.get(),
            salience: item.salience.get(),
            last_accessed: item.last_accessed_at.map(|ts| ts.to_rfc3339()),
            source_metadata: Self::browse_metadata(item),
            pinned: item.pinned,
            scope: item.scope.as_str().to_string(),
            available_actions: Self::available_actions(item.status, item.pinned),
        }
    }

    /// Browse-mode metadata (source and access count).
    pub fn browse_metadata(item: &MemoryItem) -> String {
        format!(
            "source={} access_count={}",
            item.source.as_str(),
            item.access_count
        )
    }

    /// Actions that should succeed for the given lifecycle state.
    pub fn available_actions(status: MemoryStatus, pinned: bool) -> Vec<MemoryJournalAction> {
        let mut actions = Vec::new();
        if pinned {
            actions.push(MemoryJournalAction::Unpin);
        } else {
            actions.push(MemoryJournalAction::Pin);
        }

        match status {
            MemoryStatus::Active => {
                actions.push(MemoryJournalAction::Forget);
                actions.push(MemoryJournalAction::Dispute);
            }
            MemoryStatus::Faded => {
                actions.push(MemoryJournalAction::Archive);
                actions.push(MemoryJournalAction::Dispute);
                actions.push(MemoryJournalAction::Restore);
            }
            MemoryStatus::Archived
            | MemoryStatus::UserDeleted
            | MemoryStatus::Superseded
            | MemoryStatus::Disputed => {
                actions.push(MemoryJournalAction::Restore);
            }
        }

        actions
    }

    /// Build a recall-debug row from pre-mapped display fields.
    pub fn recall_row(
        id: i64,
        title: impl Into<String>,
        reason: impl Into<String>,
        score_summary: impl Into<String>,
    ) -> MemoryJournalRecallRow {
        MemoryJournalRecallRow {
            id,
            title: title.into(),
            reason: reason.into(),
            score_summary: score_summary.into(),
        }
    }
}

impl MemoryJournalAction {
    /// Fluent i18n key for this action label.
    pub const fn i18n_key(self) -> &'static str {
        match self {
            Self::Pin => "memory-journal-action-pin",
            Self::Unpin => "memory-journal-action-unpin",
            Self::Archive => "memory-journal-action-archive",
            Self::Forget => "memory-journal-action-forget",
            Self::Dispute => "memory-journal-action-dispute",
            Self::Restore => "memory-journal-action-restore",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_row_allows_forget_not_archive() {
        let actions = MemoryJournalPresenter::available_actions(MemoryStatus::Active, false);
        assert!(actions.contains(&MemoryJournalAction::Forget));
        assert!(!actions.contains(&MemoryJournalAction::Archive));
        assert!(!actions.contains(&MemoryJournalAction::Restore));
    }

    #[test]
    fn faded_row_allows_archive_and_restore() {
        let actions = MemoryJournalPresenter::available_actions(MemoryStatus::Faded, false);
        assert!(actions.contains(&MemoryJournalAction::Archive));
        assert!(actions.contains(&MemoryJournalAction::Restore));
    }

    #[test]
    fn pinned_row_shows_unpin() {
        let actions = MemoryJournalPresenter::available_actions(MemoryStatus::Active, true);
        assert!(actions.contains(&MemoryJournalAction::Unpin));
        assert!(!actions.contains(&MemoryJournalAction::Pin));
    }
}
