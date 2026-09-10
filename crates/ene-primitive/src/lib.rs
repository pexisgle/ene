//! Shared opaque primitives: identity shape, monotonic order, wall clock, and
//! directed correspondence.
//!
//! This crate is not a semantic owner: it holds no domain types, no wire
//! DTOs, and no storage or OS dependencies. Downstream crates wrap these
//! shapes in domain newtypes (`CompanionId`, `TaskRevision`,
//! `PresenceGeneration`, ...) and never convert between those newtypes.

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
