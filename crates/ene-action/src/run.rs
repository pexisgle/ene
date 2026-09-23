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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionNotStarted {
    StalePremise,
    TaskTerminal,
    ExecutionSealed,
    DataUseHeld,
    Rejected(TargetRejection),
    MissingContent,
    ContentNotAllowed,
    Denied(ActionDenyCode),
    NeedsRevalidation,
    EvaluationConsumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRunOutcome {
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        fact_recorded: bool,
    },
    NotStarted(ActionNotStarted),
}

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
