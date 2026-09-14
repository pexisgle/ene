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
//! - [`HostTaskControl`] is the composition root behind the companion's
//!   [`DialogueTaskControlPort`]: a companion `[task-control]` directive from
//!   an ordinary dialogue turn resolves its target through the transient
//!   conversation projection and maps onto the same owner boundaries below.
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

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionTechnicalError,
    CertaintyUpdateOutcome, EffectGrounds,
};
use ene_companion::CompanionId;
use ene_companion::dialogue::{
    DialogueTaskCommand, DialogueTaskControlPort, DialogueTaskControlReply, ProposeSteeringCommand,
    ProposeTaskCommand, TaskReport, TaskReportAttempt, TaskReportCertainty,
};
use ene_primitive::RawId;
use ene_task::{
    ConversationTaskRepository as _, CreateDelegationCommand, DelegatedWorkspace, DelegationId,
    DelegationOutcome, DelegationScope, OwnerMessageCurrentness, SteeringPremiseRef,
    TaskCancelOutcome, TaskContextOrigin, TaskContextOriginKind, TaskCreationOutcome, TaskId,
    TaskProgress, TaskProposalOutcome, TaskPurpose, TaskRef, TaskRepository as _,
    TaskResultAcceptance, TaskTechnicalError, WorkspaceFolderRef, WorkspaceNeedRef,
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

/// Bounded accounting of one reconciliation pass.
///
/// The pass never materializes every candidate: it processes one bounded page
/// at a time and returns counters plus at most the first technical error, so
/// memory stays independent of history size. A per-candidate technical error
/// is counted (never rounded to `Completed` / `Withheld`); the first one is
/// kept for diagnostics and the rest are summarized by count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconciliationSummary {
    /// Candidates evaluated in this pass.
    pub evaluated: u64,
    /// Results adopted as completion.
    pub adopted: u64,
    /// Results that legitimately stayed withheld by effect facts.
    pub withheld: u64,
    /// Results recorded to their original only (terminal / moved).
    pub recorded_to_original_only: u64,
    /// Results whose identity, delegation, or Task row is missing.
    pub missing: u64,
    /// Candidates whose re-evaluation failed technically.
    pub unavailable: u64,
    /// The first technical error, kept for diagnostics only.
    pub first_error: Option<TaskTechnicalError>,
}

/// Transient conversation projection of the Task one dialogue is working on.
///
/// In-memory only, keyed by Companion, and never durable authority: it lets a
/// task-less companion directive resolve to the Task the conversation most
/// recently created, while every operation still goes through the Task
/// owner's durable compare. The projection is dropped on restart, so a
/// post-restart directive answers "no active task" instead of guessing;
/// restart continuation is Stage 5.
#[derive(Default)]
pub(crate) struct ConversationTaskProjection {
    current: StdMutex<HashMap<CompanionId, ConversationTask>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConversationTask {
    task: TaskId,
    /// `None` when the creation committed but the delegation was refused; the
    /// report then shows no execution-local result.
    delegation: Option<DelegationId>,
}

impl ConversationTaskProjection {
    fn record(&self, companion: CompanionId, task: TaskId, delegation: Option<DelegationId>) {
        crate::lock_unpoison(&self.current)
            .insert(companion, ConversationTask { task, delegation });
    }

    fn current(&self, companion: CompanionId) -> Option<ConversationTask> {
        crate::lock_unpoison(&self.current).get(&companion).copied()
    }
}

/// Trusted first-party Task premises.
///
/// The Owner selects the Workspace through a first-party management inlet;
/// provider output can never create or widen these premises. In-memory only:
/// a restart re-selection is Stage 5.
#[derive(Default)]
pub(crate) struct TrustedTaskPremises {
    workspace: StdMutex<Option<WorkspaceFolderRef>>,
}

impl TrustedTaskPremises {
    pub(crate) fn set_workspace(&self, folder: WorkspaceFolderRef) {
        *crate::lock_unpoison(&self.workspace) = Some(folder);
    }

    pub(crate) fn workspace(&self) -> Option<WorkspaceFolderRef> {
        crate::lock_unpoison(&self.workspace).clone()
    }
}

/// Deterministic race gate for one conversation task-control command.
///
/// Test-only: it pauses a mutating directive right before it enters the Task
/// owner boundary, so a test can commit a newer Owner input first and pin
/// that the superseded command changes nothing.
#[cfg(test)]
pub(crate) struct TestTaskControlGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl Default for TestTaskControlGate {
    fn default() -> Self {
        Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

#[cfg(test)]
impl TestTaskControlGate {
    /// Pauses until the test releases the gate, marking entry first.
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    /// Waits until a paused command has entered the gate.
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    /// Releases one paused command.
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// Host composition root implementing the companion's Task control port.
///
/// The companion interprets its provider output into a
/// [`DialogueTaskCommand`]; this adapter maps that command onto the existing
/// Task owner boundaries ([`HostHandle::propose_task`],
/// [`HostHandle::propose_steering`], [`HostHandle::cancel_task`],
/// [`HostHandle::task_report`]) and renders the typed outcome. It never
/// writes Task state itself and never decides completion, cancellation
/// meaning, or certainty.
pub(crate) struct HostTaskControl<'a> {
    handle: &'a HostHandle,
    companion: CompanionId,
}

impl<'a> HostTaskControl<'a> {
    pub(crate) fn new(handle: &'a HostHandle, companion: CompanionId) -> Self {
        Self { handle, companion }
    }

    fn no_active_task() -> DialogueTaskControlReply {
        DialogueTaskControlReply::Answered(String::from(
            "There is no active task in this conversation.",
        ))
    }

    async fn propose(&self, purpose: String, origin: RawId) -> DialogueTaskControlReply {
        // The Workspace authority is the trusted first-party premise only;
        // provider output never carries one. Without a selection the Task is
        // not created: file work cannot run without an Owner-confirmed
        // boundary.
        let Some(workspace) = self.handle.trusted_task_premises.workspace() else {
            return DialogueTaskControlReply::Answered(String::from(
                "Select a workspace folder first; I cannot start file work without one.",
            ));
        };
        let currentness = OwnerMessageCurrentness {
            companion: self.companion.as_raw(),
            message: origin,
        };
        let outcome = ene_companion::dialogue::propose_task_current(
            ProposeTaskCommand {
                requester: self.companion,
                purpose: TaskPurpose { text: purpose },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: origin,
                },
                workspace_need: Some(WorkspaceNeedRef {
                    folder: workspace,
                    save_target: None,
                }),
            },
            &self.handle.store,
            currentness,
        )
        .await;
        match outcome {
            Err(_) => DialogueTaskControlReply::Unavailable,
            // The turn was superseded before the creation transaction: no
            // Task, no delegation, no reply.
            Ok(TaskCreationOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskCreationOutcome::Created(task)) => {
                match self.handle.delegate_task(task).await {
                    Err(_) => DialogueTaskControlReply::Unavailable,
                    Ok(DelegationOutcome::Delegated(delegation)) => {
                        self.handle.conversation_tasks.record(
                            self.companion,
                            task.task,
                            Some(delegation.delegation),
                        );
                        // Production launcher: the existing runner starts in
                        // the background; this turn never awaits it.
                        if let Some(launcher) = self.handle.task_launcher() {
                            launcher.launch(delegation.delegation);
                        }
                        DialogueTaskControlReply::Answered(String::from(
                            "Task accepted (status: in-progress). I will work on it.",
                        ))
                    }
                    Ok(_) => {
                        self.handle
                            .conversation_tasks
                            .record(self.companion, task.task, None);
                        DialogueTaskControlReply::Answered(String::from(
                            "Task accepted, but no execution could be delegated; it stays without an execution.",
                        ))
                    }
                }
            }
        }
    }

    async fn report(&self) -> DialogueTaskControlReply {
        let Some(current) = self.handle.conversation_tasks.current(self.companion) else {
            return Self::no_active_task();
        };
        match self
            .handle
            .task_report(current.task, current.delegation)
            .await
        {
            Err(_) => DialogueTaskControlReply::Unavailable,
            Ok(None) => Self::no_active_task(),
            Ok(Some(report)) => DialogueTaskControlReply::Answered(report.render()),
        }
    }

    async fn steer(
        &self,
        _instruction: String,
        purpose: Option<String>,
        origin: RawId,
    ) -> DialogueTaskControlReply {
        let Some(current) = self.handle.conversation_tasks.current(self.companion) else {
            return Self::no_active_task();
        };
        let record = match self.handle.store.load_task(current.task).await {
            Err(_) => return DialogueTaskControlReply::Unavailable,
            Ok(None) => return Self::no_active_task(),
            Ok(Some(record)) => record,
        };
        if record.task.progress.is_terminal() {
            return DialogueTaskControlReply::Answered(format!(
                "That task is already {}.",
                progress_label(record.task.progress)
            ));
        }
        // The adopted instruction body is canonical in the committed Owner
        // message record this turn appended; `origin` references it and the
        // directive's own summary text is never copied into Task state.
        let command = ProposeSteeringCommand {
            premise: SteeringPremiseRef {
                expected: record.task.reference,
                purpose: record.task.purpose,
            },
            new_purpose: purpose.map(|text| TaskPurpose { text }),
            instruction_source: origin,
        };
        let currentness = OwnerMessageCurrentness {
            companion: self.companion.as_raw(),
            message: origin,
        };
        match ene_companion::dialogue::propose_steering_current(
            command,
            &self.handle.store,
            currentness,
        )
        .await
        {
            Err(_) => DialogueTaskControlReply::Unavailable,
            // The turn was superseded before the revision commit: nothing was
            // written and no reply is adopted.
            Ok(TaskProposalOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskProposalOutcome::AcceptedAsSteering(reference)) => {
                DialogueTaskControlReply::Answered(format!(
                    "Instruction recorded (revision {}).",
                    reference.revision.as_u64()
                ))
            }
            Ok(outcome) => DialogueTaskControlReply::Answered(task_outcome_text(&outcome)),
        }
    }

    async fn cancel(&self, origin: RawId) -> DialogueTaskControlReply {
        let Some(current) = self.handle.conversation_tasks.current(self.companion) else {
            return Self::no_active_task();
        };
        let currentness = OwnerMessageCurrentness {
            companion: self.companion.as_raw(),
            message: origin,
        };
        match self
            .handle
            .store
            .cancel_task_from_conversation(current.task, currentness)
            .await
        {
            Err(_) => DialogueTaskControlReply::Unavailable,
            Ok(TaskCancelOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskCancelOutcome::CancelAccepted) => {
                // The durable admission is the authority; the local signal is
                // best-effort, exactly as in `HostHandle::cancel_task`.
                self.handle.task_executions.cancel(current.task);
                DialogueTaskControlReply::Answered(String::from(
                    "Cancel accepted. Running work stops best-effort; already-started effects keep their recorded certainty.",
                ))
            }
            Ok(TaskCancelOutcome::AlreadyCancelled) => {
                DialogueTaskControlReply::Answered(String::from("The task was already cancelled."))
            }
            Ok(TaskCancelOutcome::TaskTerminal { progress, .. }) => {
                DialogueTaskControlReply::Answered(format!(
                    "That task is already {}.",
                    progress_label(progress)
                ))
            }
            Ok(TaskCancelOutcome::MissingTask { .. }) => Self::no_active_task(),
        }
    }
}

impl DialogueTaskControlPort for HostTaskControl<'_> {
    async fn apply(&self, command: DialogueTaskCommand, origin: RawId) -> DialogueTaskControlReply {
        // Test-only race gate: a mutating command pauses before the owner
        // boundary so a newer Owner input can be committed first.
        #[cfg(test)]
        if !matches!(command, DialogueTaskCommand::Report)
            && let Some(gate) = self.handle.test_task_control_gate()
        {
            gate.pause().await;
        }
        match command {
            DialogueTaskCommand::ProposeTask { purpose } => self.propose(purpose, origin).await,
            DialogueTaskCommand::Report => self.report().await,
            DialogueTaskCommand::Steer {
                instruction,
                purpose,
            } => self.steer(instruction, purpose, origin).await,
            DialogueTaskCommand::Cancel => self.cancel(origin).await,
        }
    }
}

fn progress_label(progress: TaskProgress) -> String {
    progress.as_str().replace('_', "-")
}

fn task_outcome_text(outcome: &TaskProposalOutcome) -> String {
    match outcome {
        TaskProposalOutcome::AcceptedAsTask(_) | TaskProposalOutcome::AcceptedAsSteering(_) => {
            String::from("The instruction was applied.")
        }
        TaskProposalOutcome::StalePremise { current } => format!(
            "The task moved on; nothing was applied (current revision {}).",
            current.revision.as_u64()
        ),
        TaskProposalOutcome::TaskTerminal { progress, .. } => {
            format!("That task is already {}.", progress_label(*progress))
        }
        TaskProposalOutcome::MissingTask { .. } => {
            String::from("There is no active task in this conversation.")
        }
        TaskProposalOutcome::RevisionExhausted { .. } => {
            String::from("The task cannot take another change.")
        }
        // The guarded mapping consumes supersession before this renderer.
        TaskProposalOutcome::Superseded => {
            String::from("The request was superseded by a newer message.")
        }
    }
}

/// Candidates one reconciliation page reads.
///
/// Each storage read is bounded by this page size; a pass keeps advancing the
/// `(recorded_at, result_id)` keyset cursor until the candidate set is
/// exhausted, so a permanently unadopted front cannot starve later
/// candidates.
pub const RECONCILIATION_PAGE_SIZE: u64 = 64;

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
        Ok(match self.delegate_task(task).await? {
            DelegationOutcome::Delegated(delegation) => TaskProposalHostOutcome::AcceptedAsTask {
                task,
                delegation: delegation.delegation,
            },
            outcome => TaskProposalHostOutcome::DelegationRefused { task, outcome },
        })
    }

    /// Issues the existing AU3 delegation request for one committed Task.
    ///
    /// The committed unit supplies the boundary copy; the Task owner confirms
    /// the association and revision inside its own commit. This is the shared
    /// delegation step of the first-party and conversation proposal paths.
    async fn delegate_task(&self, task: TaskRef) -> Result<DelegationOutcome, TaskTechnicalError> {
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
        orchestrate_delegation(&self.store, CreateDelegationCommand { task, scope_copy }).await
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

    /// Re-evaluates the reconciliation candidate set, one bounded page at a
    /// time, without materializing every candidate.
    ///
    /// This is the explicit recovery producer for results that were durably
    /// recorded (AU15a) but whose adoption commit (AU15b) did not run before
    /// a stop, and for withheld results whose blocking facts settled while
    /// nothing was listening. The candidate predicate is canonical-facts
    /// only: `adopted_revision IS NULL` and re-adoption is still possible (the
    /// Task is non-terminal and its current revision is the relied revision).
    /// A result that can only answer `RecordedToOriginalOnly` is not a
    /// candidate, so permanent history is not re-evaluated on every startup;
    /// no pending flag, retry queue, or second adoption state exists. The walk
    /// uses keyset pages over `(recorded_at, result_id)`, so every candidate
    /// is visited once per pass and a candidate already passed is never
    /// re-read from the front. Each page read is bounded by
    /// [`RECONCILIATION_PAGE_SIZE`]. Each candidate goes through the same
    /// [`ene_task::reevaluate_result_adoption`] path, so a still-blocked
    /// result keeps its existing semantics. An execution is never resumed and
    /// no provider call or filesystem Action is replayed.
    ///
    /// # Errors
    ///
    /// [`TaskTechnicalError`] when a candidate page cannot be read;
    /// per-candidate failures are counted in the returned summary (never
    /// rounded to completion or withheld), so one corrupt result cannot hide
    /// the rest or wedge startup.
    pub async fn reconcile_sealed_results(
        &self,
    ) -> Result<ReconciliationSummary, TaskTechnicalError> {
        let mut summary = ReconciliationSummary::default();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .list_unadopted_results_after(cursor, RECONCILIATION_PAGE_SIZE)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = Some(*last);
            let full_page = page.len() as u64 == RECONCILIATION_PAGE_SIZE;
            for candidate in page {
                summary.evaluated += 1;
                match reevaluate_result_adoption(&self.store, candidate.result).await {
                    Ok(TaskResultAcceptance::AdoptedAsCompletion(_)) => summary.adopted += 1,
                    Ok(TaskResultAcceptance::WithheldByEffectFacts { .. }) => {
                        summary.withheld += 1;
                    }
                    Ok(TaskResultAcceptance::RecordedToOriginalOnly) => {
                        summary.recorded_to_original_only += 1;
                    }
                    Ok(
                        TaskResultAcceptance::MissingResult { .. }
                        | TaskResultAcceptance::MissingDelegation { .. }
                        | TaskResultAcceptance::MissingTask { .. },
                    ) => summary.missing += 1,
                    Err(error) => {
                        summary.unavailable += 1;
                        if summary.first_error.is_none() {
                            summary.first_error = Some(error);
                        }
                    }
                }
            }
            if !full_page {
                break;
            }
        }
        Ok(summary)
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
