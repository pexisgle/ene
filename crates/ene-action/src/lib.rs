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
//! The crate owns no Task or Permission types: the delegation, relied TaskRef,
//! and workspace association arrive as owner-defined opaque values
//! ([`RawId`](ene_primitive::RawId) / [`RevisionInner`](ene_primitive::RevisionInner)),
//! mapped at the Host composition root. Result adoption, Task completion,
//! retries (`prior_unknown`), delete/execute operations, and the general
//! Action permission evaluation are deliberately absent: each arrives with
//! the producer that owns it.

mod attempt;
mod filesystem;
mod run;

pub use attempt::{
    ActionAttemptId, ActionAttemptRecord, ActionAttemptRepository, ActionCertainty,
    ActionStartOutcome, ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome,
    EffectGrounds, OperationKind, RealTargetRef, certainty_grounds_pair_is_valid,
};
pub use filesystem::{
    MAX_ACTION_FILE_BYTES, ObservedEffect, TargetRejection, WorkspaceRoot, WorkspaceRootError,
};
pub use run::{
    ActionNotStarted, ActionRunOutcome, WorkspaceActionCommand, orchestrate_workspace_action,
};
