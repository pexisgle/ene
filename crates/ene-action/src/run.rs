//! Orchestration of one Workspace-contained filesystem Action.
//!
//! The order is fixed by K-B.1 and AU5: input shape and path resolution
//! happen before any durable claim (a refused or malformed request never
//! leaves an attempt row), the permission-owned live decision is taken for
//! exactly the resolved target, the attempt insert is the start linearization
//! point inside the repository, and the filesystem effect runs only after
//! [`ActionStartOutcome::Started`], outside every transaction.
//!
//! The observed effect is always returned once the attempt started, even when
//! recording the fact fails technically: an external effect must never be
//! hidden by a storage failure. Recording failure leaves the durable row at
//! `Unknown`, which is exactly what it means.
//!
//! This orchestration never adopts the effect into a Task, never completes
//! the Task, and never re-executes an unknown outcome.

use ene_permission::{
    ActionAuthorizationDecision, ActionDenyCode, ActionEvaluationTracker, ActionKind,
    ActionUseCandidate, CurrentActionPremise, authorize_action_use,
};
use ene_primitive::{RawId, RevisionInner};

use crate::attempt::{
    ActionAttemptId, ActionAttemptRepository, ActionCertainty, ActionStartOutcome,
    ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, OperationKind,
    RealTargetRef,
};
use crate::filesystem::{ObservedEffect, TargetRejection, WorkspaceRoot};

/// One start-and-execute request under an existing delegation/workspace
/// premise.
///
/// The correlation values are mapped by the composition root from the Task
/// owner's delegation and current workspace association; `root` is the opened
/// current association folder. `content` is the intended bytes for
/// create/edit and is redacted from [`core::fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct WorkspaceActionCommand {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub root: WorkspaceRoot,
    pub operation: OperationKind,
    pub requested_path: String,
    pub content: Option<Vec<u8>>,
}

impl core::fmt::Debug for WorkspaceActionCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WorkspaceActionCommand")
            .field("delegation", &self.delegation)
            .field("task", &self.task)
            .field("task_revision", &self.task_revision)
            .field("workspace", &self.workspace)
            .field("root", &self.root)
            .field("operation", &self.operation)
            .field("requested_path", &self.requested_path)
            .field(
                "content",
                &self.content.as_ref().map(|bytes| {
                    if bytes.is_empty() {
                        String::from("<0 bytes>")
                    } else {
                        format!("<{} bytes redacted>", bytes.len())
                    }
                }),
            )
            .finish()
    }
}

/// Why one request never started. Every variant means zero durable writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionNotStarted {
    /// The delegation/task/workspace premise is no longer current.
    StalePremise,
    /// The Task is terminal (`Completed` / `Failed`); nothing was claimed or
    /// executed. Unit-style because the Task lifecycle vocabulary stays with
    /// its owner; the Work-side adapter re-reads to carry the detail.
    TaskTerminal,
    /// The delegation's execution already submitted its final result;
    /// nothing was claimed or executed, even while the Task is not terminal.
    ExecutionSealed,
    /// A canonical current erasure condition covers the resolved target;
    /// nothing was claimed and nothing was executed. The caller reports a
    /// data-use hold; a completed operation is not a current condition, so a
    /// fresh target after completion proceeds.
    DataUseHeld,
    /// The requested path was refused before any claim.
    Rejected(TargetRejection),
    /// Create/edit arrived without content.
    MissingContent,
    /// List/read arrived with content.
    ContentNotAllowed,
    /// The permission-owned decision refused this use.
    Denied(ActionDenyCode),
    /// The permission-owned candidate disagrees with the current premise; the
    /// caller reloads current state and rebuilds the request.
    NeedsRevalidation,
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different candidate fingerprint.
    EvaluationConsumed,
}

/// The result of one start-and-execute request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRunOutcome {
    /// The attempt started and its effect was observed.
    Completed {
        attempt: ActionAttemptId,
        /// What the executor observed; the [`core::fmt::Debug`] rendering of
        /// [`ObservedEffect`] redacts content and target paths.
        effect: ObservedEffect,
        /// Whether the observation became durable. `false` means the attempt
        /// row still reads `Unknown` (the effect happened and is reported,
        /// but the fact could not be stored).
        fact_recorded: bool,
    },
    /// Nothing was claimed and nothing was executed.
    NotStarted(ActionNotStarted),
}

/// Starts and executes one Workspace-contained filesystem action.
///
/// Input shape and path resolution are checked first, so a refused request
/// leaves no attempt row. The resolved target then goes through the
/// permission-owned [`authorize_action_use`], whose single-use evaluation is
/// consumed before the durable claim; the claim itself carries the evaluation
/// only as its opaque [`RawId`], mapped at this orchestration boundary. The
/// repository's claim decides start versus [`ActionNotStarted::StalePremise`],
/// and only `Started` executes. The effect is observed from the executor's own
/// operation (read-back for writes), never from an agent self-report, and its
/// certainty is recorded with a compare-and-set from `Unknown`.
pub async fn orchestrate_workspace_action(
    repository: &impl ActionAttemptRepository,
    tracker: &mut ActionEvaluationTracker,
    command: WorkspaceActionCommand,
) -> Result<ActionRunOutcome, ActionTechnicalError> {
    if let Some(not_started) = input_check(&command) {
        return Ok(ActionRunOutcome::NotStarted(not_started));
    }
    let target = match command
        .root
        .resolve(&command.requested_path, command.operation)
    {
        Ok(target) => target,
        Err(rejection) => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(
                rejection,
            )));
        }
    };
    // The live decision binds this operation to the exact resolved target and
    // the current premise; only its single-use evaluation may start the
    // attempt.
    let candidate = ActionUseCandidate {
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
        operation: action_kind(command.operation),
        resolved_target: target.as_path().to_owned(),
    };
    let current = CurrentActionPremise {
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
    };
    let evaluation = match authorize_action_use(&candidate, &current, tracker) {
        ActionAuthorizationDecision::AllowForThisUse(evaluation) => evaluation,
        ActionAuthorizationDecision::Deny(code) => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::Denied(code)));
        }
        ActionAuthorizationDecision::NeedsRevalidation => {
            return Ok(ActionRunOutcome::NotStarted(
                ActionNotStarted::NeedsRevalidation,
            ));
        }
    };
    if !tracker.consume(&evaluation, &candidate) {
        return Ok(ActionRunOutcome::NotStarted(
            ActionNotStarted::EvaluationConsumed,
        ));
    }
    // The Permission-owned decision ends here: the durable claim carries only
    // its opaque raw correlation identity.
    let evaluation_id = evaluation.as_raw();
    let premise = attempt_premise(&command, &target, evaluation_id);
    let attempt = premise.attempt;
    let outcome = repository.insert_attempt_if_current(premise).await?;
    match outcome {
        ActionStartOutcome::Started => {}
        ActionStartOutcome::StalePremise => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::StalePremise));
        }
        ActionStartOutcome::TaskTerminal => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::TaskTerminal));
        }
        ActionStartOutcome::ExecutionSealed => {
            return Ok(ActionRunOutcome::NotStarted(
                ActionNotStarted::ExecutionSealed,
            ));
        }
        ActionStartOutcome::HeldForErasure => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::DataUseHeld));
        }
    }
    // The attempt is durable: execute outside every transaction, exactly the
    // premise that was compared, then observe the effect.
    let effect = command
        .root
        .execute(&target, command.operation, command.content.as_deref());
    let fact_recorded = repository
        .compare_and_set_certainty(
            attempt,
            ActionCertainty::Unknown,
            effect.certainty,
            effect.grounds,
        )
        .await
        .map(|outcome| outcome == CertaintyUpdateOutcome::Updated)
        .unwrap_or(false);
    Ok(ActionRunOutcome::Completed {
        attempt,
        effect,
        fact_recorded,
    })
}

/// The mapping boundary from the Permission-owned live decision to the
/// Action-owned durable premise.
///
/// The caller consumes the Permission-owned evaluation and reduces it to its
/// opaque [`RawId`] at this orchestration boundary; this helper only fills the
/// Action-owned premise.
/// The durable types
/// ([`AttemptCommitPremise`] / [`ActionAttemptRecord`](crate::ActionAttemptRecord))
/// never name the Permission newtype, and the Action repository neither
/// decodes nor reconstructs the Permission-owned evaluation.
fn attempt_premise(
    command: &WorkspaceActionCommand,
    target: &RealTargetRef,
    evaluation: RawId,
) -> AttemptCommitPremise {
    AttemptCommitPremise {
        attempt: ActionAttemptId::generate(),
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
        real_target: target.clone(),
        operation: command.operation,
        relied_evaluation: evaluation,
    }
}

/// Total mapping from the Action-owned operation kind to the permission-owned
/// capability vocabulary; the closed worlds must grow together.
fn action_kind(operation: OperationKind) -> ActionKind {
    match operation {
        OperationKind::List => ActionKind::List,
        OperationKind::Read => ActionKind::Read,
        OperationKind::Create => ActionKind::Create,
        OperationKind::Edit => ActionKind::Edit,
    }
}

fn input_check(command: &WorkspaceActionCommand) -> Option<ActionNotStarted> {
    match command.operation {
        OperationKind::List | OperationKind::Read => command
            .content
            .is_some()
            .then_some(ActionNotStarted::ContentNotAllowed),
        OperationKind::Create | OperationKind::Edit => command
            .content
            .is_none()
            .then_some(ActionNotStarted::MissingContent),
    }
}
