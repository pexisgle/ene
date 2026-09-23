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
//! (IPC §18.1) is the sole production admission premise, and it needs the
//! durable staged request plus its durable confirmation fact. Participant-local erasure
//! implementations and delayed-arrival acceptance gates belong to other
//! slices and crates. Global completion is a sealed transition whose premise
//! is re-derived from the canonical store ([`DeletionCompletionSummary`] plus
//! a system-wide mechanical remainder verification); no caller boolean,
//! Client payload, or LLM output can declare it. This boundary never depends
//! on a concrete participant crate: the Host composition registers
//! [`ErasureParticipant`] implementations and performs the fan-out.

mod operation;
mod request;
pub use operation::*;
pub use request::*;

mod participant;
pub use participant::*;

mod completion;
pub use completion::*;

use ene_primitive::RawId;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErasureConditionRef {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
}
