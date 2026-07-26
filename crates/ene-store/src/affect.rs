//! Affect state domain model — re-exported from `ene-core` (#270).
//!
//! `AffectState` is core cognitive-domain vocabulary, not a persistence
//! concern; its definition now lives in `ene-core` so that `ene-mind` does
//! not need to depend on this persistence crate to name it. This module
//! stays so existing `ene_store::affect::*` / `ene_store::AffectState`
//! import paths keep working unchanged.

pub use ene_core::{AffectState, DiscreteEmotion, PendingAffectProposal};
