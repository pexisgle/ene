//! Host composition for conversation / first-party Task control.
//!
//! Every method here is composition only. The Task owner decides Task
//! creation, revision commits, cancel admission, failure, and result
//! adoption; the Action owner decides attempt starts and certainty. This
//! module loads those durable facts, maps owner-defined premises across the
//! boundary, and renders user-facing reports. It adds no Task lifecycle, no
//! report master, no adoption queue, and no SQL: reopening a report never
//! changes a canonical fact.
//!
//! The production triggers live here because they compose two owners:
//!
//! - [`HostHandle::propose_task`] receives the dialogue layer's accepted
//!   proposal, lets the Task owner commit the creation unit, then issues the
//!   first delegation through the existing AU3 orchestration. Starting the
//!   returned execution is [`HostHandle::run_task_agent`]'s job, not a side
//!   effect of the proposal.
//! - [`HostHandle::settle_action_certainty`] is the late-evidence settlement
//!   entry: it commits the Action owner's certainty CAS and then re-evaluates
//!   the same execution's sealed-but-unadopted result through the Task
//!   owner's adoption gate.
//! - [`HostHandle::reconcile_sealed_results`] is the explicit bounded startup
//!   reconciliation producer for results sealed after AU15a but not adopted
//!   before a stop. It never resumes an execution.
//! - [`HostHandle::task_report`] composes the progress / cancel / completion
//!   report from canonical Task and Action facts.

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionTechnicalError,
    CertaintyUpdateOutcome, EffectGrounds,
};
use ene_companion::CompanionId;
use ene_companion::dialogue::{
    ProposeSteeringCommand, ProposeTaskCommand, TaskReport, TaskReportAttempt, TaskReportCertainty,
};
use ene_task::{
    CreateDelegationCommand, DelegatedWorkspace, DelegationId, DelegationOutcome, DelegationScope,
    TaskContextOrigin, TaskId, TaskProposalOutcome, TaskPurpose, TaskRef, TaskRepository as _,
    TaskResultAcceptance, TaskResultId, TaskTechnicalError, WorkspaceNeedRef,
    orchestrate_delegation, reevaluate_result_adoption,
};
use thiserror::Error;

use crate::serve::HostHandle;

/// Technical failure of one conversation / first-party Task control call.
///
/// Domain refusals (stale, terminal, missing, withheld) stay on the `Ok` side
/// of each owner outcome; this error exists only where a composition crosses
/// two owners and either store can fail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskControlError {
    #[error(transparent)]
    Task(#[from] TaskTechnicalError),
    #[error(transparent)]
    Action(#[from] ActionTechnicalError),
}

/// Host-level result of one conversation-initiated Task proposal.
///
/// The Task owner's decision is preserved; the delegation is the separate
/// AU3 owner request the composition root issues after an accepted creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProposalHostOutcome {
    /// The Task was created and its first delegation committed.
    AcceptedAsTask {
        task: TaskRef,
        delegation: DelegationId,
    },
    /// The Task was created, but the first delegation was refused by the Task
    /// owner (for example a concurrent steering advanced the revision).
    /// The Task stays non-terminal without an execution; the caller decides
    /// whether to propose again.
    DelegationRefused {
        task: TaskRef,
        outcome: DelegationOutcome,
    },
    /// The Task owner refused the proposal itself ([`TaskProposalOutcome`]).
    Proposal(TaskProposalOutcome),
}

/// The Action owner's settled certainty together with the adoption
/// re-evaluation it may have triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSettlementOutcome {
    /// The Action owner's compare-and-set answer.
    pub certainty: CertaintyUpdateOutcome,
    /// The re-evaluation of the execution's sealed-but-unadopted result, when
    /// exactly one such result existed.
    pub adoption: Option<TaskResultAcceptance>,
}

/// One candidate's re-evaluation from the bounded recovery sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedResultReconciliation {
    pub result: TaskResultId,
    /// The Task owner's adoption answer. A technical correspondence error is
    /// reported per candidate so one corrupt result never hides the sweep's
    /// other answers (and never gets rounded to withheld or completed).
    pub adoption: Result<TaskResultAcceptance, TaskTechnicalError>,
}

/// Candidate results one startup reconciliation pass re-evaluates.
///
/// The sweep is bounded so startup work never grows with the whole result
/// history; each pass makes progress from the oldest sealed-but-unadopted
/// candidate, and the next pass (or an explicit call) continues.
pub const STARTUP_RECONCILIATION_LIMIT: u64 = 64;

impl HostHandle {
    /// Proposes one Task from the Owner conversation and creates its first
    /// delegation.
    ///
    /// The dialogue layer owns the command mapping
    /// ([`ene_companion::dialogue::propose_task`]); the Task owner mints the
    /// Task, context, and association identities and commits the AU2 unit.
    /// On acceptance this composition loads the committed unit, copies its
    /// confirmed workspace boundary into the delegation scope, and issues the
    /// existing AU3 delegation request. A delegation refusal keeps its typed
    /// owner outcome; nothing here re-tries, re-mints, or writes SQL.
    ///
    /// # Errors
    ///
    /// [`TaskTechnicalError`] when the store cannot answer; owner domain
    /// refusals stay inside [`TaskProposalHostOutcome`].
    pub async fn propose_task(
        &self,
        requester: CompanionId,
        purpose: TaskPurpose,
        origin: TaskContextOrigin,
        workspace_need: Option<WorkspaceNeedRef>,
    ) -> Result<TaskProposalHostOutcome, TaskTechnicalError> {
        let outcome = ene_companion::dialogue::propose_task(
            ProposeTaskCommand {
                requester,
                purpose,
                origin,
                workspace_need,
            },
            &self.store,
        )
        .await?;
        let TaskProposalOutcome::AcceptedAsTask(task) = outcome else {
            return Ok(TaskProposalHostOutcome::Proposal(outcome));
        };
        let Some(record) = self.store.load_task(task.task).await? else {
            // The AU2 commit just succeeded: a missing read is an
            // inconsistent unit, never a domain refusal.
            return Err(TaskTechnicalError::StorageUnavailable {
                reason: String::from("accepted task creation is not readable"),
            });
        };
        let scope_copy = match &record.workspace {
            Some(association) => DelegationScope {
                workspace: Some(DelegatedWorkspace {
                    assoc: association.assoc,
                    folder: association.folder.clone(),
                    save_target: association.save_target.clone(),
                }),
            },
            None => DelegationScope { workspace: None },
        };
        let delegated =
            orchestrate_delegation(&self.store, CreateDelegationCommand { task, scope_copy })
                .await?;
        Ok(match delegated {
            DelegationOutcome::Delegated(delegation) => TaskProposalHostOutcome::AcceptedAsTask {
                task,
                delegation: delegation.delegation,
            },
            outcome => TaskProposalHostOutcome::DelegationRefused { task, outcome },
        })
    }

    /// Proposes one steering change from the Owner conversation.
    ///
    /// The caller passes the relied-on revision and purpose identity it
    /// observed; the existing [`ene_companion::dialogue::propose_steering`]
    /// path returns the Task owner's outcome unchanged, so a stale revision
    /// is never retried or overwritten here.
    ///
    /// # Errors
    ///
    /// [`TaskTechnicalError`] when the store cannot answer.
    pub async fn propose_steering(
        &self,
        command: ProposeSteeringCommand,
    ) -> Result<TaskProposalOutcome, TaskTechnicalError> {
        ene_companion::dialogue::propose_steering(command, &self.store).await
    }

    /// Settles one Action attempt's late objective evidence and, when the
    /// settlement commits, re-evaluates the execution's sealed result.
    ///
    /// This composes two owner operations without merging them: the Action
    /// owner's certainty compare-and-set commits first (it alone may change
    /// certainty), and only an `Updated` answer triggers a bounded read of
    /// the attempt's delegation and its sealed result. A result that exists
    /// and is not adopted yet goes through
    /// [`ene_task::reevaluate_result_adoption`], which re-runs the existing
    /// `adopt_result` gate against current facts. A still-present blocker
    /// legitimately answers `WithheldByEffectFacts`; no busy retry loop
    /// exists. Nothing here re-executes a provider call or a filesystem
    /// Action.
    ///
    /// # Errors
    ///
    /// [`TaskControlError`] when either store cannot answer.
    pub async fn settle_action_certainty(
        &self,
        attempt: ActionAttemptId,
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<EffectSettlementOutcome, TaskControlError> {
        let certainty = self
            .store
            .compare_and_set_certainty(attempt, ActionCertainty::Unknown, new, grounds)
            .await?;
        let adoption = if certainty == CertaintyUpdateOutcome::Updated {
            self.reevaluate_sealed_result_for_attempt(attempt).await?
        } else {
            None
        };
        Ok(EffectSettlementOutcome {
            certainty,
            adoption,
        })
    }

    /// Bounded re-evaluation of one attempt's execution sealed result.
    ///
    /// A missing attempt row, a missing sealed result, and an already-adopted
    /// result all answer [`None`]: there is nothing to re-evaluate. The read
    /// is bounded to the attempt → delegation → sealed result path, never a
    /// scan.
    async fn reevaluate_sealed_result_for_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<TaskResultAcceptance>, TaskControlError> {
        let Some(record) = self.store.load_attempt(attempt).await? else {
            return Ok(None);
        };
        let delegation = DelegationId::from_raw(record.delegation);
        let Some(result) = self.store.load_delegation_result(delegation).await? else {
            return Ok(None);
        };
        if result.adopted_revision.is_some() {
            return Ok(None);
        }
        Ok(Some(
            reevaluate_result_adoption(&self.store, result.result).await?,
        ))
    }

    /// Re-evaluates a bounded prefix of sealed-but-unadopted results.
    ///
    /// This is the explicit recovery producer for results that were durably
    /// recorded (AU15a) but whose adoption commit (AU15b) did not run before
    /// a stop, and for withheld results whose blocking facts settled while
    /// nothing was listening. `adopted_revision IS NULL` is the whole durable
    /// candidate truth; no pending flag or retry queue exists. The listing is
    /// bounded and deterministically ordered, and each candidate goes through
    /// the same [`ene_task::reevaluate_result_adoption`] path, so a
    /// cancelled, moved-revision, or still-blocked result keeps its existing
    /// semantics. An execution is never resumed and no provider call or
    /// filesystem Action is replayed.
    ///
    /// # Errors
    ///
    /// [`TaskTechnicalError`] when the candidate listing itself cannot be
    /// read; per-candidate failures stay in
    /// [`SealedResultReconciliation::adoption`] so one corrupt result cannot
    /// hide the others.
    pub async fn reconcile_sealed_results(
        &self,
        limit: u64,
    ) -> Result<Vec<SealedResultReconciliation>, TaskTechnicalError> {
        let candidates = self.store.list_unadopted_results(limit).await?;
        let mut outcomes = Vec::with_capacity(candidates.len());
        for result in candidates {
            let adoption = reevaluate_result_adoption(&self.store, result).await;
            outcomes.push(SealedResultReconciliation { result, adoption });
        }
        Ok(outcomes)
    }

    /// Composes the user-facing Task report from canonical durable facts.
    ///
    /// `delegation` names the conversation's current delegated execution, when
    /// the caller holds one: it lets the report include a sealed-but-not-yet-
    /// adopted result body. Everything else is read from the Task owner
    /// (`task.progress`, the current workspace association, the adopted
    /// result) and the Action owner (each attempt's operation, target, and
    /// certainty). `None` means no Task with this identity exists.
    ///
    /// The composed report rewrites nothing: an `Unknown` effect stays
    /// unknown, and a cancelled Task is reported as cancelled even when
    /// already-started effects remain unresolved.
    ///
    /// # Errors
    ///
    /// [`TaskControlError`] when either store cannot answer, or when the
    /// durable correlation the report reads is internally inconsistent.
    pub async fn task_report(
        &self,
        task: TaskId,
        delegation: Option<DelegationId>,
    ) -> Result<Option<TaskReport>, TaskControlError> {
        let Some(record) = self.store.load_task(task).await? else {
            return Ok(None);
        };
        // The adopted result is the current completion master; without one,
        // the named execution's sealed result is shown as recorded-but-not-
        // adopted. Either way the body and verified correlation come from the
        // one `task_result` row.
        let (result_body, result_adopted, correlated_refs) = match record.task.adopted_result {
            Some(result) => {
                let Some(stored) = self.store.load_task_result(result).await? else {
                    return Err(inconsistent("adopted task result is not readable").into());
                };
                (
                    Some(stored.body.text().to_owned()),
                    true,
                    stored.attempt_refs,
                )
            }
            None => match delegation {
                Some(delegation) => match self.store.load_delegation_result(delegation).await? {
                    Some(stored) => (
                        Some(stored.body.text().to_owned()),
                        stored.adopted_revision.is_some(),
                        stored.attempt_refs,
                    ),
                    None => (None, false, Vec::new()),
                },
                None => (None, false, Vec::new()),
            },
        };
        let attempt_ids = self.store.load_task_action_attempts(task).await?;
        let mut correlated_attempts = Vec::new();
        let mut other_attempts = Vec::new();
        for attempt in attempt_ids {
            let Some(stored) = self
                .store
                .load_attempt(ActionAttemptId::from_raw(attempt))
                .await?
            else {
                return Err(inconsistent("reported action attempt is not readable").into());
            };
            let report = TaskReportAttempt {
                operation: stored.operation.as_str().to_owned(),
                target: stored.real_target.as_path().to_owned(),
                certainty: report_certainty(stored.certainty),
            };
            if correlated_refs.contains(&attempt) {
                correlated_attempts.push(report);
            } else {
                other_attempts.push(report);
            }
        }
        Ok(Some(TaskReport {
            progress: record.task.progress,
            workspace_folder: record
                .workspace
                .as_ref()
                .map(|association| association.folder.path.clone()),
            save_target: record
                .workspace
                .as_ref()
                .and_then(|association| association.save_target.as_ref())
                .map(|target| target.path.clone()),
            result_body,
            result_adopted,
            correlated_attempts,
            other_attempts,
        }))
    }
}

fn report_certainty(certainty: ActionCertainty) -> TaskReportCertainty {
    match certainty {
        ActionCertainty::ConfirmedSuccess => TaskReportCertainty::ConfirmedSuccess,
        ActionCertainty::ConfirmedFailure => TaskReportCertainty::ConfirmedFailure,
        ActionCertainty::Unknown => TaskReportCertainty::Unknown,
    }
}

fn inconsistent(reason: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: String::from(reason),
    }
}

#[cfg(test)]
mod tests;
