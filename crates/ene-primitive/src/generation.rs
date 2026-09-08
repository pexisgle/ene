//! Monotonic ordering helper for lifecycle intervals.
//!
//! [`GenerationInner`] tracks which interval of a lifecycle (presence
//! attribution, restore, deletion sweep, ...) a fact belongs to. It is a
//! helper only: downstream crates wrap it in domain newtypes such as
//! `PresenceGeneration`, always carried with their lifecycle.
//!
//! This module intentionally duplicates the shape of the `revision` module
//! instead of sharing it. A revision answers "which content order?" while a
//! generation answers "which interval?"; sharing a type, a `From` conversion,
//! a comparison, or a common trait between the two would let one stand in for
//! the other, which correspondence-identity §4.2–§4.3 forbids.

use serde::{Deserialize, Serialize};

/// Monotonic interval order for a single lifecycle.
///
/// A generation marks that a lifecycle was cut over or re-established. It says
/// which interval a fact belongs to, not how new a value is, and comparisons
/// are meaningful only within the same lifecycle. Never compare generations
/// across different lifecycles, and never substitute a generation for a
/// [`crate::revision::RevisionInner`] or vice versa.
///
/// [`Self::first`] is the smallest value in a sequence. That is a crate-local
/// starting point, not a reserved sentinel: each domain newtype chooses which
/// value it exposes first.
///
/// ```
/// use ene_primitive::generation::GenerationInner;
///
/// let first = GenerationInner::first();
/// assert_eq!(first.checked_next(), Some(GenerationInner::from_u64(1)));
/// assert_eq!(GenerationInner::from_u64(u64::MAX).checked_next(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GenerationInner(u64);

impl GenerationInner {
    /// Smallest value in a sequence.
    ///
    /// ```
    /// use ene_primitive::generation::GenerationInner;
    ///
    /// assert_eq!(GenerationInner::first().as_u64(), 0);
    /// ```
    #[must_use]
    pub fn first() -> Self {
        Self(0)
    }

    /// Successor in the sequence, or [`None`] when no further distinct value
    /// exists.
    ///
    /// Exhaustion is reported, never hidden: at [`u64::MAX`] there is no
    /// successor, so this returns [`None`] instead of aliasing the maximum.
    /// A new lifecycle interval must be distinguishable from the previous
    /// one; a silent `MAX -> MAX` step would merge two intervals into one
    /// generation.
    ///
    /// ```
    /// use ene_primitive::generation::GenerationInner;
    ///
    /// let max = GenerationInner::from_u64(u64::MAX);
    /// assert_eq!(max.checked_next(), None);
    /// ```
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    /// Reconstitutes a stored value.
    ///
    /// Reconstitute only alongside the lifecycle this generation belongs to; a
    /// bare number on its own says nothing about which interval it marks.
    ///
    /// ```
    /// use ene_primitive::generation::GenerationInner;
    ///
    /// assert_eq!(GenerationInner::from_u64(7).as_u64(), 7);
    /// ```
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Returns the stored value for persistence or boundary tokens.
    ///
    /// The value travels only inside its `(lifecycle, generation)` pair; alone
    /// it is never evidence of currentness.
    ///
    /// ```
    /// use ene_primitive::generation::GenerationInner;
    ///
    /// assert_eq!(GenerationInner::from_u64(1).as_u64(), 1);
    /// ```
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
