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
