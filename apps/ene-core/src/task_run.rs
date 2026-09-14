//! Autonomous Task Agent ↔ Action execution loop (Stage 4 slice D).
//!
//! One [`DelegationId`] is one delegated Task Agent execution lifetime
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
//!    ([`TaskRepository::adopt_result`], AU15b).
//! 3. Action observations are replayed into the next turn as execution-local
//!    transcript. They are never persisted: the only durable result body is
//!    the `task_result` row.
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

use ene_action::{ActionCertainty, ActionNotStarted, ObservedEffect, OperationKind};
use ene_credential::SecretScrubber;
use ene_inference::DispatchAbort;
use ene_primitive::RawId;
use ene_store::Store;
use ene_task::{
    TaskAgentActionExchange, TaskAgentInference, TaskAgentNotSent, TaskAgentObservation,
    TaskAgentOutput, TaskAgentTurnOutcome, TaskAgentTurnPremise, TaskInstructionSource,
    TaskProgress, TaskRef, TaskRepository as _, TaskResultRecord, orchestrate_result_arrival,
    orchestrate_task_agent_turn,
};

use crate::action::{WorkspaceActionHostError, WorkspaceActionHostOutcome, run_workspace_action};

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
#[derive(Default)]
pub struct TaskExecutionRegistry {
    running: std::sync::Mutex<std::collections::HashMap<ene_task::DelegationId, RunningExecution>>,
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

    fn remove(&self, delegation: ene_task::DelegationId, token: &DispatchAbort) {
        let mut running = crate::lock_unpoison(&self.running);
        if running
            .get(&delegation)
            .is_some_and(|stored| stored.cancellation.same_signal(token))
        {
            running.remove(&delegation);
        }
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
        self.registry.remove(self.delegation, &self.cancellation);
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
    /// start, or aborted an in-flight provider wait through the inference
    /// boundary after its usage accounting completed. The durable cancel
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
/// `cancellation` is the local cooperative stop signal for this execution. It
/// is checked before every provider call and Action start, and it is handed to
/// the inference port (via the Host adapter) so an in-flight provider call is
/// aborted best-effort with its usage fact recorded before the turn answers
/// [`TaskAgentTurnOutcome::Aborted`]. The signal is never authority: the
/// durable cancel admission (AU16) is the Task owner's commit, and stopping
/// locally proves nothing about provider or external effects. A final answer
/// that was already produced is still recorded and adoption still runs, so a
/// cancel race resolves through the ordinary `RecordedToOriginalOnly` path.
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
    delegation: ene_task::DelegationId,
    max_turns: u32,
    cancellation: &DispatchAbort,
) -> Result<TaskAgentRunOutcome, TaskAgentRunError> {
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
                let result =
                    orchestrate_result_arrival(store, delegation, TaskAgentOutput::new(body))
                        .await?;
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
                        match completed_follow_up(produced.output, attempt, &effect, fact_recorded)
                        {
                            CompletedFollowUp::Continue(exchange) => exchanges.push(exchange),
                            CompletedFollowUp::Stop(outcome) => return Ok(outcome),
                        }
                    }
                    WorkspaceActionHostOutcome::NotStarted(reason) => {
                        let text = not_started_observation(&reason);
                        exchanges.push(TaskAgentActionExchange {
                            request: produced.output,
                            observation: TaskAgentObservation::new(text),
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

/// The follow-up of one completed Action.
#[derive(Debug)]
enum CompletedFollowUp {
    /// The effect resolved; replay the observation.
    Continue(TaskAgentActionExchange),
    /// The effect could not be confirmed; stop without replaying anything.
    Stop(TaskAgentRunOutcome),
}

/// Decides whether one completed Action lets the loop continue.
///
/// An unverified effect stops the loop: the unknown stays as the Action
/// owner's durable fact, the same effect is never re-executed automatically,
/// and the execution stays unsealed for the user to judge. Confirmed success
/// and confirmed failure both continue with the fixed-class observation.
fn completed_follow_up(
    request: TaskAgentOutput,
    attempt: ene_action::ActionAttemptId,
    effect: &ObservedEffect,
    fact_recorded: bool,
) -> CompletedFollowUp {
    if effect.certainty == ActionCertainty::Unknown {
        return CompletedFollowUp::Stop(TaskAgentRunOutcome::EffectUnresolved { attempt });
    }
    CompletedFollowUp::Continue(TaskAgentActionExchange {
        request,
        observation: TaskAgentObservation::new(completed_observation(effect, fact_recorded)),
    })
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

#[cfg(test)]
mod tests;
