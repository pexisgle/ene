//! Companion commitment ledger domain model — re-exported from `ene-core`.
//!
//! Pure domain vocabulary for tracked promises/tasks. This module stays so
//! existing `ene_store::commitment::*` / `ene_store::Commitment`-style import
//! paths keep working unchanged.

pub use ene_core::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
