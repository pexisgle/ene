//! Memory forgetting lifecycle: status transitions and decay scoring.
//!
//! Decay scoring and thresholds live in `ene-rag` (the RAG policy layer); this
//! module is `pub(crate)`; [`active_decay_anchor`] is re-exported with
//! `pub(crate) use` so store code keeps its `crate::forgetting::*` paths, and
//! the pure state-machine validators ([`validate_transition`] /
//! [`validate_user_restore`]) are re-exported from the crate root for
//! lifecycle-policy consumers (journal/CLI UX gating). They are lifecycle
//! policy, not scoring policy.

use ene_core::MemoryStatus;

pub(crate) use ene_rag::active_decay_anchor;

/// Error returned when a memory status transition is not allowed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid memory status transition: {from:?} -> {to:?}")]
pub struct InvalidTransition {
    /// Current status.
    pub from: MemoryStatus,
    /// Requested target status.
    pub to: MemoryStatus,
}

/// Statuses that a user may restore to [`MemoryStatus::Active`] via journal/CLI UX.
pub const fn user_restorable_statuses() -> &'static [MemoryStatus] {
    &[
        MemoryStatus::Faded,
        MemoryStatus::Archived,
        MemoryStatus::Disputed,
        MemoryStatus::UserDeleted,
        MemoryStatus::Superseded,
    ]
}

/// Validate whether a user-driven restore to [`MemoryStatus::Active`] is permitted.
pub fn validate_user_restore(from: MemoryStatus) -> Result<(), InvalidTransition> {
    if user_restorable_statuses().contains(&from) {
        Ok(())
    } else {
        Err(InvalidTransition {
            from,
            to: MemoryStatus::Active,
        })
    }
}

/// Validate whether a single-step lifecycle transition is permitted.
pub const fn validate_transition(
    from: MemoryStatus,
    to: MemoryStatus,
) -> Result<(), InvalidTransition> {
    let allowed = matches!(
        (from, to),
        (
            MemoryStatus::Active,
            MemoryStatus::Faded
                | MemoryStatus::Superseded
                | MemoryStatus::UserDeleted
                | MemoryStatus::Disputed
        ) | (
            MemoryStatus::Faded,
            MemoryStatus::Archived | MemoryStatus::Disputed
        )
    );
    if allowed {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_transition_allows_issue_edges() {
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Faded).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Superseded).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::UserDeleted).is_ok());
        assert!(validate_transition(MemoryStatus::Active, MemoryStatus::Disputed).is_ok());
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Archived).is_ok());
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Disputed).is_ok());
    }

    #[test]
    fn validate_transition_rejects_invalid_edges() {
        assert!(validate_transition(MemoryStatus::Faded, MemoryStatus::Active).is_err());
        assert!(validate_transition(MemoryStatus::Archived, MemoryStatus::Faded).is_err());
        assert!(validate_transition(MemoryStatus::UserDeleted, MemoryStatus::Active).is_err());
    }

    #[test]
    fn validate_user_restore_allows_journal_targets() {
        assert!(validate_user_restore(MemoryStatus::Archived).is_ok());
        assert!(validate_user_restore(MemoryStatus::UserDeleted).is_ok());
        assert!(validate_user_restore(MemoryStatus::Faded).is_ok());
        assert!(validate_user_restore(MemoryStatus::Active).is_err());
    }
}
