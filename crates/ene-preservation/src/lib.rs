//! Preservation owner boundary: canonical erasure-condition state.
//!
//! Owns deletion admission, unfinished lifecycle, the first-party request /
//! confirmation surface, the erasure identities consumed by the canonical
//! Group J store in `ene-store`, and the required participant snapshot with
//! its completion vocabulary. Active, held and finalizing operations keep
//! their current condition effective across restart. An empty current set is
//! a database result, never a sentinel or cached gate.
//!
//! The wire intent stages a request only; the trusted Host-local confirmation
//! (IPC §18.1) is the sole production mint site of
//! [`TrustedOwnerConfirmationRef`], and it needs the durable staged request
//! plus its durable confirmation fact. Participant-local erasure
//! implementations, delayed-arrival collection, and verified global completion
//! belong to other slices and crates. This boundary cannot close a condition
//! or declare global completion, and it never depends on a concrete
//! participant crate: the Host composition registers [`ErasureParticipant`]
//! implementations and performs the fan-out.

mod operation;
mod request;
pub use operation::*;
pub use request::*;

mod participant;
pub use participant::*;

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

    /// The participant boundary must stay cross-cutting: this crate owns the
    /// vocabulary and the trait, the composition registers concrete
    /// implementations, and `ene-preservation` never depends on a participant
    /// crate (lifecycle §9, crate-module-decomposition §5).
    #[test]
    fn preservation_depends_on_no_participant_crate() {
        let manifest = include_str!("../Cargo.toml");
        let dependencies = manifest
            .split("[dependencies]")
            .nth(1)
            .expect("the manifest has a dependencies section")
            .split('[')
            .next()
            .expect("the dependencies section has a body");
        for crate_name in [
            "ene-store",
            "ene-companion",
            "ene-learning",
            "ene-task",
            "ene-action",
            "ene-inference",
            "ene-presence",
            "ene-presentation",
            "ene-permission",
            "ene-credential",
            "ene-core",
        ] {
            assert!(
                !dependencies.contains(crate_name),
                "ene-preservation must not depend on {crate_name}"
            );
        }
        assert!(
            dependencies.contains("ene-primitive"),
            "the shared identity primitive stays the one domain dependency"
        );
    }
}
