//! Monotonic ordering helper for one identity's content sequence.
//!
//! A helper only: downstream crates wrap it in domain newtypes such as
//! `TaskRevision` and always carry it with the identity it orders.

use serde::{Deserialize, Serialize};

/// Monotonic content order for a single identity, as decided by its owner.
///
/// Meaningful only together with that identity, and comparable only within the
/// same `(identity, owner)` pair: a larger value never proves newness across
/// different identities or owners. [`Self::first`] is the smallest value of a
/// sequence, not a reserved sentinel; each domain newtype chooses the value it
/// exposes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RevisionInner(u64);

impl RevisionInner {
    /// Returns the smallest value in a sequence.
    #[must_use]
    pub fn first() -> Self {
        Self(0)
    }

    /// Returns the successor in the sequence, or [`None`] at [`u64::MAX`].
    ///
    /// Exhaustion is reported rather than hidden: a silent `MAX -> MAX` step
    /// would make a new revision indistinguishable from its predecessor and
    /// break compare-before-commit.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::RevisionInner;

    #[test]
    fn starts_at_zero_and_advances_by_one() {
        let first = RevisionInner::first();
        assert_eq!(first.as_u64(), 0);
        assert_eq!(first.checked_next(), Some(RevisionInner::from_u64(1)));
    }

    #[test]
    fn exhaustion_reports_none_instead_of_aliasing_the_maximum() {
        let max = RevisionInner::from_u64(u64::MAX);
        assert_eq!(max.checked_next(), None);
    }

    #[test]
    fn reconstitutes_the_stored_value() {
        assert_eq!(RevisionInner::from_u64(41).as_u64(), 41);
    }

    #[test]
    fn orders_within_one_sequence() {
        let first = RevisionInner::first();
        let Some(second) = first.checked_next() else {
            return;
        };
        assert!(first < second);
        assert!(second > first);
    }
}
