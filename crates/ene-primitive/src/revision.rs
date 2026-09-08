//! Monotonic ordering helper for one identity's content sequence.
//!
//! [`RevisionInner`] tracks that an owner judged the content behind an
//! identity to have changed. It is a helper only: downstream crates wrap it in
//! domain newtypes such as `TaskRevision`, always carried with their identity.

use serde::{Deserialize, Serialize};

/// Monotonic content order for a single identity, as decided by its owner.
///
/// Never carry a [`RevisionInner`] alone: it is meaningful only with the
/// identity it belongs to, and comparisons are meaningful only within the same
/// `(identity, owner)` pair. A larger value never proves newness across
/// different identities or owners.
///
/// [`Self::first`] is the smallest value in a sequence. That is a crate-local
/// starting point, not a reserved sentinel: each domain newtype chooses which
/// value it exposes first.
///
/// ```
/// use ene_primitive::revision::RevisionInner;
///
/// let first = RevisionInner::first();
/// assert_eq!(first.checked_next(), Some(RevisionInner::from_u64(1)));
/// assert_eq!(RevisionInner::from_u64(u64::MAX).checked_next(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RevisionInner(u64);

impl RevisionInner {
    /// Smallest value in a sequence.
    ///
    /// ```
    /// use ene_primitive::revision::RevisionInner;
    ///
    /// assert_eq!(RevisionInner::first().as_u64(), 0);
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
    /// A new revision must be distinguishable from the previous one for
    /// compare-before-commit to mean anything, and a silent `MAX -> MAX`
    /// step would break exactly that.
    ///
    /// ```
    /// use ene_primitive::revision::RevisionInner;
    ///
    /// let max = RevisionInner::from_u64(u64::MAX);
    /// assert_eq!(max.checked_next(), None);
    /// ```
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    /// Reconstitutes a stored value.
    ///
    /// Reconstitute only alongside the identity this revision belongs to; a
    /// bare number on its own says nothing about which content it orders.
    ///
    /// ```
    /// use ene_primitive::revision::RevisionInner;
    ///
    /// assert_eq!(RevisionInner::from_u64(41).as_u64(), 41);
    /// ```
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Returns the stored value for persistence or boundary tokens.
    ///
    /// The value travels only inside its `(identity, revision)` pair; alone it
    /// is never evidence of newness.
    ///
    /// ```
    /// use ene_primitive::revision::RevisionInner;
    ///
    /// assert_eq!(RevisionInner::from_u64(1).as_u64(), 1);
    /// ```
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
