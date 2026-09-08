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
/// assert_eq!(first.next(), RevisionInner::from_u64(1));
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

    /// Successor in the sequence, saturating at [`u64::MAX`].
    ///
    /// Saturation keeps the order total without wrapping back to
    /// [`Self::first`]: once the maximum is reached, every further step stays
    /// at the maximum rather than aliasing an old revision.
    ///
    /// ```
    /// use ene_primitive::revision::RevisionInner;
    ///
    /// let max = RevisionInner::from_u64(u64::MAX);
    /// assert_eq!(max.next(), max);
    /// ```
    #[must_use]
    pub fn next(&self) -> Self {
        Self(self.0.saturating_add(1))
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
    /// assert_eq!(RevisionInner::first().next().as_u64(), 1);
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
        assert_eq!(first.next().as_u64(), 1);
    }

    #[test]
    fn saturates_instead_of_wrapping() {
        let max = RevisionInner::from_u64(u64::MAX);
        assert_eq!(max.next(), max);
        assert_eq!(max.next().as_u64(), u64::MAX);
    }

    #[test]
    fn reconstitutes_the_stored_value() {
        assert_eq!(RevisionInner::from_u64(41).as_u64(), 41);
    }

    #[test]
    fn orders_within_one_sequence() {
        let first = RevisionInner::first();
        let second = first.next();
        assert!(first < second);
        assert!(second > first);
    }
}
