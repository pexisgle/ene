use ene_action::{ActionCertainty, ActionNotStarted, ActionOutput, ObservedEffect, OperationKind};
use ene_credential::{CredentialSetRevision, SecretScrubber};
use ene_inference::{DispatchAbort, ProviderTransport};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;
use ene_task::{
    TaskAgentActionExchange, TaskAgentInference, TaskAgentNotSent, TaskAgentObservation,
    TaskAgentObservationId, TaskAgentObservationPremise, TaskAgentTurnOutcome,
    TaskAgentTurnPremise, TaskInstructionSource, TaskProgress, TaskRef, TaskRepository as _,
    TaskResultArrivalOutcome, TaskResultRecord, TaskResultScrubPremise, orchestrate_result_arrival,
    orchestrate_task_agent_turn,
};
use std::sync::Arc;

use crate::action::{
    TaskEffectRuntime, WorkspaceActionHostError, WorkspaceActionHostOutcome, run_workspace_action,
};
use crate::serve::{CoreError, HostHandle};

pub const DEFAULT_MAX_TURNS: u32 = 16;
pub(crate) const TASK_AGENT_QUIESCE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskClaimKind {
    Inference,
    Action,
    #[cfg(any(test, feature = "test-support"))]
    ActionEffect,
}

#[cfg(any(test, feature = "test-support"))]
struct TaskClaimPause {
    kind: TaskClaimKind,
    entered: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
}

#[cfg(any(test, feature = "test-support"))]
impl TaskClaimPause {
    fn new(kind: TaskClaimKind) -> Self {
        Self {
            kind,
            entered: std::sync::atomic::AtomicBool::new(false),
            release: tokio::sync::Notify::new(),
        }
    }
}

#[derive(Default)]
pub struct TaskExecutionRegistry {
    running: std::sync::Mutex<std::collections::HashMap<ene_task::DelegationId, RunningExecution>>,
    reservations:
        std::sync::Mutex<std::collections::HashMap<ene_task::DelegationId, ene_task::TaskId>>,
    commit_scope: Arc<tokio::sync::Mutex<()>>,
    host_shutdown: DispatchAbort,
    #[cfg(any(test, feature = "test-support"))]
    claim_pause: std::sync::Mutex<Option<Arc<TaskClaimPause>>>,
}

pub enum TakeReservation<'a> {
    Admitted(TaskExecutionRegistration<'a>),
    AlreadyRunning,
    Unreserved,
}

struct RunningExecution {
    task: ene_task::TaskId,
    cancellation: DispatchAbort,
}

impl TaskExecutionRegistry {
    pub fn cancel(&self, task: ene_task::TaskId) -> bool {
        let running = crate::lock_unpoison(&self.running);
        let mut signalled = false;
        for execution in running.values() {
            if execution.task == task {
                execution.cancellation.abort();
                signalled = true;
            }
        }
        signalled
    }

    pub(crate) async fn abort_for_host_shutdown(&self) {
        let _commit = self.commit_scope().await;
        self.host_shutdown.abort();
    }

    pub(crate) fn abort_for_host_shutdown_without_commit_scope(&self) {
        self.host_shutdown.abort();
    }

    pub async fn commit_scope(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.commit_scope).lock_owned().await
    }

    pub(crate) async fn task_claim_scope(
        &self,
        _kind: TaskClaimKind,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        #[cfg(any(test, feature = "test-support"))]
        self.pause_before_task_claim(_kind).await;
        self.commit_scope().await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn arm_task_claim_pause_for_tests(&self, kind: TaskClaimKind) {
        *crate::lock_unpoison(&self.claim_pause) = Some(Arc::new(TaskClaimPause::new(kind)));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_task_claim_pause_for_tests(&self) {
        loop {
            let pause = crate::lock_unpoison(&self.claim_pause).clone();
            if pause.is_some_and(|pause| pause.entered.load(std::sync::atomic::Ordering::SeqCst)) {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn release_task_claim_pause_for_tests(&self) {
        if let Some(pause) = crate::lock_unpoison(&self.claim_pause).as_ref() {
            pause.release.notify_waiters();
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) async fn pause_after_task_effect_for_tests(&self) {
        self.pause_before_task_claim(TaskClaimKind::ActionEffect)
            .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn pause_before_task_claim(&self, kind: TaskClaimKind) {
        let pause = {
            let configured = crate::lock_unpoison(&self.claim_pause).clone();
            configured.filter(|pause| pause.kind == kind)
        };
        let Some(pause) = pause else {
            return;
        };
        let released = pause.release.notified();
        pause
            .entered
            .store(true, std::sync::atomic::Ordering::SeqCst);
        released.await;
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn wait_host_shutdown_for_tests(&self) {
        while !self.host_shutdown.is_aborted() {
            tokio::task::yield_now().await;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn running_task_executions_for_tests(&self) -> usize {
        crate::lock_unpoison(&self.running).len()
    }

    pub fn reserve(&self, delegation: ene_task::DelegationId, task: ene_task::TaskId) -> bool {
        let running = crate::lock_unpoison(&self.running);
        let mut reservations = crate::lock_unpoison(&self.reservations);
        if running.contains_key(&delegation) || reservations.contains_key(&delegation) {
            return false;
        }
        reservations.insert(delegation, task);
        true
    }

    pub fn release(&self, delegation: ene_task::DelegationId) {
        crate::lock_unpoison(&self.reservations).remove(&delegation);
    }

    pub fn task_has_reservation_or_running(&self, task: ene_task::TaskId) -> bool {
        crate::lock_unpoison(&self.running)
            .values()
            .any(|execution| execution.task == task)
            || crate::lock_unpoison(&self.reservations)
                .values()
                .any(|reserved| *reserved == task)
    }

    pub fn take_reservation(
        &self,
        delegation: ene_task::DelegationId,
        task: ene_task::TaskId,
    ) -> TakeReservation<'_> {
        let mut running = crate::lock_unpoison(&self.running);
        if running.contains_key(&delegation) {
            return TakeReservation::AlreadyRunning;
        }
        let mut reservations = crate::lock_unpoison(&self.reservations);
        match reservations.remove(&delegation) {
            Some(reserved_task) if reserved_task == task => {}
            Some(reserved_task) => {
                reservations.insert(delegation, reserved_task);
                return TakeReservation::Unreserved;
            }
            None => return TakeReservation::Unreserved,
        }
        let cancellation = DispatchAbort::default();
        let dispatch_abort = cancellation.linked_to(&self.host_shutdown);
        running.insert(
            delegation,
            RunningExecution {
                task,
                cancellation: cancellation.clone(),
            },
        );
        TakeReservation::Admitted(TaskExecutionRegistration {
            registry: self,
            delegation,
            cancellation,
            host_shutdown: self.host_shutdown.clone(),
            dispatch_abort,
        })
    }

    fn remove(&self, delegation: ene_task::DelegationId) {
        crate::lock_unpoison(&self.running).remove(&delegation);
    }
}

pub struct TaskExecutionRegistration<'a> {
    registry: &'a TaskExecutionRegistry,
    delegation: ene_task::DelegationId,
    cancellation: DispatchAbort,
    host_shutdown: DispatchAbort,
    dispatch_abort: DispatchAbort,
}

impl TaskExecutionRegistration<'_> {
    pub(crate) fn dispatch_abort(&self) -> &DispatchAbort {
        &self.dispatch_abort
    }

    pub(crate) fn task_executions(&self) -> &TaskExecutionRegistry {
        self.registry
    }

    fn stop_outcome(&self) -> Option<TaskAgentRunOutcome> {
        if self.cancellation.is_aborted() {
            Some(TaskAgentRunOutcome::Cancelled)
        } else if self.host_shutdown.is_aborted() {
            Some(TaskAgentRunOutcome::HostShutdown)
        } else {
            None
        }
    }
}

impl Drop for TaskExecutionRegistration<'_> {
    fn drop(&mut self) {
        self.registry.remove(self.delegation);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentDirective {
    Act(TaskAgentActionRequest),
    Finish { body: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentActionRequest {
    pub operation: OperationKind,
    pub path: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAgentProtocolViolation {
    NotAJsonObject,
    MissingDirective,
    ConflictingDirective,
    UnknownTool,
    InvalidFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentRunRefusal {
    MissingDelegation {
        delegation: ene_task::DelegationId,
    },
    MissingTask {
        task: ene_task::TaskId,
    },
    StaleTaskRevision {
        current: TaskRef,
    },
    TaskTerminal {
        task: ene_task::TaskId,
        progress: TaskProgress,
    },
    ExecutionSealed {
        delegation: ene_task::DelegationId,
    },
    ExecutionAlreadyStarted {
        delegation: ene_task::DelegationId,
    },
    ExecutionAlreadyRunning {
        delegation: ene_task::DelegationId,
    },
    ExecutionUnavailable {
        delegation: ene_task::DelegationId,
    },
    MissingWorkspace {
        task: ene_task::TaskId,
    },
    WorkspaceUnavailable {
        task: ene_task::TaskId,
    },
    InstructionSourceMissing {
        entry: ene_task::TaskContextEntryId,
        source: RawId,
    },
    StaleCredentialSet {
        current: CredentialSetRevision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentRunOutcome {
    Finalized {
        result: TaskResultRecord,
        acceptance: ene_task::TaskResultAcceptance,
    },
    Refused(TaskAgentRunRefusal),
    NotSent(TaskAgentNotSent),
    ConsentStaleAfterSend {
        turn: u32,
    },
    ProtocolViolation {
        turn: u32,
        reason: TaskAgentProtocolViolation,
    },
    TurnLimitReached {
        turns: u32,
    },
    EffectUnresolved {
        attempt: ene_action::ActionAttemptId,
    },
    Cancelled,
    HostShutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentRunError {
    #[error(transparent)]
    Turn(#[from] ene_task::TaskAgentTurnError),
    #[error(transparent)]
    Action(#[from] WorkspaceActionHostError),
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    #[error("result body scrub unavailable: {reason}")]
    ResultScrubUnavailable { reason: String },
}

impl From<ene_task::TaskTechnicalError> for TaskAgentRunError {
    fn from(error: ene_task::TaskTechnicalError) -> Self {
        match error {
            ene_task::TaskTechnicalError::StorageUnavailable { reason } => {
                Self::StorageUnavailable { reason }
            }
        }
    }
}

pub(crate) async fn run_task_agent_execution(
    store: &Store,
    instructions: &impl TaskInstructionSource,
    inference: &impl TaskAgentInference,
    scrubber: &impl SecretScrubber,
    effect_runtime: &TaskEffectRuntime,
    max_turns: u32,
    registration: &TaskExecutionRegistration<'_>,
) -> Result<TaskAgentRunOutcome, TaskAgentRunError> {
    let delegation = registration.delegation;
    if let Some(outcome) = registration.stop_outcome() {
        return Ok(outcome);
    }
    if execution_already_started(store, delegation).await? {
        return Ok(TaskAgentRunOutcome::Refused(
            TaskAgentRunRefusal::ExecutionAlreadyStarted { delegation },
        ));
    }
    let mut exchanges: Vec<TaskAgentActionExchange> = Vec::new();
    let mut attempt_refs: Vec<RawId> = Vec::new();
    let mut turn = 0_u32;
    loop {
        if let Some(outcome) = registration.stop_outcome() {
            return Ok(outcome);
        }
        if turn >= max_turns {
            return Ok(TaskAgentRunOutcome::TurnLimitReached { turns: turn });
        }
        turn += 1;
        let outcome = orchestrate_task_agent_turn(
            store,
            instructions,
            inference,
            scrubber,
            TaskAgentTurnPremise {
                delegation,
                exchanges: exchanges.clone(),
            },
        )
        .await?;
        let produced = match outcome {
            TaskAgentTurnOutcome::Produced(produced) => produced,
            TaskAgentTurnOutcome::StaleTaskRevision { current } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::StaleTaskRevision { current },
                ));
            }
            TaskAgentTurnOutcome::MissingTask { task } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::MissingTask { task },
                ));
            }
            TaskAgentTurnOutcome::MissingDelegation { delegation } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::MissingDelegation { delegation },
                ));
            }
            TaskAgentTurnOutcome::TaskTerminal { task, progress } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::TaskTerminal { task, progress },
                ));
            }
            TaskAgentTurnOutcome::ExecutionSealed { delegation } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::ExecutionSealed { delegation },
                ));
            }
            TaskAgentTurnOutcome::InstructionSourceMissing { entry, source } => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::InstructionSourceMissing { entry, source },
                ));
            }
            TaskAgentTurnOutcome::NotSent(reason) => {
                return Ok(TaskAgentRunOutcome::NotSent(reason));
            }
            TaskAgentTurnOutcome::Aborted => {
                return Ok(if registration.cancellation.is_aborted() {
                    TaskAgentRunOutcome::Cancelled
                } else {
                    TaskAgentRunOutcome::HostShutdown
                });
            }
        };
        if !produced.adoption_consent_current {
            return Ok(TaskAgentRunOutcome::ConsentStaleAfterSend { turn });
        }
        match parse_directive(produced.output.text()) {
            Err(reason) => return Ok(TaskAgentRunOutcome::ProtocolViolation { turn, reason }),
            Ok(TaskAgentDirective::Finish { body }) => {
                let arrival = finalize_result_body(store, scrubber, delegation, &body).await?;
                let result = match arrival {
                    TaskResultArrivalOutcome::Recorded(result) => result,
                    TaskResultArrivalOutcome::StaleCredentialSet { current } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::StaleCredentialSet { current },
                        ));
                    }
                };
                let acceptance = store
                    .adopt_result(ene_task::TaskResultAdoptionClaim {
                        result: result.result,
                        attempt_refs,
                    })
                    .await?;
                return Ok(TaskAgentRunOutcome::Finalized { result, acceptance });
            }
            Ok(TaskAgentDirective::Act(request)) => {
                if let Some(outcome) = registration.stop_outcome() {
                    return Ok(outcome);
                }
                let action = run_workspace_action(
                    store,
                    registration.task_executions(),
                    effect_runtime,
                    registration.dispatch_abort(),
                    delegation,
                    request.operation,
                    request.path.clone(),
                    request.content.clone().map(String::into_bytes),
                )
                .await?;
                match action {
                    WorkspaceActionHostOutcome::Stopped => {
                        return Ok(if registration.cancellation.is_aborted() {
                            TaskAgentRunOutcome::Cancelled
                        } else {
                            TaskAgentRunOutcome::HostShutdown
                        });
                    }
                    WorkspaceActionHostOutcome::Completed {
                        attempt,
                        effect,
                        fact_recorded,
                    } => {
                        attempt_refs.push(attempt.as_raw());
                        if unconfirmed_effect(&effect) {
                            return Ok(TaskAgentRunOutcome::EffectUnresolved { attempt });
                        }
                        let text = completed_observation(&effect, fact_recorded);
                        let observed = observed_workspace_body(&effect).then(|| text.clone());
                        let occurrence = record_task_observation(
                            store,
                            delegation,
                            Some(attempt.as_raw()),
                            observed,
                        )
                        .await?;
                        let text =
                            replay_observation_text(store, delegation, &effect, text).await?;
                        exchanges.push(TaskAgentActionExchange {
                            request: produced.output,
                            observation: TaskAgentObservation::new(occurrence, text),
                        });
                    }
                    WorkspaceActionHostOutcome::NotStarted(reason) => {
                        let text = not_started_observation(&reason);
                        let occurrence =
                            record_task_observation(store, delegation, None, None).await?;
                        exchanges.push(TaskAgentActionExchange {
                            request: produced.output,
                            observation: TaskAgentObservation::new(occurrence, text),
                        });
                    }
                    WorkspaceActionHostOutcome::MissingDelegation { delegation } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::MissingDelegation { delegation },
                        ));
                    }
                    WorkspaceActionHostOutcome::MissingTask { task } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::MissingTask { task },
                        ));
                    }
                    WorkspaceActionHostOutcome::StaleTaskRevision { current } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::StaleTaskRevision { current },
                        ));
                    }
                    WorkspaceActionHostOutcome::TaskTerminal { task, progress } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::TaskTerminal { task, progress },
                        ));
                    }
                    WorkspaceActionHostOutcome::ExecutionSealed { delegation } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::ExecutionSealed { delegation },
                        ));
                    }
                    WorkspaceActionHostOutcome::MissingWorkspace { task } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::MissingWorkspace { task },
                        ));
                    }
                    WorkspaceActionHostOutcome::WorkspaceUnavailable { task } => {
                        return Ok(TaskAgentRunOutcome::Refused(
                            TaskAgentRunRefusal::WorkspaceUnavailable { task },
                        ));
                    }
                }
            }
        }
    }
}

const FINAL_RESULT_SCRUB_ATTEMPTS: u32 = 3;

async fn finalize_result_body(
    store: &Store,
    scrubber: &impl SecretScrubber,
    delegation: ene_task::DelegationId,
    body: &str,
) -> Result<TaskResultArrivalOutcome, TaskAgentRunError> {
    let mut attempts = 0_u32;
    loop {
        attempts += 1;
        let scrubbed =
            scrubber
                .scrub(body)
                .await
                .map_err(|_| TaskAgentRunError::ResultScrubUnavailable {
                    reason: String::from("credential scrub failed"),
                })?;
        let arrival = orchestrate_result_arrival(
            store,
            delegation,
            TaskResultScrubPremise::from_scrubbed(scrubbed),
        )
        .await?;
        match arrival {
            TaskResultArrivalOutcome::Recorded(_) => return Ok(arrival),
            TaskResultArrivalOutcome::StaleCredentialSet { current } => {
                if attempts == FINAL_RESULT_SCRUB_ATTEMPTS {
                    return Ok(TaskResultArrivalOutcome::StaleCredentialSet { current });
                }
            }
        }
    }
}

async fn execution_already_started(
    store: &Store,
    delegation: ene_task::DelegationId,
) -> Result<bool, TaskAgentRunError> {
    let Some(correspondence) = store.load_delegation(delegation).await? else {
        return Ok(false);
    };
    let Some(record) = store.load_task(correspondence.task.task).await? else {
        return Ok(false);
    };
    if record.task.reference != correspondence.task || record.task.progress.is_terminal() {
        return Ok(false);
    }
    if store.load_delegation_result(delegation).await?.is_some() {
        return Ok(false);
    }
    Ok(store.delegation_has_started_work(delegation).await?)
}

fn unconfirmed_effect(effect: &ObservedEffect) -> bool {
    effect.certainty == ActionCertainty::Unknown
}

async fn record_task_observation(
    store: &Store,
    delegation: ene_task::DelegationId,
    attempt: Option<RawId>,
    observed: Option<String>,
) -> Result<TaskAgentObservationId, TaskAgentRunError> {
    Ok(store
        .record_task_agent_observation(TaskAgentObservationPremise {
            observation: TaskAgentObservationId::generate(),
            delegation,
            attempt,
            observed,
            observed_at: WallClockWithTz::now(),
        })
        .await?)
}

fn observed_workspace_body(effect: &ObservedEffect) -> bool {
    matches!(
        effect.output,
        Some(ActionOutput::Bytes(_) | ActionOutput::Listing(_))
    )
}

async fn replay_observation_text(
    store: &Store,
    delegation: ene_task::DelegationId,
    effect: &ObservedEffect,
    text: String,
) -> Result<String, TaskAgentRunError> {
    if !observed_workspace_body(effect) {
        return Ok(text);
    }
    match store.task_delegation_held(delegation.as_raw()).await {
        Ok(true) => Ok(String::from(
            "the observed workspace content was discarded because its producing execution is associated with a deletion interval",
        )),
        Ok(false) => Ok(text),
        Err(error) => Err(TaskAgentRunError::StorageUnavailable {
            reason: error.to_string(),
        }),
    }
}

fn completed_observation(effect: &ObservedEffect, fact_recorded: bool) -> String {
    let mut text = match (&effect.certainty, &effect.output) {
        (ActionCertainty::ConfirmedSuccess, Some(ene_action::ActionOutput::Bytes(bytes))) => {
            format!("read ok:\n{}", String::from_utf8_lossy(bytes))
        }
        (ActionCertainty::ConfirmedSuccess, Some(ene_action::ActionOutput::Listing(entries))) => {
            let mut listing = String::from("listed:\n");
            for entry in entries {
                listing.push_str(&format!("{}/{:?}\n", entry.name, entry.kind));
            }
            listing
        }
        (ActionCertainty::ConfirmedSuccess, Some(ene_action::ActionOutput::Created { .. })) => {
            String::from("create ok")
        }
        (ActionCertainty::ConfirmedSuccess, Some(ene_action::ActionOutput::Updated)) => {
            String::from("edit ok")
        }
        (ActionCertainty::ConfirmedSuccess, None) => {
            String::from("the operation was confirmed but produced no output")
        }
        (ActionCertainty::ConfirmedFailure, _) => {
            String::from("refused before changing the target")
        }
        (ActionCertainty::Unknown, _) => String::from("the outcome could not be verified"),
    };
    if !fact_recorded {
        text.push_str(
            "\n(the observation could not be stored; the durable record stays unverified)",
        );
    }
    text
}

fn not_started_observation(reason: &ActionNotStarted) -> String {
    match reason {
        ActionNotStarted::Rejected(rejection) => format!("refused: {rejection}"),
        ActionNotStarted::MissingContent => String::from("refused: content is required"),
        ActionNotStarted::ContentNotAllowed => String::from("refused: content is not allowed"),
        ActionNotStarted::Denied(code) => format!("denied: {code:?}"),
        ActionNotStarted::NeedsRevalidation => String::from("refused: revalidation needed"),
        ActionNotStarted::StalePremise => String::from("refused: the task premise moved"),
        ActionNotStarted::TaskTerminal => String::from("refused: the task is terminal"),
        ActionNotStarted::ExecutionSealed => String::from("refused: the execution is sealed"),
        ActionNotStarted::DataUseHeld => String::from("refused: the target is under deletion"),
    }
}

pub fn parse_directive(text: &str) -> Result<TaskAgentDirective, TaskAgentProtocolViolation> {
    use TaskAgentProtocolViolation as Violation;

    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|_| Violation::NotAJsonObject)?;
    let serde_json::Value::Object(map) = value else {
        return Err(Violation::NotAJsonObject);
    };
    let has_tool = map.contains_key("tool");
    let has_final = map.contains_key("final");
    match (has_tool, has_final) {
        (false, false) => return Err(Violation::MissingDirective),
        (true, true) => return Err(Violation::ConflictingDirective),
        (true, false) => {}
        (false, true) => {
            if map.len() != 1 {
                return Err(Violation::ConflictingDirective);
            }
            let Some(body) = map.get("final").and_then(serde_json::Value::as_str) else {
                return Err(Violation::InvalidFields);
            };
            return Ok(TaskAgentDirective::Finish {
                body: body.to_owned(),
            });
        }
    }
    let Some(tool) = map.get("tool").and_then(serde_json::Value::as_str) else {
        return Err(Violation::InvalidFields);
    };
    let operation = match tool {
        "list" => OperationKind::List,
        "read" => OperationKind::Read,
        "create" => OperationKind::Create,
        "edit" => OperationKind::Edit,
        _ => return Err(Violation::UnknownTool),
    };
    let allowed_fields = match operation {
        OperationKind::List | OperationKind::Read => 2,
        OperationKind::Create | OperationKind::Edit => 3,
    };
    if map.len() > allowed_fields {
        return Err(Violation::ConflictingDirective);
    }
    let Some(path) = map.get("path").and_then(serde_json::Value::as_str) else {
        return Err(Violation::InvalidFields);
    };
    let content = match operation {
        OperationKind::List | OperationKind::Read => None,
        OperationKind::Create | OperationKind::Edit => {
            let Some(content) = map.get("content").and_then(serde_json::Value::as_str) else {
                return Err(Violation::InvalidFields);
            };
            Some(content.to_owned())
        }
    };
    Ok(TaskAgentDirective::Act(TaskAgentActionRequest {
        operation,
        path: path.to_owned(),
        content,
    }))
}

pub trait TaskAgentLauncher: Send + Sync {
    fn launch(&self, delegation: ene_task::DelegationId);
}

type TaskAgentJoinResult = Result<TaskAgentRunOutcome, TaskAgentRunError>;

pub struct BackgroundTaskAgent<T> {
    handle: std::sync::Weak<HostHandle>,
    transport: Arc<T>,
    tasks: std::sync::Mutex<Option<tokio::task::JoinSet<TaskAgentJoinResult>>>,
    shutdown: tokio::sync::Mutex<()>,
    failure: std::sync::Mutex<Option<CoreError>>,
    quiesce_timeout: std::sync::Mutex<std::time::Duration>,
}

impl<T> BackgroundTaskAgent<T> {
    #[must_use]
    pub fn new(handle: Arc<HostHandle>, transport: Arc<T>) -> Self {
        Self {
            handle: Arc::downgrade(&handle),
            transport,
            tasks: std::sync::Mutex::new(Some(tokio::task::JoinSet::new())),
            shutdown: tokio::sync::Mutex::new(()),
            failure: std::sync::Mutex::new(None),
            quiesce_timeout: std::sync::Mutex::new(TASK_AGENT_QUIESCE_TIMEOUT),
        }
    }

    pub(crate) fn abort(&self) {
        if let Some(handle) = self.handle.upgrade() {
            handle
                .task_executions
                .abort_for_host_shutdown_without_commit_scope();
        }
        let tasks = crate::lock_unpoison(&self.tasks).take();
        drop(tasks);
    }

    pub(crate) async fn shutdown_and_join(&self) -> Result<(), CoreError> {
        let _shutdown = self.shutdown.lock().await;
        let tasks = crate::lock_unpoison(&self.tasks).take();
        let handle = self.handle.upgrade();
        if let Some(handle) = &handle {
            handle.close_task_effect_admission();
        }
        self.signal_host_shutdown().await;
        let Some(mut tasks) = tasks else {
            let mut failure = crate::lock_unpoison(&self.failure).take();
            if let Some(handle) = &handle {
                if let Err(error) = handle.terminate_and_join_task_effects().await {
                    record_core_failure(&mut failure, error);
                }
                if let Err(error) = handle.verify_task_effect_shutdown() {
                    record_core_failure(&mut failure, error);
                }
            }
            return failure.map_or(Ok(()), Err);
        };
        let mut failure = crate::lock_unpoison(&self.failure).take();
        let quiesce_timeout = handle.as_ref().map_or_else(
            || *crate::lock_unpoison(&self.quiesce_timeout),
            |handle| handle.task_agent_quiesce_timeout(),
        );
        let joined = tokio::time::timeout(quiesce_timeout, async {
            while let Some(result) = tasks.join_next().await {
                record_task_agent_result(&mut failure, result);
            }
        })
        .await;
        if joined.is_err() {
            if let Some(handle) = &handle
                && let Err(error) = handle.terminate_and_join_task_effects().await
            {
                record_core_failure(&mut failure, error);
            }
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                record_task_agent_result(&mut failure, result);
            }
            if let Some(handle) = &handle
                && let Err(error) = handle.verify_task_effect_shutdown()
            {
                record_core_failure(&mut failure, error);
            }
            return failure.map_or(Ok(()), Err);
        }
        if let Some(handle) = &handle {
            if let Err(error) = handle.terminate_and_join_task_effects().await {
                record_core_failure(&mut failure, error);
            }
            if let Err(error) = handle.verify_task_effect_shutdown() {
                record_core_failure(&mut failure, error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn signal_host_shutdown(&self) {
        if let Some(handle) = self.handle.upgrade() {
            handle.abort_task_agents_for_host_shutdown().await;
        }
    }
}

fn record_core_failure(failure: &mut Option<CoreError>, error: CoreError) {
    failure.get_or_insert(error);
}

fn record_task_agent_result(
    failure: &mut Option<CoreError>,
    result: Result<TaskAgentJoinResult, tokio::task::JoinError>,
) {
    match result {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            failure.get_or_insert_with(|| {
                CoreError::Serving(format!("Task Agent technical failure: {error}"))
            });
        }
        Err(_) => {
            failure.get_or_insert_with(task_agent_join_failure);
        }
    }
}

fn task_agent_join_failure() -> CoreError {
    CoreError::Serving(String::from("Task Agent runner panicked or was cancelled"))
}

impl<T> BackgroundTaskAgent<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    fn try_launch(&self, delegation: ene_task::DelegationId) -> Result<(), TaskAgentRunRefusal> {
        let mut tasks = crate::lock_unpoison(&self.tasks);
        let Some(handle) = self.handle.upgrade() else {
            return Err(TaskAgentRunRefusal::ExecutionUnavailable { delegation });
        };
        let Some(tasks) = tasks.as_mut() else {
            handle.task_executions.release(delegation);
            return Err(TaskAgentRunRefusal::ExecutionUnavailable { delegation });
        };
        while let Some(result) = tasks.try_join_next() {
            let mut failure = crate::lock_unpoison(&self.failure);
            record_task_agent_result(&mut failure, result);
        }
        let transport = Arc::clone(&self.transport);
        tasks.spawn(async move {
            let result = handle.run_task_agent(transport.as_ref(), delegation).await;
            handle.task_executions.release(delegation);
            result
        });
        Ok(())
    }
}

impl<T> TaskAgentLauncher for BackgroundTaskAgent<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    fn launch(&self, delegation: ene_task::DelegationId) {
        drop(self.try_launch(delegation));
    }
}

#[cfg(test)]
mod launcher_tests {
    use super::*;

    struct NoProvider;

    impl ProviderTransport for NoProvider {
        fn complete_streaming<'a>(
            &'a self,
            _request: ene_inference::ProviderRequest,
            _sink: &'a mut (dyn ene_inference::DeltaSink + Send),
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            ene_inference::ProviderResponse,
                            ene_inference::InferenceTechnicalError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn host_shutdown_abort_is_distinct_from_task_cancellation() {
        let registry = TaskExecutionRegistry::default();
        let task = ene_task::TaskId::generate();
        let delegation = ene_task::DelegationId::generate();
        assert!(registry.reserve(delegation, task));
        let TakeReservation::Admitted(registration) = registry.take_reservation(delegation, task)
        else {
            panic!("the reservation must enter the running registry");
        };

        registry.abort_for_host_shutdown().await;

        assert!(registration.dispatch_abort.is_aborted());
        assert!(!registration.cancellation.is_aborted());
        assert!(registration.host_shutdown.is_aborted());
        assert_eq!(
            registration.stop_outcome(),
            Some(TaskAgentRunOutcome::HostShutdown)
        );
    }

    #[test]
    fn task_cancellation_aborts_dispatch_without_relabelling_host_shutdown() {
        let registry = TaskExecutionRegistry::default();
        let task = ene_task::TaskId::generate();
        let delegation = ene_task::DelegationId::generate();
        assert!(registry.reserve(delegation, task));
        let TakeReservation::Admitted(registration) = registry.take_reservation(delegation, task)
        else {
            panic!("the reservation must enter the running registry");
        };

        assert!(registry.cancel(task));

        assert!(registration.dispatch_abort.is_aborted());
        assert!(registration.cancellation.is_aborted());
        assert!(!registration.host_shutdown.is_aborted());
        assert_eq!(
            registration.stop_outcome(),
            Some(TaskAgentRunOutcome::Cancelled)
        );
    }

    #[tokio::test]
    async fn shutdown_joins_started_blocking_work_and_closes_launch_admission() {
        use std::future::Future as _;
        use std::task::Poll;

        let directory = tempfile::tempdir().unwrap();
        let handle = Arc::new(
            HostHandle::open_with_cred_store(
                directory.path(),
                crate::serve::CredStore::Memory(ene_credential::MemoryCredentialStore::new()),
            )
            .await
            .unwrap(),
        );
        let launcher = Arc::new(BackgroundTaskAgent::new(
            Arc::clone(&handle),
            Arc::new(NoProvider),
        ));
        assert!(handle.install_task_launcher(launcher.clone()));
        let weak = Arc::downgrade(&handle);
        let task = ene_task::TaskId::generate();
        let delegation = ene_task::DelegationId::generate();
        assert!(handle.task_executions.reserve(delegation, task));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(&handle);
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        crate::lock_unpoison(&launcher.tasks)
            .as_mut()
            .unwrap()
            .spawn(async move {
                tokio::task::spawn_blocking(move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    worker_finished.store(true, std::sync::atomic::Ordering::SeqCst);
                })
                .await
                .unwrap();
                drop(worker);
                Ok(TaskAgentRunOutcome::Cancelled)
            });
        entered_rx.await.unwrap();
        let mut joining = std::pin::pin!(launcher.shutdown_and_join());
        std::future::poll_fn(|cx| {
            assert!(joining.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(
            launcher.try_launch(delegation),
            Err(TaskAgentRunRefusal::ExecutionUnavailable { delegation })
        );
        assert!(!handle.task_executions.task_has_reservation_or_running(task));
        let mut second_join = std::pin::pin!(launcher.shutdown_and_join());
        std::future::poll_fn(|cx| {
            assert!(second_join.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(handle);
        assert!(weak.upgrade().is_some());
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        joining.await.unwrap();
        second_join.await.unwrap();
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn admitted_launch_is_owned_until_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let handle = Arc::new(
            HostHandle::open_with_cred_store(
                directory.path(),
                crate::serve::CredStore::Memory(ene_credential::MemoryCredentialStore::new()),
            )
            .await
            .unwrap(),
        );
        let launcher = Arc::new(BackgroundTaskAgent::new(
            Arc::clone(&handle),
            Arc::new(NoProvider),
        ));
        assert!(handle.install_task_launcher(launcher.clone()));
        let weak = Arc::downgrade(&handle);
        launcher
            .try_launch(ene_task::DelegationId::generate())
            .unwrap();
        assert_eq!(
            crate::lock_unpoison(&launcher.tasks)
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        drop(handle);
        assert!(weak.upgrade().is_some());
        launcher.shutdown_and_join().await.unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn dropping_launcher_aborts_owned_async_work() {
        let launcher = BackgroundTaskAgent {
            handle: std::sync::Weak::new(),
            transport: Arc::new(NoProvider),
            tasks: std::sync::Mutex::new(Some(tokio::task::JoinSet::new())),
            shutdown: tokio::sync::Mutex::new(()),
            failure: std::sync::Mutex::new(None),
            quiesce_timeout: std::sync::Mutex::new(TASK_AGENT_QUIESCE_TIMEOUT),
        };
        let (held, released) = tokio::sync::oneshot::channel::<()>();
        crate::lock_unpoison(&launcher.tasks)
            .as_mut()
            .unwrap()
            .spawn(async move {
                std::future::pending::<()>().await;
                drop(held);
                std::future::pending::<TaskAgentJoinResult>().await
            });
        drop(launcher);
        assert!(released.await.is_err());
    }

    #[tokio::test]
    async fn emergency_abort_breaks_running_host_ownership_cycle() {
        let directory = tempfile::tempdir().unwrap();
        let handle = Arc::new(
            HostHandle::open_with_cred_store(
                directory.path(),
                crate::serve::CredStore::Memory(ene_credential::MemoryCredentialStore::new()),
            )
            .await
            .unwrap(),
        );
        let launcher = Arc::new(BackgroundTaskAgent::new(
            Arc::clone(&handle),
            Arc::new(NoProvider),
        ));
        assert!(handle.install_task_launcher(launcher.clone()));
        let weak = Arc::downgrade(&handle);
        let (held, released) = tokio::sync::oneshot::channel::<()>();
        crate::lock_unpoison(&launcher.tasks)
            .as_mut()
            .unwrap()
            .spawn(async move {
                std::future::pending::<()>().await;
                drop(handle);
                drop(held);
                std::future::pending::<TaskAgentJoinResult>().await
            });
        assert!(weak.upgrade().is_some());
        launcher.abort();
        assert!(released.await.is_err());
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn domain_stop_outcomes_are_not_task_agent_technical_failures() {
        let launcher = BackgroundTaskAgent {
            handle: std::sync::Weak::new(),
            transport: Arc::new(NoProvider),
            tasks: std::sync::Mutex::new(Some(tokio::task::JoinSet::new())),
            shutdown: tokio::sync::Mutex::new(()),
            failure: std::sync::Mutex::new(None),
            quiesce_timeout: std::sync::Mutex::new(TASK_AGENT_QUIESCE_TIMEOUT),
        };
        {
            let mut registry = crate::lock_unpoison(&launcher.tasks);
            let tasks = registry.as_mut().unwrap();
            tasks.spawn(async { Ok(TaskAgentRunOutcome::Cancelled) });
            tasks.spawn(async { Ok(TaskAgentRunOutcome::HostShutdown) });
        }

        launcher.shutdown_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn task_agent_technical_failure_reaches_shutdown_accounting() {
        let launcher = BackgroundTaskAgent {
            handle: std::sync::Weak::new(),
            transport: Arc::new(NoProvider),
            tasks: std::sync::Mutex::new(Some(tokio::task::JoinSet::new())),
            shutdown: tokio::sync::Mutex::new(()),
            failure: std::sync::Mutex::new(None),
            quiesce_timeout: std::sync::Mutex::new(TASK_AGENT_QUIESCE_TIMEOUT),
        };
        {
            let mut registry = crate::lock_unpoison(&launcher.tasks);
            registry.as_mut().unwrap().spawn(async {
                Err(TaskAgentRunError::Action(
                    WorkspaceActionHostError::EffectUnavailable {
                        reason: String::from("staging cleanup failed"),
                    },
                ))
            });
        }

        let error = launcher
            .shutdown_and_join()
            .await
            .expect_err("technical task failure must reach shutdown");
        assert!(error.to_string().contains("Task Agent technical failure"));
        assert!(error.to_string().contains("staging cleanup failed"));
    }

    #[tokio::test]
    async fn join_failure_is_returned_only_after_remaining_children_finish() {
        use std::future::Future as _;
        use std::task::Poll;

        let launcher = BackgroundTaskAgent {
            handle: std::sync::Weak::new(),
            transport: Arc::new(NoProvider),
            tasks: std::sync::Mutex::new(Some(tokio::task::JoinSet::new())),
            shutdown: tokio::sync::Mutex::new(()),
            failure: std::sync::Mutex::new(None),
            quiesce_timeout: std::sync::Mutex::new(TASK_AGENT_QUIESCE_TIMEOUT),
        };
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let (panicking, panicked) = tokio::sync::oneshot::channel::<()>();
        {
            let mut tasks = crate::lock_unpoison(&launcher.tasks);
            let tasks = tasks.as_mut().unwrap();
            tasks.spawn(async move {
                let _panicking = panicking;
                panic!("test runner panic");
            });
            tasks.spawn(async move {
                wait.await.unwrap();
                Ok(TaskAgentRunOutcome::Cancelled)
            });
        }
        assert!(panicked.await.is_err());
        let mut joining = std::pin::pin!(launcher.shutdown_and_join());
        std::future::poll_fn(|cx| {
            assert!(joining.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        release.send(()).unwrap();
        assert!(matches!(joining.await, Err(CoreError::Serving(_))));
    }
}
