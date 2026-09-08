//! Shared opaque primitives (identity shape, monotonic order, wall clock).
//!
//! This crate is not a semantic owner: it holds no domain types, no wire
//! DTOs, and no storage or OS dependencies.
//!
//! The five modules below share only shapes: opaque identity ([`RawId`]),
//! monotonic content order ([`RevisionInner`]), lifecycle interval order
//! ([`GenerationInner`]), wall-clock time with its creation offset
//! ([`WallClockWithTz`]), and directed correspondence ([`DirectedPair`]).
//! Downstream crates wrap these in domain newtypes (`CompanionId`,
//! `TaskRevision`, `PresenceGeneration`, ...) and never convert between those
//! newtypes.

pub mod clock;
pub mod correlation;
pub mod generation;
pub mod raw_id;
pub mod revision;

pub use clock::WallClockWithTz;
pub use correlation::{DirectedPair, EmptyPurpose};
pub use generation::GenerationInner;
pub use raw_id::RawId;
pub use revision::RevisionInner;
