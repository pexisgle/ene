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

pub(crate) const ERASED_MARKER: &str = "[erased]";

const MARKER_PASS_BOUND: usize = 8;

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
