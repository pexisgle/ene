//! Built-in local-erasure participants for the durable owners `ene-store`
//! serves.
//!
//! `ene-preservation` owns the [`ErasureParticipant`] contract and never
//! depends on a concrete participant; each owner's durable master lives in one
//! `ene-store` domain module, and this crate implements the participant next to
//! that master so a sweep writes only its own owner's rows. The Host
//! composition registers the implementations and performs the fan-out.
//!
//! [`ErasureParticipant`]: ene_preservation::ErasureParticipant
//!
//! `companion_learning` covers the Companion and Learning owners
//! (`history_message`, `activity_record`, the undelivered references, and the
//! Summary / Memory / revision / token-index surfaces). `task_action_inference`
//! covers the Task, Action, and Inference owners. Owners whose domain crate
//! defines a port instead (`ene-permission`, `ene-credential`, `ene-presence`)
//! implement their participants behind those ports.

mod companion_learning;
mod remainder;
mod task_action_inference;

pub use companion_learning::{
    CompanionErasureParticipant, ERASURE_SCAN_ROWS, LearningErasureParticipant,
};
pub(crate) use remainder::system_remainder;
pub use task_action_inference::{
    ActionErasureParticipant, InferenceErasureParticipant, TaskErasureParticipant,
};

/// The fixed marker a mechanically redacted value keeps in place of the
/// target. It carries no target-derived material and is never treated as a
/// match for a non-overlapping target.
pub(crate) const ERASED_MARKER: &str = "[erased]";

/// Redaction passes bounded before a value that keeps re-matching falls back
/// to outright removal. A target that overlaps the marker itself is the only
/// way to re-match; removal strictly shortens the value, so the fallback
/// always reaches a clean fixpoint.
const MARKER_PASS_BOUND: usize = 8;

/// Mechanically removes every occurrence of `target` from `text`.
///
/// Returns [`None`] when the value contains no occurrence (no write needed).
/// A replacement can join the surrounding text into a new occurrence, so the
/// pass repeats until the value is clean; a target that overlaps the marker
/// itself falls back to outright removal, which strictly shortens the value
/// and therefore always reaches a clean fixpoint. The returned count is the
/// number of occurrences removed.
///
/// This is the one mechanical predicate the A3 owner sweeps and the A4
/// acceptance boundaries share: a body an accepting boundary redacts and a
/// body an owner sweep redacts are erased (or refused) by the same rule.
pub(crate) fn redact_exact(text: &str, target: &str) -> Option<(String, u64)> {
    if target.is_empty() || !text.contains(target) {
        return None;
    }
    let mut removed = count_occurrences(text, target);
    let mut current = text.replace(target, ERASED_MARKER);
    for _ in 0..MARKER_PASS_BOUND {
        if !current.contains(target) {
            return Some((current, removed));
        }
        removed += count_occurrences(&current, target);
        current = current.replace(target, ERASED_MARKER);
    }
    while current.contains(target) {
        removed += count_occurrences(&current, target);
        current = current.replace(target, "");
    }
    Some((current, removed))
}

fn count_occurrences(text: &str, target: &str) -> u64 {
    text.matches(target).count() as u64
}

#[cfg(feature = "test-support")]
pub(crate) use remainder::system_remainder as exact_remainder_probe;
