//! Directed correspondence shape between two identities.
//!
//! [`DirectedPair`] records only that `from_raw` relates to `to_raw` and why,
//! in words. Domain crates define their own correlation structs (with their
//! own revision or generation pins) on top of this shape; the shape itself
//! carries no acceptance, achievement, or permission meaning.

use serde::{Deserialize, Serialize};

use crate::raw_id::RawId;

/// Directed correspondence from one identity to another.
///
/// The pair is ordered: `from_raw` is the source side (for example the
/// derivative) and `to_raw` is the target side (for example the source range).
/// Arrival order, identifier magnitude, and wall-clock time are never the
/// basis of the correspondence; only this explicit pairing is.
///
/// `purpose` is explanatory, never a match key (correspondence-identity §4.6):
/// no logic may branch on its contents, prefix-match it, or treat equal
/// purposes as the same correspondence.
///
/// ```
/// use ene_primitive::correlation::DirectedPair;
/// use ene_primitive::raw_id::RawId;
///
/// let from = RawId::new();
/// let to = RawId::new();
/// let link = DirectedPair::try_new(from, to, String::from("retry supersedes attempt"));
/// assert!(link.is_ok());
/// if let Ok(pair) = link {
///     assert_eq!(pair.from_raw, from);
///     assert_eq!(pair.to_raw, to);
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DirectedPair {
    /// Correspondence source side.
    pub from_raw: RawId,
    /// Correspondence target side.
    pub to_raw: RawId,
    /// Human explanation of why the two are related. Never matched on.
    pub purpose: String,
}

impl DirectedPair {
    /// Links `from_raw` to `to_raw` with an explanatory purpose.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyPurpose`] when `purpose` is empty: a correspondence with
    /// nothing to say about itself explains nothing.
    ///
    /// ```
    /// use ene_primitive::correlation::{DirectedPair, EmptyPurpose};
    /// use ene_primitive::raw_id::RawId;
    ///
    /// assert_eq!(
    ///     DirectedPair::try_new(RawId::new(), RawId::new(), String::new()),
    ///     Err(EmptyPurpose)
    /// );
    /// ```
    pub fn try_new(from_raw: RawId, to_raw: RawId, purpose: String) -> Result<Self, EmptyPurpose> {
        if purpose.is_empty() {
            Err(EmptyPurpose)
        } else {
            Ok(Self {
                from_raw,
                to_raw,
                purpose,
            })
        }
    }
}

/// Rejection reason for a directed pair with no explanatory purpose.
///
/// Returned by [`DirectedPair::try_new`] when `purpose` is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("directed pair purpose must not be empty")]
pub struct EmptyPurpose;

#[cfg(test)]
mod tests {
    use super::DirectedPair;
    use super::EmptyPurpose;
    use crate::raw_id::RawId;

    #[test]
    fn keeps_direction_and_purpose() {
        let from = RawId::new();
        let to = RawId::new();
        let link = DirectedPair::try_new(from, to, String::from("retry supersedes attempt"))
            .expect("non-empty purpose must construct");
        assert_eq!(link.from_raw, from);
        assert_eq!(link.to_raw, to);
        assert_eq!(link.purpose, "retry supersedes attempt");
    }

    #[test]
    fn rejects_an_empty_purpose() {
        assert_eq!(
            DirectedPair::try_new(RawId::new(), RawId::new(), String::new()),
            Err(EmptyPurpose)
        );
    }
}
