//! Preservation owner boundary: canonical erasure-condition state.
//!
//! This crate is the semantic owner of the erasure identities other owners
//! must not invent for themselves. It holds only what the Stage 4
//! erasure-currentness foundation needs: the identity of one deletion
//! operation, its sweep generation, and the condition the two identify. The
//! durable canonical store is Group J `erasure_condition` /
//! `erasure_condition_source`, implemented by `ene-store`; consumers of a
//! refusal (inference, task) carry only their own receiver-owned opaque
//! `RawId` correlation and never import these newtypes.
//!
//! Stage 4 has no user-facing deletion-operation producer, so the canonical
//! store is normally empty. "No covering condition" is the result of reading
//! that store — an authoritative empty set — never a sentinel such as
//! `none()`, sweep `0`, or an "always current" default. Full Targeted
//! Deletion (request intake, operation lifecycle, sweep, participant
//! coordination, local erasure, delayed-arrival collection, remainder
//! verification, finalizing, global completion, audit, backup/restore,
//! retention) stays in Stage 6 and extends the same canonical tables; it does
//! not create a second currentness registry.
//!
//! The validity interval of an [`ErasureConditionRef`] is not represented
//! yet: the operation lifecycle that opens and closes it arrives with the
//! Stage 6 producer, so every durable condition row is currently active.
//! Stage 6 adds the interval/closure columns to the same canonical tables
//! and narrows the read there.

use ene_primitive::RawId;

/// Identity of one deletion operation. Wraps [`RawId`]; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionOperationId(RawId);

impl DeletionOperationId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

/// Monotonic sweep generation within one deletion operation.
///
/// The generation is part of the condition identity: a later sweep of the
/// same operation is a different condition, and a generation number from one
/// operation is never compared with another operation's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionSweepGeneration(u64);

impl DeletionSweepGeneration {
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// One canonical erasure condition: the constraint a durable deletion
/// operation places on data use and re-preservation.
///
/// A condition covers the canonical source identities recorded against it.
/// The coverage decision is mechanical: a send whose logical input names a
/// covered source is held, and the consumer of the decision does not
/// interpret deletion semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErasureConditionRef {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
}

#[cfg(test)]
mod tests {
    use super::{DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef};
    use ene_primitive::RawId;

    #[test]
    fn identity_round_trips_and_conditions_keep_their_pair() {
        let operation = DeletionOperationId::from_raw(RawId::new());
        assert_eq!(DeletionOperationId::from_raw(operation.as_raw()), operation);
        assert_eq!(
            DeletionSweepGeneration::from_u64(3).as_u64(),
            3,
            "the sweep generation round-trips"
        );
        let first = ErasureConditionRef {
            operation,
            sweep: DeletionSweepGeneration::from_u64(1),
        };
        let second = ErasureConditionRef {
            operation,
            sweep: DeletionSweepGeneration::from_u64(2),
        };
        assert_ne!(
            first, second,
            "a later sweep is a different condition of the same operation"
        );
    }
}
