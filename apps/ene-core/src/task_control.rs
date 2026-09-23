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
//! - `HostTaskControl` is the composition root behind the companion's
//!   [`DialogueTaskControlPort`]: a companion `[task-control]` directive from
//!   an ordinary dialogue turn resolves its target through the transient
//!   conversation projection and maps onto the same owner boundaries below.
//! - `HostTaskControl::propose` receives the dialogue layer's accepted
//!   proposal, lets the Task owner commit the creation unit, then issues and
//!   launches the first delegation through the existing AU3 orchestration.
//! - [`HostHandle::settle_action_certainty`] is the late-evidence settlement
//!   entry: it commits the Action owner's certainty CAS and then re-evaluates
//!   the same execution's sealed-but-unadopted result through the Task
//!   owner's adoption gate. The settlement is durable before the
//!   re-evaluation, so a re-evaluation failure is a retryable partial
//!   outcome, never a reported loss of the settlement.
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
use ene_api::v1::refs::ConnectionWireId;
use ene_companion::ActionCertaintyWire;
use ene_companion::CompanionId;
use ene_companion::RecordResumeActivityCommand;
use ene_companion::ResumeActivityOutcome;
use ene_companion::dialogue::{
    DialogueTaskCommand, DialogueTaskControlPort, DialogueTaskControlReply, ProposeSteeringCommand,
    ProposeTaskCommand, TaskReport, TaskReportAttempt,
};
use ene_primitive::RawId;
use ene_task::{
    ConversationTaskRepository as _, CreateDelegationCommand, DelegatedWorkspace, DelegationId,
    DelegationOutcome, DelegationScope, OwnerMessageCurrentness, ResumeInstructionSource,
    ResumeTaskCommand, SteeringPremiseRef, TaskCancelOutcome, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationOutcome, TaskId, TaskProgress, TaskProposalOutcome,
    TaskPurpose, TaskRef, TaskRepository as _, TaskResultAcceptance, TaskResumeOutcome,
    TaskResumeReadiness, TaskTechnicalError, WorkspaceFolderRef, WorkspaceNeedRef,
    orchestrate_delegation, orchestrate_resume, orchestrate_resume_current,
    reevaluate_result_adoption, resume_commit_premise, route_available_result,
};
use thiserror::Error;

use crate::serve::{HostHandle, LiveInput};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskControlError {
    #[error(transparent)]
    Task(#[from] TaskTechnicalError),
    #[error(transparent)]
    Action(#[from] ActionTechnicalError),
}

/// The Action owner's settled certainty together with the adoption
/// re-evaluation it may have triggered.
///
/// The two steps are ordered but not atomic: the certainty compare-and-set
/// commits first and stays durable even when the follow-up re-evaluation
/// fails. A re-evaluation failure is therefore a retryable partial outcome on
/// `Ok` ([`Self::adoption_error`]), never an `Err`: `Err` means the settlement
/// itself did not commit and nothing changed. Re-calling
/// [`HostHandle::settle_action_certainty`] with the same arguments is the
/// retry that converges on the re-evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSettlementOutcome {
    /// The Action owner's compare-and-set answer. `Updated` means this call
    /// committed the settlement; `StaleCurrent` means an earlier settlement is
    /// already durable and this call changed nothing.
    pub certainty: CertaintyUpdateOutcome,
    /// The re-evaluation of the execution's sealed-but-unadopted result, when
    /// the attempt exists, exactly one such result existed, and the
    /// re-evaluation answered.
    pub adoption: Option<TaskResultAcceptance>,
    /// The re-evaluation's technical failure, when it ran and failed.
    ///
    /// `Some` means the settlement (this call's or an earlier one's) is
    /// durable and the re-evaluation is still pending: retry the same call to
    /// converge. `None` alongside `Updated` / `StaleCurrent` means the
    /// re-evaluation answered; `adoption == None` then means no
    /// sealed-but-unadopted result was left to evaluate.
    pub adoption_error: Option<TaskControlError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconciliationSummary {
    pub evaluated: u64,
    pub adopted: u64,
    pub withheld: u64,
    pub recorded_to_original_only: u64,
    pub missing: u64,
    pub unavailable: u64,
    pub first_error: Option<TaskTechnicalError>,
}

#[derive(Default)]
pub(crate) struct ConversationTaskProjection {
    current: StdMutex<HashMap<CompanionId, ConversationTask>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversationTask {
    pub(crate) task: TaskId,
    pub(crate) delegation: Option<DelegationId>,
    source: ConversationTaskSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversationTaskSource {
    Dialogue,
    FirstPartySelection { connection: ConnectionWireId },
}

impl ConversationTaskProjection {
    fn record(&self, companion: CompanionId, task: TaskId, delegation: Option<DelegationId>) {
        crate::lock_unpoison(&self.current).insert(
            companion,
            ConversationTask {
                task,
                delegation,
                source: ConversationTaskSource::Dialogue,
            },
        );
    }

    pub(crate) fn select(
        &self,
        companion: CompanionId,
        task: TaskId,
        connection: ConnectionWireId,
    ) {
        crate::lock_unpoison(&self.current).insert(
            companion,
            ConversationTask {
                task,
                delegation: None,
                source: ConversationTaskSource::FirstPartySelection { connection },
            },
        );
    }

    pub(crate) fn current(
        &self,
        companion: CompanionId,
        connection: &ConnectionWireId,
    ) -> Option<ConversationTask> {
        let entry = crate::lock_unpoison(&self.current)
            .get(&companion)
            .copied()?;
        match entry.source {
            ConversationTaskSource::Dialogue => Some(entry),
            ConversationTaskSource::FirstPartySelection { connection: owner }
                if owner == *connection =>
            {
                Some(entry)
            }
            ConversationTaskSource::FirstPartySelection { .. } => None,
        }
    }

    pub(crate) fn drop_first_party_selection_for(&self, connection: &ConnectionWireId) {
        crate::lock_unpoison(&self.current).retain(|_, entry| {
            !matches!(
                entry.source,
                ConversationTaskSource::FirstPartySelection { connection: owner } if owner == *connection
            )
        });
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn first_party_selection_count(&self) -> usize {
        crate::lock_unpoison(&self.current)
            .values()
            .filter(|entry| {
                matches!(
                    entry.source,
                    ConversationTaskSource::FirstPartySelection { .. }
                )
            })
            .count()
    }
}

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
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    #[expect(dead_code, reason = "test synchronization gate")]
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    #[expect(dead_code, reason = "test synchronization gate")]
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

#[cfg(test)]
pub(crate) struct TestResumeGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl Default for TestResumeGate {
    fn default() -> Self {
        Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

#[cfg(test)]
impl TestResumeGate {
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    #[expect(dead_code, reason = "test gate hook")]
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    #[expect(dead_code, reason = "test gate hook")]
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// Host composition root implementing the companion's Task control port.
///
/// The companion interprets its provider output into a
/// [`DialogueTaskCommand`]; this adapter maps that command onto the existing
/// Task owner boundaries ([`HostHandle::cancel_task`],
/// [`HostHandle::task_report`]) and renders the typed outcome. It never
/// writes Task state itself and never decides completion, cancellation
/// meaning, or certainty.
pub(crate) struct HostTaskControl<'a> {
    handle: &'a HostHandle,
    companion: CompanionId,
    connection: ene_api::v1::refs::ConnectionWireId,
}

impl<'a> HostTaskControl<'a> {
    pub(crate) fn new(
        handle: &'a HostHandle,
        companion: CompanionId,
        connection: ene_api::v1::refs::ConnectionWireId,
    ) -> Self {
        Self {
            handle,
            companion,
            connection,
        }
    }

    fn current_task(&self) -> Option<ConversationTask> {
        self.handle
            .conversation_tasks
            .current(self.companion, &self.connection)
    }

    fn no_active_task() -> DialogueTaskControlReply {
        DialogueTaskControlReply::Answered(String::from(
            "There is no active task in this conversation.",
        ))
    }

    async fn propose(&self, purpose: String, origin: RawId) -> DialogueTaskControlReply {
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
            Ok(TaskCreationOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskCreationOutcome::Created(task)) => match self.handle.delegate_task(task).await {
                Err(_) => DialogueTaskControlReply::Unavailable,
                Ok(DelegationOutcome::Delegated(delegation)) => {
                    self.handle.conversation_tasks.record(
                        self.companion,
                        task.task,
                        Some(delegation.delegation),
                    );
                    self.handle.launch_or_release(delegation.delegation);
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
            },
        }
    }

    async fn report(&self) -> DialogueTaskControlReply {
        let Some(current) = self.current_task() else {
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
        let Some(current) = self.current_task() else {
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
            Ok(TaskProposalOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskProposalOutcome::AcceptedAsSteering(reference)) => {
                match self.handle.delegate_task(reference).await {
                    Err(_) => DialogueTaskControlReply::Unavailable,
                    Ok(DelegationOutcome::Delegated(delegation)) => {
                        self.handle.conversation_tasks.record(
                            self.companion,
                            reference.task,
                            Some(delegation.delegation),
                        );
                        self.handle.launch_or_release(delegation.delegation);
                        DialogueTaskControlReply::Answered(format!(
                            "Instruction recorded (revision {}). I will continue with it.",
                            reference.revision.as_u64()
                        ))
                    }
                    Ok(outcome) => {
                        DialogueTaskControlReply::Answered(delegation_outcome_text(&outcome))
                    }
                }
            }
            Ok(outcome) => DialogueTaskControlReply::Answered(task_outcome_text(&outcome)),
        }
    }

    async fn cancel(&self, origin: RawId) -> DialogueTaskControlReply {
        let Some(current) = self.current_task() else {
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

    async fn resume(&self, origin: RawId) -> DialogueTaskControlReply {
        let Some(current) = self.current_task() else {
            return DialogueTaskControlReply::Answered(String::from(
                "There is no active task in this conversation. Tell me which task to resume.",
            ));
        };
        let record = match self.handle.store.load_task(current.task).await {
            Err(_) => return DialogueTaskControlReply::Unavailable,
            Ok(None) => return Self::no_active_task(),
            Ok(Some(record)) => record,
        };
        let currentness = OwnerMessageCurrentness {
            companion: self.companion.as_raw(),
            message: origin,
        };
        let command = ResumeTaskCommand {
            premise: SteeringPremiseRef {
                expected: record.task.reference,
                purpose: record.task.purpose,
            },
            instruction: ResumeInstructionSource::OwnerHistory {
                message: origin,
                currentness,
            },
        };
        match self.handle.resume_task_current(command, currentness).await {
            Err(_) => DialogueTaskControlReply::Unavailable,
            Ok(TaskResumeOutcome::Superseded) => DialogueTaskControlReply::Unavailable,
            Ok(TaskResumeOutcome::Resumed { task, delegation }) => {
                self.handle.conversation_tasks.record(
                    self.companion,
                    task.task,
                    Some(delegation.delegation),
                );
                DialogueTaskControlReply::Answered(format!(
                    "Task resumed (revision {}). I will continue with it.",
                    task.revision.as_u64()
                ))
            }
            Ok(outcome) => DialogueTaskControlReply::Answered(resume_outcome_text(&outcome)),
        }
    }
}

impl DialogueTaskControlPort for HostTaskControl<'_> {
    async fn apply(&self, command: DialogueTaskCommand, origin: RawId) -> DialogueTaskControlReply {
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
            DialogueTaskCommand::Resume => self.resume(origin).await,
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
        TaskProposalOutcome::Superseded => {
            String::from("The request was superseded by a newer message.")
        }
        TaskProposalOutcome::HeldForErasure => {
            String::from("The instruction could not be applied while its data is being deleted.")
        }
    }
}

fn resume_outcome_text(outcome: &TaskResumeOutcome) -> String {
    match outcome {
        TaskResumeOutcome::Resumed { .. } => String::from("The task was resumed."),
        TaskResumeOutcome::StalePremise { current } => format!(
            "The task moved on; nothing was resumed (current revision {}).",
            current.revision.as_u64()
        ),
        TaskResumeOutcome::TaskTerminal { progress, .. } => {
            format!("That task is already {}.", progress_label(*progress))
        }
        TaskResumeOutcome::AlreadyRunning { .. } => {
            String::from("That task is already running; nothing was resumed.")
        }
        TaskResumeOutcome::HeldByUnknownEffects { .. } => String::from(
            "The task has effects with unknown outcomes; resume stays on hold until they settle.",
        ),
        TaskResumeOutcome::ResultAvailable { .. } => {
            String::from("The task has a recorded result to review first; nothing was resumed.")
        }
        TaskResumeOutcome::NeedsRevalidation(hold) => String::from(match hold {
            ene_task::TaskResumeHold::CompanionUnavailable => {
                "The task owner is unavailable; nothing was resumed."
            }
            ene_task::TaskResumeHold::WorkspaceUnavailable => {
                "The task has no workspace to continue in; nothing was resumed."
            }
            ene_task::TaskResumeHold::InstructionUnavailable => {
                "The resume instruction is unavailable; nothing was resumed."
            }
            ene_task::TaskResumeHold::PermissionUnavailable => {
                "A permission check is needed first; nothing was resumed."
            }
            ene_task::TaskResumeHold::DataUseHeld => {
                "Some task content is under a deletion hold; nothing was resumed."
            }
            ene_task::TaskResumeHold::ExecutionUnavailable => {
                "No runner is available to continue the task; nothing was resumed."
            }
        }),
        TaskResumeOutcome::MissingTask { .. } => {
            String::from("There is no active task in this conversation.")
        }
        TaskResumeOutcome::RevisionExhausted { .. } => {
            String::from("The task cannot take another change.")
        }
        TaskResumeOutcome::Superseded => {
            String::from("The request was superseded by a newer message.")
        }
    }
}

fn delegation_outcome_text(outcome: &DelegationOutcome) -> String {
    match outcome {
        DelegationOutcome::Delegated(_) => String::from("The execution was delegated."),
        DelegationOutcome::StaleTaskRevision { current } => format!(
            "The task moved on again; the latest instruction was not executed (current revision {}).",
            current.revision.as_u64()
        ),
        DelegationOutcome::TaskTerminal { progress, .. } => {
            format!("That task is already {}.", progress_label(*progress))
        }
        DelegationOutcome::MissingTask { .. } => {
            String::from("There is no active task in this conversation.")
        }
    }
}

pub const RECONCILIATION_PAGE_SIZE: u64 = 64;

impl HostHandle {
    /// Issues the existing AU3 delegation request for one committed Task.
    ///
    /// The committed unit supplies the boundary copy; the Task owner confirms
    /// the association and revision inside its own commit. This is the shared
    /// delegation step of the first-party and conversation proposal paths.
    ///
    /// The commit and the launch reservation share the registry commit scope
    /// (CCT §7.4): the scope serializes this producer against every other
    /// AU3/AU17 producer in the process, so the committed delegation is
    /// reserved before any concurrent resume can observe the Task as free.
    /// A lost reservation race is a technical error: the delegation is
    /// durable but unlaunchable, and a retry is a new explicit delegation,
    /// never an automatic relaunch.
    async fn delegate_task(&self, task: TaskRef) -> Result<DelegationOutcome, TaskTechnicalError> {
        let _scope = self.task_executions.commit_scope().await;
        let Some(record) = self.store.load_task(task.task).await? else {
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
        let outcome =
            orchestrate_delegation(&self.store, CreateDelegationCommand { task, scope_copy })
                .await?;
        if let DelegationOutcome::Delegated(delegation) = &outcome {
            // refusal here is a corrupted registry, never a lost race with
            // another Task.
            if !self
                .task_executions
                .reserve(delegation.delegation, task.task)
            {
                return Err(TaskTechnicalError::StorageUnavailable {
                    reason: String::from("delegation launch reservation lost its race"),
                });
            }
        }
        Ok(outcome)
    }

    fn launch_or_release(&self, delegation: DelegationId) {
        if let Some(launcher) = self.task_launcher() {
            launcher.launch(delegation);
        } else {
            self.task_executions.release(delegation);
        }
    }

    /// Host-known readiness for one resume commit.
    ///
    /// Read under the launch commit scope: the final permission / cap
    /// judgement stays with the AU14/AU5 gates (`permission_available` is
    /// always `true` here), while the Task's reservation/registration state
    /// and the launcher's presence are read now so the commit orders them
    /// in its refusal priority. No presence check and no provider call are
    /// involved: an explicit Owner instruction is sufficient premise.
    fn resume_readiness(&self, task: TaskId) -> TaskResumeReadiness {
        TaskResumeReadiness {
            permission_available: true,
            execution_free: !self.task_executions.task_has_reservation_or_running(task),
            launch_possible: self.task_launcher().is_some(),
        }
    }

    fn reserve_and_launch_resumed(
        &self,
        outcome: &TaskResumeOutcome,
    ) -> Result<(), TaskTechnicalError> {
        let TaskResumeOutcome::Resumed { delegation, .. } = outcome else {
            return Ok(());
        };
        if !self
            .task_executions
            .reserve(delegation.delegation, delegation.task.task)
        {
            return Err(TaskTechnicalError::StorageUnavailable {
                reason: String::from("resume launch reservation lost its race"),
            });
        }
        self.launch_or_release(delegation.delegation);
        Ok(())
    }

    pub(crate) async fn resume_task_guarded_by_connection(
        &self,
        live: &LiveInput,
        premise: SteeringPremiseRef,
        activity: RecordResumeActivityCommand,
    ) -> Result<Option<TaskResumeOutcome>, TaskTechnicalError> {
        let _scope = self.task_executions.commit_scope().await;
        let launch_possible = self.task_launcher().is_some();
        #[cfg(test)]
        {
            let gate = crate::lock_unpoison(&self.resume_gate).clone();
            if let Some(gate) = gate {
                gate.pause().await;
            }
        }
        let task = premise.expected.task;
        let store = self.store.clone();
        let registry = std::sync::Arc::clone(&self.task_executions);
        let table = std::sync::Arc::clone(&live.authority);
        let connection = live.connection_id;
        let joined = tokio::task::spawn_blocking(move || {
            table.with_current_connection(&connection, || {
                let activity = store
                    .record_resume_activity_sync(activity)
                    .map_err(|error| TaskTechnicalError::StorageUnavailable {
                        reason: error.to_string(),
                    })?;
                let ResumeActivityOutcome::Recorded(activity) = activity else {
                    return Ok(TaskResumeOutcome::NeedsRevalidation(
                        ene_task::TaskResumeHold::DataUseHeld,
                    ));
                };
                let command = ResumeTaskCommand {
                    premise,
                    instruction: ResumeInstructionSource::OwnerManagement {
                        activity: activity.as_raw(),
                    },
                };
                let readiness = TaskResumeReadiness {
                    permission_available: true,
                    execution_free: !registry.task_has_reservation_or_running(task),
                    launch_possible,
                };
                let outcome =
                    store.commit_task_resume_sync(resume_commit_premise(command, readiness))?;
                if let TaskResumeOutcome::Resumed { delegation, .. } = &outcome
                    && !registry.reserve(delegation.delegation, delegation.task.task)
                {
                    return Err(TaskTechnicalError::StorageUnavailable {
                        reason: String::from("resume launch reservation lost its race"),
                    });
                }
                Ok(outcome)
            })
        })
        .await;
        let outcome = match joined {
            Ok(Some(Ok(outcome))) => outcome,
            Ok(Some(Err(error))) => return Err(error),
            Ok(None) => return Ok(None),
            Err(join) => std::panic::resume_unwind(join.into_panic()),
        };
        if let TaskResumeOutcome::ResultAvailable { task } = &outcome {
            route_available_result(&self.store, *task).await?;
        }
        if let TaskResumeOutcome::Resumed { delegation, .. } = &outcome {
            self.launch_or_release(delegation.delegation);
        }
        Ok(Some(outcome))
    }

    pub async fn resume_task(
        &self,
        command: ResumeTaskCommand,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        let _scope = self.task_executions.commit_scope().await;
        let readiness = self.resume_readiness(command.premise.expected.task);
        let outcome = orchestrate_resume(&self.store, command, readiness).await?;
        self.reserve_and_launch_resumed(&outcome)?;
        Ok(outcome)
    }

    async fn resume_task_current(
        &self,
        command: ResumeTaskCommand,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        let _scope = self.task_executions.commit_scope().await;
        let readiness = self.resume_readiness(command.premise.expected.task);
        let outcome =
            orchestrate_resume_current(&self.store, command, readiness, currentness).await?;
        self.reserve_and_launch_resumed(&outcome)?;
        Ok(outcome)
    }

    /// Settles one Action attempt's late objective evidence and, when the
    /// settlement commits, re-evaluates the execution's sealed result.
    ///
    /// This composes two owner operations without merging them: the Action
    /// owner's certainty compare-and-set commits first (it alone may change
    /// certainty), and only then does a bounded read of the attempt's
    /// delegation and its sealed result run through the Task owner's
    /// [`ene_task::reevaluate_result_adoption`]. A still-present blocker
    /// legitimately answers `WithheldByEffectFacts`; no busy retry loop
    /// exists. Nothing here re-executes a provider call or a filesystem
    /// Action.
    ///
    /// The two steps are ordered, not atomic: a failure in the re-evaluation
    /// can never retract a committed settlement, so it is reported as the
    /// partial outcome [`EffectSettlementOutcome::adoption_error`] and the
    /// same call is its retry. Re-invoking after a committed settlement is
    /// idempotent: the compare-and-set answers `StaleCurrent` (or
    /// `MissingAttempt` when the row is gone), certainty is never rewritten,
    /// and an already-adopted result is never adopted twice. `StaleCurrent`
    /// is not a dead end — the re-evaluation still runs against current
    /// durable facts, so an attempt settled before its blocker cleared
    /// converges without restart. Only an `Err` means nothing was committed.
    /// After a restart, the bounded
    /// [`HostHandle::reconcile_sealed_results`] pass covers the same
    /// sealed-but-unadopted result.
    ///
    /// # Errors
    ///
    /// [`TaskControlError`] only when the certainty compare-and-set itself
    /// cannot answer; a re-evaluation failure is reported on the returned
    /// outcome.
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
        let (adoption, adoption_error) = if certainty == CertaintyUpdateOutcome::MissingAttempt {
            // No attempt row means no delegation to correlate a sealed result
            // with: nothing was committed and nothing is left to re-evaluate.
            (None, None)
        } else {
            match self.reevaluate_sealed_result_for_attempt(attempt).await {
                Ok(adoption) => (adoption, None),
                Err(error) => (None, Some(error)),
            }
        };
        Ok(EffectSettlementOutcome {
            certainty,
            adoption,
            adoption_error,
        })
    }

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

    pub async fn reconcile_sealed_results(
        &self,
    ) -> Result<ReconciliationSummary, TaskTechnicalError> {
        self.reconcile_sealed_results_with_page_size(RECONCILIATION_PAGE_SIZE)
            .await
    }

    async fn reconcile_sealed_results_with_page_size(
        &self,
        page_size: u64,
    ) -> Result<ReconciliationSummary, TaskTechnicalError> {
        debug_assert_ne!(page_size, 0, "reconciliation page size must be non-zero");
        let mut summary = ReconciliationSummary::default();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .list_unadopted_results_after(cursor, page_size)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = Some(*last);
            let full_page = page.len() as u64 == page_size;
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

    pub async fn task_report(
        &self,
        task: TaskId,
        delegation: Option<DelegationId>,
    ) -> Result<Option<TaskReport>, TaskControlError> {
        let Some(record) = self.store.load_task(task).await? else {
            return Ok(None);
        };
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

fn report_certainty(certainty: ActionCertainty) -> ActionCertaintyWire {
    match certainty {
        ActionCertainty::ConfirmedSuccess => ActionCertaintyWire::ConfirmedSuccess,
        ActionCertainty::ConfirmedFailure => ActionCertaintyWire::ConfirmedFailure,
        ActionCertainty::Unknown => ActionCertaintyWire::Unknown,
    }
}

fn inconsistent(reason: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: String::from(reason),
    }
}
