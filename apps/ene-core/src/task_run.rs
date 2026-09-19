//! Autonomous Task Agent ↔ Action execution loop (Stage 4 slice D).
//!
//! One [`DelegationId`](ene_task::DelegationId) is one delegated Task Agent
//! execution lifetime
//! (0..N inference turns, 0..N Action attempts, 0..1 final result). This
//! module runs that lifetime end to end through the existing owner
//! boundaries and adds no new admission, state, or storage:
//!
//! 1. Each turn goes through [`orchestrate_task_agent_turn`], which resolves
//!    the adopted input and whose port owns the AU14 attempt claim; the
//!    provider is only reached after the claim commits.
//! 2. The provider output is parsed against the fixed response protocol the
//!    harness preamble states. An Action request runs through
//!    [`run_workspace_action`] (AU5 + the workspace filesystem boundary); a
//!    final answer goes through the explicit finalization boundary
//!    (`orchestrate_result_arrival`, AU15a) and then adoption
//!    ([`TaskRepository::adopt_result`](ene_task::TaskRepository::adopt_result), AU15b).
//! 3. Action observations are replayed into the next turn as execution-local
//!    transcript. The observation body is never persisted; each occurrence's
//!    durable, body-free provenance row (identity, delegation/execution
//!    correlation, producing attempt, workspace path) is recorded before the
//!    occurrence is replayed, and its identity joins the consuming turn's
//!    `data_use`. The only durable result body is the `task_result` row.
//!
//! The loop is bounded ([`DEFAULT_MAX_TURNS`]) and never treats a provider
//! output as final unless the model says so with the final directive, so an
//! intermediate turn can never seal the execution. A malformed response stops
//! the execution without an Action and without a result record; the Task
//! stays non-terminal and the user can steer or cancel. A provider output
//! whose adoption consent lapsed during the network wait is discarded the
//! same way (no Action, no result record), while its already-started attempt
//! stays durable.
//!
//! An Action whose effect the executor could not confirm
//! ([`ActionCertainty::Unknown`]) stops the loop: the unknown outcome is kept
//! as it is and the same effect is never re-executed automatically. Cancelled
//! Tasks and moved revisions are refused by the existing gates on the next
//! turn or Action start and end the loop as [`TaskAgentRunRefusal`].
//!
//! A local cooperative stop signal ([`DispatchAbort`]) stops the loop before
//! the next provider call or Action and aborts an in-flight provider call
//! best-effort. The abort travels through the inference port so a claimed
//! attempt records its unknown-usage fact before the turn answers; the loop
//! never drops the turn future itself, which would skip that accounting. The
//! signal is never authority: the durable AU16 admission is the Task owner's
//! commit, and aborting locally is not evidence that a provider request or
//! external effect stopped.
//!
//! What this module deliberately does not do: it does not create Tasks or
//! delegations, does not retry provider calls or Actions, does not resume
//! anything after restart, and does not decide Task completion itself (the
//! Task owner's adoption does). A delegated execution is run once: when it
//! stops without a final result (protocol violation, turn bound, unconfirmed
//! effect, cancel), the loop entry refuses to run it again over the same
//! delegation on the durable attempt facts (the first claimed inference turn
//! or Action start is the start marker) — continued work is a new delegation
//! (the design's execution lifetime), and a cancelled Task is re-executed
//! only as a new Task.

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

use crate::action::{WorkspaceActionHostError, WorkspaceActionHostOutcome, run_workspace_action};
use crate::serve::{CoreError, HostHandle};

/// The default bound on inference turns per execution.
///
/// The bound exists so a model that never emits a final answer cannot loop
/// forever; reaching it stops the execution without sealing a result.
pub const DEFAULT_MAX_TURNS: u32 = 16;

/// Tracks the running Task Agent executions of this process, keyed by
/// delegation (one delegated execution lifetime).
///
/// The registry is not durable state: a restart drops it, and losing a local
/// token never means an execution or effect stopped. Several executions of
/// one Task are allowed (AU3 allows several delegations of one revision); a
/// cancel signals every token registered for that Task, and a registration
/// removes only its own entry.
///
/// Registration is atomic per delegation: a second registration for the same
/// delegation is refused while the first is held, so two loops can never
/// start provider calls or Actions for one execution lifetime. The durable
/// attempt facts ([`TaskAgentRunRefusal::ExecutionAlreadyStarted`]) cover
/// what the in-memory registry cannot: a restart or a lost registration.
///
/// Launch reservations ([`TaskExecutionRegistry::reserve`]) are the other
/// half: an AU3/AU17 producer records the committed delegation here in the
/// same commit scope as the store commit, and the runner only starts a
/// delegation holding a reservation. A restart drops every reservation, so
/// an old delegation never launches without a new explicit commit; the
/// commit succeeding while the later spawn fails releases the reservation
/// with no automatic relaunch.
#[derive(Default)]
pub struct TaskExecutionRegistry {
    running: std::sync::Mutex<std::collections::HashMap<ene_task::DelegationId, RunningExecution>>,
    reservations:
        std::sync::Mutex<std::collections::HashMap<ene_task::DelegationId, ene_task::TaskId>>,
    /// Serializes the Host-side launch critical section (CCT §7.4): the
    /// reservation/registration check, the store commit, and the new
    /// reservation record share one scope per process. Lock order is always
    /// registry scope first, then SQLite inside the store's blocking
    /// section; the scope never crosses a SQLite transaction boundary on
    /// this thread, and no provider I/O, external effect, or prompt
    /// assembly runs under it.
    commit_scope: tokio::sync::Mutex<()>,
}

/// Admission answer of one launch reservation take.
pub enum TakeReservation<'a> {
    /// The reservation was consumed and the execution registered.
    Admitted(TaskExecutionRegistration<'a>),
    /// Another execution of the same delegation is already running.
    AlreadyRunning,
    /// No launch reservation covers the delegation in this process.
    Unreserved,
}

struct RunningExecution {
    task: ene_task::TaskId,
    cancellation: DispatchAbort,
}

impl TaskExecutionRegistry {
    /// Registers one running execution and returns its cancellable handle.
    ///
    /// `None` means another execution of the same delegation is already
    /// running in this process; the caller must refuse before any provider
    /// call or Action. The non-overwrite is part of the same lock section as
    /// the insert, so two racers cannot both observe "not running". The
    /// returned registration removes its own entry on drop, so an abandoned
    /// future cannot leave a stale token behind.
    pub fn register(
        &self,
        delegation: ene_task::DelegationId,
        task: ene_task::TaskId,
    ) -> Option<TaskExecutionRegistration<'_>> {
        let mut running = crate::lock_unpoison(&self.running);
        let entry = match running.entry(delegation) {
            std::collections::hash_map::Entry::Occupied(_) => return None,
            std::collections::hash_map::Entry::Vacant(entry) => entry,
        };
        let cancellation = DispatchAbort::default();
        entry.insert(RunningExecution {
            task,
            cancellation: cancellation.clone(),
        });
        Some(TaskExecutionRegistration {
            registry: self,
            delegation,
            cancellation,
        })
    }

    /// Signals every running execution of `task`, if any is registered.
    ///
    /// Returns whether at least one token was signalled; `false` means no
    /// execution of this Task is running in this process, which says nothing
    /// about durable work.
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

    /// Enters the Host-side launch critical section (CCT §7.4).
    ///
    /// The holder runs the reservation/registration check, the store
    /// commit, and the new reservation record under this scope, so an
    /// AU3/AU17 commit and its launch reservation are one atomic section
    /// against every other producer in this process. The scope is a Tokio
    /// mutex: it is held across the store commit's `.await` (which parks
    /// this task while the store's blocking thread owns SQLite briefly),
    /// but never across provider I/O, external effects, or prompt
    /// assembly, and never in the opposite order with SQLite.
    pub async fn commit_scope(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.commit_scope.lock().await
    }

    /// Records the launch reservation for one committed delegation.
    ///
    /// Call under [`Self::commit_scope`] right after the store commit that
    /// created the delegation. Returns `false` — recording nothing — when
    /// the delegation is already reserved or running: the caller must treat
    /// that as a lost race with no automatic relaunch. The check is
    /// delegation-scoped on purpose: steering a running Task commits a new
    /// revision with its own delegation while the old execution is still
    /// stopping on the revision gate, so a Task-wide refusal here would
    /// block every steering of a running Task. The Task-wide "same Task
    /// already launching" question stays a separate
    /// [`Self::task_has_reservation_or_running`] read under the same commit
    /// scope, where the resume admission orders it. A reservation is
    /// consumed by [`Self::take_reservation`], released by [`Self::release`],
    /// and dropped by a restart.
    pub fn reserve(&self, delegation: ene_task::DelegationId, task: ene_task::TaskId) -> bool {
        let running = crate::lock_unpoison(&self.running);
        let mut reservations = crate::lock_unpoison(&self.reservations);
        if running.contains_key(&delegation) || reservations.contains_key(&delegation) {
            return false;
        }
        reservations.insert(delegation, task);
        true
    }

    /// Releases one launch reservation without starting anything.
    ///
    /// Used when the commit succeeded but the later spawn failed or no
    /// runner exists: the delegation stays durable and unexecuted, and a
    /// retry is a new explicit commit, never an automatic relaunch.
    pub fn release(&self, delegation: ene_task::DelegationId) {
        crate::lock_unpoison(&self.reservations).remove(&delegation);
    }

    /// Whether the Task has any launch reservation or running registration
    /// in this process.
    ///
    /// Read under [`Self::commit_scope`] as the resume "same Task" check:
    /// memory-only, never durable authority, and never evidence that an
    /// effect stopped.
    pub fn task_has_reservation_or_running(&self, task: ene_task::TaskId) -> bool {
        crate::lock_unpoison(&self.running)
            .values()
            .any(|execution| execution.task == task)
            || crate::lock_unpoison(&self.reservations)
                .values()
                .any(|reserved| *reserved == task)
    }

    /// Consumes one launch reservation and registers the execution.
    ///
    /// `AlreadyRunning` wins over `Unreserved`: a delegation that is
    /// already running in this process is refused even if its reservation
    /// row is somehow also present. An `Unreserved` delegation never starts
    /// provider calls or Actions — after a restart, only a new explicit
    /// commit reserves again. The returned registration removes its own
    /// running entry on drop, exactly like [`Self::register`].
    pub fn take_reservation(
        &self,
        delegation: ene_task::DelegationId,
        task: ene_task::TaskId,
    ) -> TakeReservation<'_> {
        // Lock order is running first, then reservations — the same order
        // `reserve` uses — and no `.await` runs under either.
        let mut running = crate::lock_unpoison(&self.running);
        if running.contains_key(&delegation) {
            return TakeReservation::AlreadyRunning;
        }
        let mut reservations = crate::lock_unpoison(&self.reservations);
        match reservations.remove(&delegation) {
            Some(reserved_task) if reserved_task == task => {}
            Some(reserved_task) => {
                // A reservation pairing a different Task with this
                // delegation identity is corrupted: put it back and refuse.
                reservations.insert(delegation, reserved_task);
                return TakeReservation::Unreserved;
            }
            None => return TakeReservation::Unreserved,
        }
        let cancellation = DispatchAbort::default();
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
        })
    }

    /// Removes the entry held by one registration.
    ///
    /// Registration never overwrites an occupied slot, so the entry is
    /// always the dropping registration's own and no identity check is
    /// needed to keep it from removing a newer execution's token.
    fn remove(&self, delegation: ene_task::DelegationId) {
        crate::lock_unpoison(&self.running).remove(&delegation);
    }
}

/// RAII registration of one running execution; see [`TaskExecutionRegistry`].
pub struct TaskExecutionRegistration<'a> {
    registry: &'a TaskExecutionRegistry,
    delegation: ene_task::DelegationId,
    /// The cooperative stop token for the execution.
    pub cancellation: DispatchAbort,
}

impl Drop for TaskExecutionRegistration<'_> {
    fn drop(&mut self) {
        self.registry.remove(self.delegation);
    }
}

/// One parsed provider response under the fixed Task Agent protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentDirective {
    /// Run one workspace Action and replay its observation.
    Act(TaskAgentActionRequest),
    /// Submit the final answer.
    Finish { body: String },
}

/// One parsed Action request (workspace-relative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentActionRequest {
    pub operation: OperationKind,
    pub path: String,
    /// Intended UTF-8 content for create/edit; never present for list/read.
    pub content: Option<String>,
}

/// Why a provider response did not follow the fixed protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAgentProtocolViolation {
    /// The response is not a JSON object.
    NotAJsonObject,
    /// No `tool` or `final` member is present.
    MissingDirective,
    /// Both `tool` and `final`, or an unknown member set.
    ConflictingDirective,
    /// The named tool is not part of the closed world.
    UnknownTool,
    /// `path` / `content` are missing, mis-typed, or inconsistent with the tool.
    InvalidFields,
}

/// Why the execution continued no further before a final answer.
///
/// Every variant mirrors a refusal already decided by an owner boundary; this
/// layer adds no new decision and no new state.
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
    /// The execution lifetime already started durable work (at least one
    /// inference claim or Action attempt) and was left unsealed: the same
    /// delegation is never started again, whether the local running
    /// registration was lost or the process restarted. Continued work is a
    /// new delegation.
    ExecutionAlreadyStarted {
        delegation: ene_task::DelegationId,
    },
    /// Another execution of the same delegation is already running in this
    /// process. The atomic registration refusal happens before any provider
    /// call or Action, so two loops can never run one execution lifetime.
    ExecutionAlreadyRunning {
        delegation: ene_task::DelegationId,
    },
    /// No launch reservation covers the delegation in this process, so the
    /// runner refuses before any provider call or Action. After a restart
    /// only a new explicit AU3/AU17 commit reserves again: the runner never
    /// restores a launch target from the delegation rows.
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
    /// The credential set kept advancing past every re-scrub of the final
    /// answer, so no result body was recorded. Fail closed: the original
    /// answer is never stored raw and a scrubbed body prepared under an old
    /// revision is never committed.
    StaleCredentialSet {
        /// The most recently observed durable revision.
        current: CredentialSetRevision,
    },
}

/// The domain result of one delegated execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentRunOutcome {
    /// A final answer was recorded and sealed (AU15a); the adoption decision
    /// is the Task owner's unchanged result (AU15b).
    Finalized {
        result: TaskResultRecord,
        acceptance: ene_task::TaskResultAcceptance,
    },
    /// An owner boundary refused the next turn or Action.
    Refused(TaskAgentRunRefusal),
    /// The inference use was refused before any provider I/O. The attempt
    /// was never claimed and no usage fact was recorded.
    NotSent(TaskAgentNotSent),
    /// The provider call already happened, but the adoption-consent premise
    /// that admitted the send moved while it was answering: the output is
    /// discarded before any Action or result record. This is NOT
    /// [`TaskAgentNotSent`] — the send was started, so the started attempt
    /// and its usage fact stay durable — and it is not task completion.
    ConsentStaleAfterSend {
        /// Turn (1-based) whose output was discarded.
        turn: u32,
    },
    /// The provider response did not follow the protocol; nothing ran and no
    /// result was sealed.
    ProtocolViolation {
        turn: u32,
        reason: TaskAgentProtocolViolation,
    },
    /// The bounded loop reached its turn limit; nothing was sealed.
    TurnLimitReached { turns: u32 },
    /// The Action started, but the executor could not confirm its effect. The
    /// loop stopped without re-executing it and without sealing a result.
    EffectUnresolved {
        attempt: ene_action::ActionAttemptId,
    },
    /// The cooperative stop signal fired before a new provider call or Action
    /// start, or won the claimed dispatch's provider wait through the
    /// inference boundary (the provider future may have been in flight or not
    /// yet polled) after its usage accounting completed. The durable cancel
    /// admission is the Task owner's separate fact; this variant claims
    /// nothing about provider or external-effect completion.
    Cancelled,
}

/// Technical failure of one delegated execution; never a domain outcome.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentRunError {
    #[error(transparent)]
    Turn(#[from] ene_task::TaskAgentTurnError),
    #[error(transparent)]
    Action(#[from] WorkspaceActionHostError),
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    /// The final answer could not be proven free of registered credential
    /// values, so no result body was recorded. Fail closed: an unprovable
    /// scrub premise is never stored as if it were scrubbed, and the
    /// carried reason is a fixed class that never quotes the body.
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

/// Runs one delegated Task Agent execution under the fixed protocol.
///
/// `inference` is the Task-owned inference port; the Host composition root
/// wires it to the concrete inference executor with
/// [`TaskAgentInferenceAdapter::new`](crate::task_agent::TaskAgentInferenceAdapter::new)
/// and implements `instructions`/`scrubber` against History and credentials.
/// The loop starts from the execution's relied revision and continues until a
/// final answer is recorded, an owner boundary refuses, the response is
/// malformed, the effect is unresolved, the consent premise for the produced
/// output lapsed, the cooperative stop signals, or the turn bound is reached.
/// See the module docs for the exact boundary order.
///
/// A provider output whose `adoption_consent_current` is `false` is discarded
/// as [`TaskAgentRunOutcome::ConsentStaleAfterSend`] before any Action or
/// result record: the consent premise that admitted the send no longer holds,
/// while the already-started attempt and its usage fact stay durable. An
/// output refused before the send at all stays [`TaskAgentRunOutcome::NotSent`].
///
/// A final answer is scrubbed through the injected credential boundary and
/// arrives with that scrub premise; the durable commit compares the premise
/// inside its transaction. If the credential set advanced since the scrub,
/// the commit refuses without writing and the answer is re-scrubbed under the
/// observed revision (bounded by `FINAL_RESULT_SCRUB_ATTEMPTS`); an answer
/// whose premise keeps going stale is never committed and ends as
/// [`TaskAgentRunRefusal::StaleCredentialSet`].
///
/// The execution's in-process identity and cooperative stop token come from
/// `registration`: holding a [`TaskExecutionRegistration`] is what makes the
/// per-delegation refusal real, because [`TaskExecutionRegistry::register`]
/// never admits a second registration for the same delegation while this one
/// is held. The token is checked before every provider call and Action start,
/// and it is handed to the inference port (via the Host adapter) so an
/// in-flight provider call is aborted best-effort with its usage fact recorded
/// before the turn answers [`TaskAgentTurnOutcome::Aborted`]. The signal is
/// never authority: the durable cancel admission (AU16) is the Task owner's
/// commit, and stopping locally proves nothing about provider or external
/// effects. A final answer that was already produced is still recorded and
/// adoption still runs, so a cancel race resolves through the ordinary
/// `RecordedToOriginalOnly` path.
///
/// One delegated execution is run once. A stopped execution (no final result)
/// is never continued by calling this function again over the same
/// delegation, and the entry enforces that on the durable attempt facts even
/// when no in-process registration is held (restart): continued work is a
/// new delegation under the design's execution-lifetime contract.
pub async fn run_task_agent_execution(
    store: &Store,
    instructions: &impl TaskInstructionSource,
    inference: &impl TaskAgentInference,
    scrubber: &impl SecretScrubber,
    max_turns: u32,
    registration: &TaskExecutionRegistration<'_>,
) -> Result<TaskAgentRunOutcome, TaskAgentRunError> {
    let delegation = registration.delegation;
    let cancellation = &registration.cancellation;
    if execution_already_started(store, delegation).await? {
        return Ok(TaskAgentRunOutcome::Refused(
            TaskAgentRunRefusal::ExecutionAlreadyStarted { delegation },
        ));
    }
    let mut exchanges: Vec<TaskAgentActionExchange> = Vec::new();
    let mut attempt_refs: Vec<RawId> = Vec::new();
    let mut turn = 0_u32;
    loop {
        if cancellation.is_aborted() {
            return Ok(TaskAgentRunOutcome::Cancelled);
        }
        if turn >= max_turns {
            return Ok(TaskAgentRunOutcome::TurnLimitReached { turns: turn });
        }
        turn += 1;
        // The turn is always awaited to its own answer: the abort reaches the
        // provider wait through the inference port, which owns the post-claim
        // usage accounting. Dropping the turn future here would skip that
        // accounting and lose the claimed attempt's usage fact.
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
                return Ok(TaskAgentRunOutcome::Cancelled);
            }
        };
        // The consent premise travels with the provider output. If the stored
        // consent moved during the network wait, the output must not drive an
        // Action or become a final result (K-E step 5): only the generated
        // result is discarded, while the already-started attempt and usage
        // fact stay durable and untouched. This is a sent-but-discarded
        // answer, never the pre-send `NotSent` refusal.
        if !produced.adoption_consent_current {
            return Ok(TaskAgentRunOutcome::ConsentStaleAfterSend { turn });
        }
        match parse_directive(produced.output.text()) {
            Err(reason) => return Ok(TaskAgentRunOutcome::ProtocolViolation { turn, reason }),
            Ok(TaskAgentDirective::Finish { body }) => {
                // The same credential boundary that admits every logical input
                // covers the durable result body: a final answer that cannot
                // be proven scrubbed is never recorded raw, and the durable
                // arrival commit compares the scrub premise inside its own
                // transaction. A stale refusal re-scrubs the original answer
                // under the revision just observed; the stale text itself is
                // never retried as it is.
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
                if cancellation.is_aborted() {
                    return Ok(TaskAgentRunOutcome::Cancelled);
                }
                let action = run_workspace_action(
                    store,
                    delegation,
                    request.operation,
                    request.path.clone(),
                    request.content.clone().map(String::into_bytes),
                )
                .await?;
                match action {
                    WorkspaceActionHostOutcome::Completed {
                        attempt,
                        effect,
                        fact_recorded,
                    } => {
                        attempt_refs.push(attempt.as_raw());
                        // An unresolved effect stops the loop and is never
                        // replayed: the unknown stays the Action owner's fact.
                        if unconfirmed_effect(&effect) {
                            return Ok(TaskAgentRunOutcome::EffectUnresolved { attempt });
                        }
                        // The observation is durable before it is replayed:
                        // the occurrence identity and its correlation must be
                        // recorded, or the turn that would consume it never
                        // starts. A body-observed occurrence (read bytes /
                        // list listing) carries the transient body into the
                        // receiving-boundary check inside the same write.
                        let text = completed_observation(&effect, fact_recorded);
                        let observed = observed_workspace_body(&effect).then(|| text.clone());
                        let occurrence = record_task_observation(
                            store,
                            delegation,
                            Some(attempt.as_raw()),
                            observed,
                        )
                        .await?;
                        // The occurrence is body-free. If this execution was
                        // associated with a deletion interval — including one
                        // that closed after the Action read and before the
                        // occurrence write — the in-memory body is old-origin
                        // and must not enter the next provider turn. The hold
                        // names the execution, not the text, so a later fresh
                        // Owner origin of the same string is unaffected.
                        let text =
                            replay_observation_text(store, delegation, &effect, text).await?;
                        exchanges.push(TaskAgentActionExchange {
                            request: produced.output,
                            observation: TaskAgentObservation::new(occurrence, text),
                        });
                    }
                    WorkspaceActionHostOutcome::NotStarted(reason) => {
                        let text = not_started_observation(&reason);
                        // A refusal observation carries no source body and no
                        // producing attempt, but it still becomes prompt
                        // context: its occurrence is recorded body-free with
                        // the execution correlation only.
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

/// The bounded number of scrubs of one final answer before the execution
/// refuses the result.
///
/// Each stale refusal means the credential set advanced between the scrub and
/// the durable arrival commit. The original answer is still in memory, so the
/// execution re-scrubs it under the revision the commit just observed and
/// arrives again; the bound keeps a continuously moving set from spinning
/// forever, and the refusal that ends the bounded loop is a domain outcome
/// with no result row.
const FINAL_RESULT_SCRUB_ATTEMPTS: u32 = 3;

/// Scrubs one final answer and arrives with the credential premise until the
/// durable commit accepts it, or gives up with the last observed revision.
///
/// Only a credential-owned scrub proof is ever submitted: the raw answer is
/// never passed to the repository, and a scrubbed body whose premise went
/// stale is dropped uncommitted. Exhaustion returns
/// [`TaskResultArrivalOutcome::StaleCredentialSet`] with zero writes.
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

/// Applies the execution-lifetime one-shot gate.
///
/// The first durable attempt (an AU14 inference claim or an AU5 Action start)
/// of a delegation is its start marker: once committed, the execution
/// lifetime has begun and a fresh run is refused even when the in-process
/// running registration was lost or the process restarted without a final
/// result. Terminal, sealed, and revision-moved delegations keep their own
/// owner outcomes from the first turn (which also perform no provider I/O),
/// so the durable probe is only consulted when the execution would otherwise
/// be allowed to start work.
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

/// Whether one observed effect could not be confirmed.
///
/// Such an effect stops the loop: the unknown stays the Action owner's durable
/// fact, the same effect is never re-executed automatically, and the
/// execution stays unsealed for the user to judge. Confirmed success and
/// confirmed failure both continue with the fixed-class observation.
fn unconfirmed_effect(effect: &ObservedEffect) -> bool {
    effect.certainty == ActionCertainty::Unknown
}

/// Records one execution-local observation occurrence before it is replayed.
///
/// The occurrence identity is minted here, at observation time; the store
/// copies and verifies the delegation/execution correlation and the producing
/// attempt, and performs the receiving-boundary erasure check on the transient
/// body. A failed record aborts the execution as a technical error: the turn
/// that would consume an unrecorded observation never starts.
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

/// Whether one observed effect reproduced workspace body content.
///
/// Only `read` bytes and a `list` listing reproduce source content into the
/// observation text; a write confirmation or a refusal does not. The
/// distinction decides whether the deletion survey must read the workspace
/// source (a body-observed occurrence whose source cannot be read fails
/// closed).
fn observed_workspace_body(effect: &ObservedEffect) -> bool {
    matches!(
        effect.output,
        Some(ActionOutput::Bytes(_) | ActionOutput::Listing(_))
    )
}

/// Observation text replayed into the next provider turn.
///
/// A body-observing execution associated with a deletion interval is
/// old-origin even when the current condition has already closed: the
/// transient body must not become logical input. Fail closed if the hold
/// cannot be read.
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

/// Renders the executor's own observation for the next turn.
///
/// The text comes from the observed effect only (never an agent self-report)
/// and is fixed-class: a refusal of an unverified write never reads as
/// success, and a recording failure is stated explicitly so the model cannot
/// mistake the durable state for confirmed.
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

/// Renders one tool-level refusal for the next turn.
///
/// The classes are fixed and never carry the requested path, file content, or
/// a secret.
fn not_started_observation(reason: &ActionNotStarted) -> String {
    match reason {
        ActionNotStarted::Rejected(rejection) => format!("refused: {rejection}"),
        ActionNotStarted::MissingContent => String::from("refused: content is required"),
        ActionNotStarted::ContentNotAllowed => String::from("refused: content is not allowed"),
        ActionNotStarted::Denied(code) => format!("denied: {code:?}"),
        ActionNotStarted::NeedsRevalidation => String::from("refused: revalidation needed"),
        ActionNotStarted::EvaluationConsumed => {
            String::from("refused: the authorization evaluation was already used")
        }
        ActionNotStarted::StalePremise => String::from("refused: the task premise moved"),
        ActionNotStarted::TaskTerminal => String::from("refused: the task is terminal"),
        ActionNotStarted::ExecutionSealed => String::from("refused: the execution is sealed"),
        ActionNotStarted::DataUseHeld => String::from("refused: the target is under deletion"),
    }
}

/// Parses one provider response under the fixed protocol.
///
/// The parser is strict: exactly one directive member, exactly the fields the
/// directive allows, and the closed-world tool names. Anything else is a
/// violation, never a guessed Action and never a final answer.
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

/// Starts the existing Task Agent runner in the background for one committed
/// delegation.
///
/// The launcher only starts the existing runner; it owns no Task lifecycle, no
/// durable running state, and no completion decision. The runner's own
/// per-delegation registration and durable one-shot attempt facts make a
/// second launch a domain refusal, and a technical runner failure stays a
/// technical outcome (never `TaskProgress::Failed`).
pub trait TaskAgentLauncher: Send + Sync {
    /// Starts [`HostHandle::run_task_agent`] for `delegation` in the
    /// background.
    fn launch(&self, delegation: ene_task::DelegationId);
}

/// Launcher over the serving process's shared handle and provider transport.
///
/// This is the production composition seam: `conn::run` owns both Arcs and
/// installs one launcher on the handle, so a conversation-accepted Task Agent
/// execution starts without any test-side runner call. The handle is held
/// weakly to avoid an idle ownership cycle. Running tasks hold strong Host
/// references, so the serving owner's drop guard must call `abort`
/// rather than relying on the last launcher Arc disappearing.
pub struct BackgroundTaskAgent<T> {
    handle: std::sync::Weak<HostHandle>,
    transport: Arc<T>,
    tasks: std::sync::Mutex<Option<tokio::task::JoinSet<()>>>,
    shutdown: tokio::sync::Mutex<()>,
    failure: std::sync::Mutex<Option<CoreError>>,
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
        }
    }

    /// Emergency close-and-abort for the serving owner's drop guard. The
    /// guard must call this even while other launcher Arcs remain: a running
    /// task holds the Host, which itself owns the installed launcher.
    /// Started blocking work may survive this call; only a graceful join
    /// establishes quiescence.
    pub(crate) fn abort(&self) {
        let tasks = crate::lock_unpoison(&self.tasks).take();
        drop(tasks);
    }

    /// Closes launch admission and joins every admitted runner, including its
    /// awaited Store work. The serving owner calls this after draining the
    /// handlers that can commit delegations; Client disconnect never calls it.
    /// No Task cancellation or external-effect outcome is inferred here.
    ///
    /// Dropping the join future or the launcher drops its `JoinSet`, aborting
    /// async runners as an emergency stop only: started blocking work is not
    /// thereby proven quiescent. Concurrent callers wait for the same drain.
    /// Do not call this after the emergency `abort` path as evidence of a
    /// graceful drain.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serving`] for a panicked or cancelled runner,
    /// after joining all remaining runners. Runner domain and technical
    /// outcomes keep their existing semantics and are not join failures.
    pub(crate) async fn shutdown_and_join(&self) -> Result<(), CoreError> {
        let _shutdown = self.shutdown.lock().await;
        let tasks = crate::lock_unpoison(&self.tasks).take();
        let Some(mut tasks) = tasks else {
            return Ok(());
        };
        let mut failure = crate::lock_unpoison(&self.failure).take();
        while let Some(result) = tasks.join_next().await {
            if result.is_err() {
                failure.get_or_insert_with(task_agent_join_failure);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

fn task_agent_join_failure() -> CoreError {
    // A panic payload may include task or provider content.
    CoreError::Serving(String::from("Task Agent runner panicked or was cancelled"))
}

impl<T> BackgroundTaskAgent<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    /// Starts a runner under the serving owner's join set.
    ///
    /// # Errors
    ///
    /// Returns [`TaskAgentRunRefusal::ExecutionUnavailable`] when serving has
    /// closed admission or the Host is gone. A refused launch releases its
    /// reservation without changing the durable delegation or retrying it.
    fn try_launch(&self, delegation: ene_task::DelegationId) -> Result<(), TaskAgentRunRefusal> {
        let mut tasks = crate::lock_unpoison(&self.tasks);
        let Some(handle) = self.handle.upgrade() else {
            return Err(TaskAgentRunRefusal::ExecutionUnavailable { delegation });
        };
        let Some(tasks) = tasks.as_mut() else {
            handle.task_executions.release(delegation);
            return Err(TaskAgentRunRefusal::ExecutionUnavailable { delegation });
        };
        // Reap completed runners so a long-lived Host does not retain one
        // task allocation per delegation until shutdown.
        while let Some(result) = tasks.try_join_next() {
            if result.is_err() {
                crate::lock_unpoison(&self.failure).get_or_insert_with(task_agent_join_failure);
            }
        }
        let transport = Arc::clone(&self.transport);
        tasks.spawn(async move {
            // The runner owns every admission and outcome; a technical
            // failure is dropped as technical and never becomes a Task
            // failure. The dialogue never awaits execution; serving shutdown
            // retains responsibility for joining it.
            drop(handle.run_task_agent(transport.as_ref(), delegation).await);
        });
        Ok(())
    }
}

impl<T> TaskAgentLauncher for BackgroundTaskAgent<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    fn launch(&self, delegation: ene_task::DelegationId) {
        // This inlet has no response channel; try_launch releases a refused
        // reservation, leaving the committed delegation unexecuted.
        drop(self.try_launch(delegation));
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod launcher_tests {
    use super::*;

    struct NoProvider;

    impl ProviderTransport for NoProvider {
        fn complete(
            &self,
            _request: ene_inference::ProviderRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            ene_inference::ProviderResponse,
                            ene_inference::InferenceTechnicalError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(std::future::pending())
        }
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
        };
        let (held, released) = tokio::sync::oneshot::channel::<()>();
        crate::lock_unpoison(&launcher.tasks)
            .as_mut()
            .unwrap()
            .spawn(async move {
                std::future::pending::<()>().await;
                drop(held);
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
            });
        assert!(weak.upgrade().is_some());
        launcher.abort();
        assert!(released.await.is_err());
        assert!(weak.upgrade().is_none());
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
            tasks.spawn(async move { wait.await.unwrap() });
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
