//! Action execution: the durable attempt boundary and the minimal
//! Workspace-contained filesystem boundary (E-1/E-2, K-H, AU5).
//!
//! One [`ActionAttemptId`] is one logical try. [`orchestrate_workspace_action`]
//! resolves the requested path inside the current workspace folder, claims the
//! attempt through [`ActionAttemptRepository`] (the start linearization point
//! against steering and an association move), executes only on
//! [`ActionStartOutcome::Started`], and records what the executor itself
//! observed. Attempts start as [`ActionCertainty::Unknown`]; an effect the
//! executor cannot verify stays unknown, and an agent's self-report is never a
//! ground.
//!
//! The durable attempt boundary owns no Task or Permission types: the
//! delegation, relied TaskRef, workspace association, and relied evaluation
//! arrive as owner-defined opaque values
//! ([`RawId`](ene_primitive::RawId) / [`RevisionInner`](ene_primitive::RevisionInner)).
//! The Permission-owned live decision is taken only at the orchestration
//! boundary and reduced to its opaque raw identity before the claim; the
//! meaning and single-use tracking of the evaluation stay with the Permission
//! owner. Result adoption, Task completion, retries (`prior_unknown`),
//! delete/execute operations, and the general Action permission evaluation are
//! deliberately absent: each arrives with the producer that owns it.

mod attempt;
mod filesystem;
mod run;

pub use attempt::{
    ActionAttemptId, ActionAttemptRecord, ActionAttemptRepository, ActionCertainty,
    ActionStartOutcome, ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome,
    EffectGrounds, OperationKind, RealTargetRef, certainty_grounds_pair_is_valid,
};
pub use filesystem::{
    ActionOutput, ListEntry, ListEntryKind, ObservedEffect, TargetRejection, WorkspaceRoot,
    WorkspaceRootError,
};
pub use run::{
    ActionNotStarted, ActionRunOutcome, WorkspaceActionCommand, orchestrate_workspace_action,
};
