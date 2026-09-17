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
mod task_action_inference;

pub use companion_learning::{
    CompanionErasureParticipant, ERASURE_SCAN_ROWS, LearningErasureParticipant,
};
pub use task_action_inference::{
    ActionErasureParticipant, InferenceErasureParticipant, TaskErasureParticipant,
};

#[cfg(feature = "test-support")]
pub(crate) use companion_learning::exact_remainder_probe;
#[cfg(test)]
pub(crate) use task_action_inference::{ERASED_LOCATOR, ROWS_PER_DEMAND};
