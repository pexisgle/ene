//! Monotonic ordering helper for lifecycle intervals.
//!
//! [`GenerationInner`] tracks which interval of a lifecycle (presence
//! attribution, restore, deletion sweep, ...) a fact belongs to. A helper
//! only: downstream crates wrap it in domain newtypes such as
//! `PresenceGeneration`, always carried with their lifecycle.
//!
//! This module intentionally duplicates the shape of the `revision` module
//! instead of sharing it: sharing a type, conversion, comparison, or common
//! trait would let a revision and a generation stand in for one another,
//! which correspondence-identity §4.2–§4.3 forbids.

use serde::{Deserialize, Serialize};

/// Monotonic interval order for a single lifecycle.
///
/// A generation marks that a lifecycle was cut over or re-established: it says
/// which interval a fact belongs to, not how new a value is, and comparisons
/// are meaningful only within the same lifecycle. Never compare generations
/// across lifecycles, and never substitute one for a
/// [`crate::revision::RevisionInner`] or vice versa. [`Self::first`] is the
/// smallest value of a sequence, not a reserved sentinel; each domain newtype
/// chooses the value it exposes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GenerationInner(u64);

impl GenerationInner {
    /// Returns the smallest value in a sequence.
    #[must_use]
    pub fn first() -> Self {
        Self(0)
    }

    /// Returns the successor in the sequence, or [`None`] at [`u64::MAX`].
    ///
    /// Exhaustion is reported rather than hidden: a silent `MAX -> MAX` step
    /// would merge two lifecycle intervals into one generation.
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
    use super::GenerationInner;

    #[test]
    fn starts_at_zero_and_advances_by_one() {
        let first = GenerationInner::first();
        assert_eq!(first.as_u64(), 0);
        assert_eq!(first.checked_next(), Some(GenerationInner::from_u64(1)));
    }

    #[test]
    fn exhaustion_reports_none_instead_of_aliasing_the_maximum() {
        let max = GenerationInner::from_u64(u64::MAX);
        assert_eq!(max.checked_next(), None);
    }

    #[test]
    fn reconstitutes_the_stored_value() {
        assert_eq!(GenerationInner::from_u64(7).as_u64(), 7);
    }

    #[test]
    fn orders_within_one_lifecycle() {
        let first = GenerationInner::first();
        let Some(second) = first.checked_next() else {
            return;
        };
        assert!(first < second);
        assert!(second > first);
    }
}
