//! Durable occurrence ledger premise for execution-local Task Agent
//! observations (Stage 6 A4).
//!
//! The Task Agent execution observes Action results as execution-local text
//! and replays them into later turns. That text is never persisted, so its
//! deletion-relevant provenance cannot ride a body column. Instead the
//! execution mints one durable occurrence identity at observation time; the
//! ledger row carries the delegation/execution correlation, the producing
//! Action attempt identity, the workspace/path correlation, and whether the
//! observation reproduced workspace content — never the observation body, a
//! body hash, or any reversible encoding.
//!
//! The occurrence identity joins the ordered `data_use` of every inference
//! attempt claim whose logical input consumed the occurrence, so the existing
//! AU14 claim gate and the deletion admission association can name it without
//! reading any body.

use ene_primitive::{RawId, WallClockWithTz};

use crate::delegation::DelegationId;

/// Durable identity of one execution-local Task Agent observation occurrence.
///
/// One occurrence is one observed Action result (or one pre-attempt refusal)
/// exactly as the execution replayed it. The identity is opaque and is never
/// reused; it names the occurrence, never its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskAgentObservationId(RawId);

impl TaskAgentObservationId {
    /// The opaque raw identity, for cross-boundary correlation.
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    /// Rebuilds the identity from its raw value at a storage or wire boundary.
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    /// Mints one fresh occurrence identity.
    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Premise of one durable observation occurrence.
///
/// The occurrence identity is caller-minted at observation time, consistent
/// with the other Task premises. `attempt` is the producing AU5 Action
/// attempt where one exists; a refusal observation has none. `observed` is the
/// body the execution is about to replay when the observation reproduced
/// workspace content (`read` bytes or a `list` listing): the repository
/// compares it against the canonical current erasure conditions inside the
/// same transaction as the row write and never persists it. A refusal
/// observation carries no body and no producing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentObservationPremise {
    /// Occurrence identity minted by the caller.
    pub observation: TaskAgentObservationId,
    /// Delegation (execution lifetime) the occurrence belongs to.
    pub delegation: DelegationId,
    /// Producing AU5 Action attempt identity, when the occurrence came from
    /// an executed attempt.
    pub attempt: Option<RawId>,
    /// Observed source body, when the occurrence reproduced one. Transient:
    /// checked against the current conditions and never stored.
    pub observed: Option<String>,
    /// When the execution observed the occurrence.
    pub observed_at: WallClockWithTz,
}
