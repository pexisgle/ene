use ene_rag::decay::{ARCHIVE_THRESHOLD, FADE_THRESHOLD};
use ene_store::{MemoryItem, MemoryStatus};

use crate::settings::{MemoryJournalRecallRow, MemoryJournalRow};

/// User-facing journal actions mapped to store APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryJournalAction {
    /// Pin a memory against natural decay.
    Pin,
    Unpin,
    Archive,
    /// User-driven forget (`Active` → `UserDeleted`).
    Forget,
    Dispute,
    /// User-driven restore to active.
    Restore,
}

/// Business rules for the desktop memory journal.
pub struct MemoryJournalPresenter;

impl MemoryJournalPresenter {
    /// Builds a browse-mode row.
    pub fn row_from_item(item: &MemoryItem) -> MemoryJournalRow {
        MemoryJournalRow {
            id: item.id.unwrap_or_default(),
            title: item.title.clone(),
            kind: item.kind.as_str().to_string(),
            status: item.status,
            confidence: item.confidence.get(),
            salience: item.salience.get(),
            last_accessed: item.last_accessed_at.map(|ts| ts.to_rfc3339()),
            source_metadata: Self::browse_metadata(item),
            pinned: item.pinned,
            scope: item.scope.as_str().to_string(),
            available_actions: Self::available_actions(item.status, item.pinned),
        }
    }

    pub fn browse_metadata(item: &MemoryItem) -> String {
        format!(
            "source={} access_count={}",
            item.source.as_str(),
            item.access_count
        )
    }

    pub fn available_actions(status: MemoryStatus, pinned: bool) -> Vec<MemoryJournalAction> {
        let mut actions = Vec::new();
        actions.push(if pinned {
            MemoryJournalAction::Unpin
        } else {
            MemoryJournalAction::Pin
        });
        for action in [
            MemoryJournalAction::Archive,
            MemoryJournalAction::Forget,
            MemoryJournalAction::Dispute,
            MemoryJournalAction::Restore,
        ] {
            if let Some(target) = action.target_status()
                && Self::transition_allowed(status, target)
            {
                actions.push(action);
            }
        }
        actions
    }

    /// Gate on the store's lifecycle validators so UI actions cannot drift
    /// from persistence policy.
    fn transition_allowed(from: MemoryStatus, to: MemoryStatus) -> bool {
        ene_store::validate_transition(from, to).is_ok()
            || (to == MemoryStatus::Active && ene_store::validate_user_restore(from).is_ok())
    }

    /// Natural next status under decay, or `None` when decay does not apply
    /// (pinned rows are exempt and non-Active/Faded statuses are terminal).
    /// The decay score is bounded by `1.0` and decays to zero with age, so an
    /// unpinned Active/Faded row reaches its threshold eventually.
    pub fn next_natural_transition(status: MemoryStatus, pinned: bool) -> Option<MemoryStatus> {
        if pinned {
            return None;
        }
        match status {
            MemoryStatus::Active => Some(MemoryStatus::Faded),
            MemoryStatus::Faded => Some(MemoryStatus::Archived),
            _ => None,
        }
    }

    /// Decay-score threshold the natural next transition keys off, when any.
    pub fn next_transition_threshold(target: MemoryStatus) -> Option<f32> {
        match target {
            MemoryStatus::Faded => Some(FADE_THRESHOLD),
            MemoryStatus::Archived => Some(ARCHIVE_THRESHOLD),
            _ => None,
        }
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
    /// Store status this action transitions to; `None` for pin toggles.
    pub const fn target_status(self) -> Option<MemoryStatus> {
        match self {
            Self::Pin | Self::Unpin => None,
            Self::Archive => Some(MemoryStatus::Archived),
            Self::Forget => Some(MemoryStatus::UserDeleted),
            Self::Dispute => Some(MemoryStatus::Disputed),
            Self::Restore => Some(MemoryStatus::Active),
        }
    }

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

    const ALL_STATUSES: &[MemoryStatus] = &[
        MemoryStatus::Active,
        MemoryStatus::Faded,
        MemoryStatus::Archived,
        MemoryStatus::UserDeleted,
        MemoryStatus::Superseded,
        MemoryStatus::Disputed,
    ];

    const STATUS_ACTIONS: &[MemoryJournalAction] = &[
        MemoryJournalAction::Archive,
        MemoryJournalAction::Forget,
        MemoryJournalAction::Dispute,
        MemoryJournalAction::Restore,
    ];

    fn store_legal(from: MemoryStatus, to: MemoryStatus) -> bool {
        ene_store::validate_transition(from, to).is_ok()
            || (to == MemoryStatus::Active && ene_store::validate_user_restore(from).is_ok())
    }

    fn status_actions(actions: &[MemoryJournalAction]) -> Vec<MemoryJournalAction> {
        actions
            .iter()
            .copied()
            .filter(|a| a.target_status().is_some())
            .collect()
    }

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

    #[test]
    fn offered_actions_pass_store_validators() {
        for &status in ALL_STATUSES {
            let offered = MemoryJournalPresenter::available_actions(status, false);
            for action in status_actions(&offered) {
                let target = action.target_status().expect("status action has target");
                assert!(
                    store_legal(status, target),
                    "{status:?} offers {action:?} -> {target:?}"
                );
            }
        }
    }

    #[test]
    fn hidden_actions_fail_store_validators() {
        for &status in ALL_STATUSES {
            let offered = MemoryJournalPresenter::available_actions(status, false);
            for action in STATUS_ACTIONS {
                if offered.contains(action) {
                    continue;
                }
                let target = action.target_status().expect("status action has target");
                assert!(
                    !store_legal(status, target),
                    "{status:?} hides {action:?} -> {target:?}"
                );
            }
        }
    }

    #[test]
    fn pin_state_does_not_change_status_actions() {
        for &status in ALL_STATUSES {
            let unpinned = MemoryJournalPresenter::available_actions(status, false);
            let pinned = MemoryJournalPresenter::available_actions(status, true);
            assert_eq!(
                status_actions(&pinned),
                status_actions(&unpinned),
                "{status:?}"
            );
            assert!(pinned.contains(&MemoryJournalAction::Unpin));
            assert!(!pinned.contains(&MemoryJournalAction::Pin));
            assert!(unpinned.contains(&MemoryJournalAction::Pin));
            assert!(!unpinned.contains(&MemoryJournalAction::Unpin));
        }
    }

    #[test]
    fn pin_toggles_never_map_to_a_status() {
        for action in [MemoryJournalAction::Pin, MemoryJournalAction::Unpin] {
            assert_eq!(action.target_status(), None);
        }
    }

    #[test]
    fn natural_transition_follows_status_and_pin() {
        for &status in ALL_STATUSES {
            assert_eq!(
                MemoryJournalPresenter::next_natural_transition(status, true),
                None,
                "{status:?} pinned"
            );
        }
        assert_eq!(
            MemoryJournalPresenter::next_natural_transition(MemoryStatus::Active, false),
            Some(MemoryStatus::Faded)
        );
        assert_eq!(
            MemoryJournalPresenter::next_natural_transition(MemoryStatus::Faded, false),
            Some(MemoryStatus::Archived)
        );
        for status in [
            MemoryStatus::Archived,
            MemoryStatus::UserDeleted,
            MemoryStatus::Superseded,
            MemoryStatus::Disputed,
        ] {
            assert_eq!(
                MemoryJournalPresenter::next_natural_transition(status, false),
                None,
                "{status:?}"
            );
        }
    }

    #[test]
    fn transition_thresholds_match_rag_constants() {
        assert_eq!(
            MemoryJournalPresenter::next_transition_threshold(MemoryStatus::Faded),
            Some(0.40)
        );
        assert_eq!(
            MemoryJournalPresenter::next_transition_threshold(MemoryStatus::Archived),
            Some(0.15)
        );
        for status in [
            MemoryStatus::Active,
            MemoryStatus::UserDeleted,
            MemoryStatus::Superseded,
            MemoryStatus::Disputed,
        ] {
            assert_eq!(
                MemoryJournalPresenter::next_transition_threshold(status),
                None
            );
        }
    }
}
