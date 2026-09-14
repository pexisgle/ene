//! Task Agent inference turn harness (H-A / K-E).
//!
//! One turn runs exactly one delegated inference through the Task-owned
//! [`TaskAgentInference`] port. The harness assembles the logical input
//! itself: a fixed response-format preamble, the relied revision's
//! adopted-purpose text from the `task_revision` snapshot, each adopted
//! instruction body resolved through the Task-owned [`TaskInstructionSource`]
//! port from its canonical History source, and the execution-local Action
//! transcript (the completed tool calls and their observations) the caller
//! passes in the premise. Everything is one string with one fixed framing,
//! scrubbed exactly once, and only the injected [`SecretScrubber`]'s output
//! reaches the port. This module never constructs a [`ScrubbedText`] literal,
//! never copies an instruction body into Task state, never persists the
//! Action transcript, and never sends workspace or file content on its own
//! (the exchange text is composed by the Host from what the Action owner
//! observed).
//!
//! The ports keep `ene-task` free of permission, credential, and
//! conversation-history concrete types: the Host composition root
//! (`apps/ene-core`) implements them against `ene-inference` and
//! `HistoryRepository`. The delegation row is a durable correspondence, not
//! a liveness claim, and provider output is a fact about the call, not task
//! completion, adoption, or external-effect success.

use ene_credential::{ScrubbedText, SecretScrubber};
use ene_primitive::RawId;

use crate::context::{TaskContextEntryId, TaskContextItem, TaskContextOriginKind};
use crate::delegation::{DelegationId, DelegationRef};
use crate::instruction::{TaskInstructionRole, TaskInstructionSource};
use crate::repository::{TaskRepository, TaskTechnicalError};
use crate::task::{TaskId, TaskProgress, TaskRecord, TaskRef};

/// One turn request from the caller; carries no liveness claim.
///
/// The delegation identity names a correspondence row, and the row's
/// existence never proves the ephemeral agent is alive or that delegated work
/// is running; the turn re-checks the task premise instead of assuming it.
/// `exchanges` is the execution-local transcript of Action requests this
/// delegation already performed (in order); it is never persisted and is
/// replayed into the provider-visible logical input for this turn only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentTurnPremise {
    /// The delegation correspondence to run.
    pub delegation: DelegationId,
    /// Prior Action exchanges of this execution, oldest first.
    pub exchanges: Vec<TaskAgentActionExchange>,
}

/// What the port receives.
///
/// `prompt` is only ever the injected scrubber's output; the harness never
/// assembles a `ScrubbedText` itself. `data_use` is the canonical source
/// correlation of the logical input in input order (opaque [`RawId`]s, never
/// bodies or hashes): the in-force adopted-purpose entry's `origin.source`
/// today, extended to every adopted-instruction entry with the slice that
/// sends instruction bodies. [`core::fmt::Debug`] redacts the prompt so
/// diagnostic output cannot leak its text.
#[derive(Clone)]
pub struct TaskAgentInferencePremise {
    /// The delegation correspondence this turn runs under.
    pub delegation: DelegationId,
    /// The relied-on Task revision.
    pub task: TaskRef,
    /// Scrubbed logical input for this turn.
    pub prompt: ScrubbedText,
    /// Canonical source correlation of `prompt`, in logical-input order.
    /// Duplicates are preserved: each adopted entry keeps its own correlation.
    pub data_use: Vec<RawId>,
}

impl core::fmt::Debug for TaskAgentInferencePremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskAgentInferencePremise")
            .field("delegation", &self.delegation)
            .field("task", &self.task)
            .field("prompt", &"[redacted]")
            .field("data_use", &self.data_use)
            .finish()
    }
}

/// Provider output text; [`core::fmt::Debug`] redacts it.
///
/// The Host adapter builds this from the provider response; Task-internal
/// code reads it through [`Self::text`]. Holding the output is not a claim
/// that anything was adopted or that the Task advanced.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskAgentOutput(String);

impl TaskAgentOutput {
    /// Builds the output from the provider response text.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// The provider output text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for TaskAgentOutput {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("TaskAgentOutput")
            .field(&"[redacted]")
            .finish()
    }
}

/// The execution-local result of one Action request, as the Task Agent
/// execution observed it.
///
/// The text is composed by the Host from the Action owner's observed effect
/// (or from the refusal class) and is never persisted: the only durable
/// result body is the final `task_result` row. [`core::fmt::Debug`] redacts
/// the text because it can carry file content.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskAgentObservation(String);

impl TaskAgentObservation {
    /// Builds the observation text the next turn replays.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// The observation text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for TaskAgentObservation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("TaskAgentObservation")
            .field(&"[redacted]")
            .finish()
    }
}

/// One completed Action exchange of a delegated execution.
///
/// `request` is the provider output that asked for the Action and
/// `observation` is what the execution observed (or the refusal class). The
/// pair is execution-local transcript, never a canonical source and never
/// persisted; it exists so the next turn sees what already happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentActionExchange {
    pub request: TaskAgentOutput,
    pub observation: TaskAgentObservation,
}

/// Mirror of the inference-side `NotSentReason`; same meanings, Task-owned.
///
/// These describe a use that was refused before any provider I/O; they are
/// not task domain outcomes and carry no output text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskAgentNotSent {
    /// No complete setup premise: no consent is recorded, the consent
    /// references an unregistered credential, or the bearer is missing.
    SetupIncomplete,
    /// The live-authorization allowlist refused the use.
    NotInAllowlist,
    /// The stored consent moved away from the premise the use was admitted
    /// under before the attempt claim. A consent that moves during the
    /// provider wait is not this refusal: the output is then produced with
    /// `adoption_consent_current = false` and classified by the caller as a
    /// sent-but-discarded output, never as this pre-send class.
    ConsentStale,
    /// The input exceeds the inference input bound.
    OverLimit,
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different fingerprint.
    EvaluationConsumed,
    /// At least one canonical source of the logical input is covered by a
    /// current erasure condition. The send is refused before any provider
    /// I/O; this is a data-use hold, not revision/consent staleness, a
    /// missing source, or a technical error.
    DataUseHeld,
}

/// Port result; provider output never leaks through [`core::fmt::Debug`]
/// because [`TaskAgentOutput`] redacts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentInferenceOutcome {
    /// The provider returned output. `adoption_consent_current` records
    /// whether the consent still held after the network wait; the output is
    /// not task completion and is not adopted here.
    Produced {
        output: TaskAgentOutput,
        adoption_consent_current: bool,
    },
    /// The relied delegation/task premise no longer held before sending; the
    /// provider was not called.
    StaleTaskPremise,
    /// The use was refused before sending for the given reason.
    NotSent(TaskAgentNotSent),
}

/// Technical failure of the inference port; never prompt or output text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentInferenceError {
    /// Provider-class cause. Never prompt, output, or secret text.
    #[error("task agent inference unavailable: {reason}")]
    InferenceUnavailable { reason: String },
}

/// The Task-owned inference port for one delegated turn.
///
/// The Host composition root (`apps/ene-core`) implements this against
/// `ene-inference`, so `ene-task` never names a permission or credential
/// concrete type beyond the scrub boundary. The implementation owns the
/// attempt claim and the provider call; [`orchestrate_task_agent_turn`] owns
/// prompt assembly and the outcome mapping.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait TaskAgentInference: Send + Sync {
    /// The maximum logical-input length this port admits, in Unicode scalar
    /// values.
    ///
    /// [`orchestrate_task_agent_turn`] keeps its assembled logical input
    /// within this bound by dropping the oldest execution-local Action
    /// exchanges when the transcript would otherwise outgrow the port; the
    /// port (dispatch) still enforces its absolute cap, so an input whose
    /// newest exchange alone exceeds the bound is refused as
    /// [`TaskAgentNotSent::OverLimit`] instead of being silently truncated.
    fn input_budget(&self) -> usize;

    /// Runs one claimed inference turn; see the port's budget contract.
    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>;
}

/// Turn-level technical failure, distinct from domain outcomes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentTurnError {
    /// The repository could not be read; no prompt was assembled.
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    /// The port failed technically; never prompt or output text.
    #[error("task agent inference unavailable: {reason}")]
    InferenceUnavailable { reason: String },
    /// The logical input could not be proven scrubbed; nothing was sent.
    #[error("task agent input unavailable: {reason}")]
    InputUnavailable { reason: String },
}

/// One produced inference result with its Task correlation.
///
/// The output is a fact about the provider call: it is not task completion,
/// not adoption, and not external-effect success. The Task revision stays
/// the relied one; a later steering may have moved the Task forward while
/// the call was in flight, which is why the consent flag is reported
/// alongside rather than acted on here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentInferenceProduced {
    /// The delegation correspondence the turn ran under.
    pub delegation: DelegationId,
    /// The relied-on Task revision.
    pub task: TaskRef,
    /// Provider output; redacted from [`core::fmt::Debug`].
    pub output: TaskAgentOutput,
    /// Whether the adoption consent still held after the network wait.
    pub adoption_consent_current: bool,
}

/// Domain outcome of one turn. Provider output is NOT task completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentTurnOutcome {
    /// The provider returned output; adoption and completion remain the
    /// owners' decisions.
    Produced(TaskAgentInferenceProduced),
    /// The relied task revision moved; nothing was sent.
    StaleTaskRevision {
        /// The durable current reference read after the stale signal.
        current: TaskRef,
    },
    /// The delegated Task has no durable state.
    MissingTask {
        /// The missing Task identity.
        task: TaskId,
    },
    /// The delegation correspondence does not exist; a missing row is not
    /// evidence about any ephemeral agent.
    MissingDelegation {
        /// The missing delegation identity.
        delegation: DelegationId,
    },
    /// The Task is terminal (`Completed` / `Failed`); no inference is
    /// claimed and no provider I/O happens.
    TaskTerminal {
        /// The terminal Task identity.
        task: TaskId,
        /// The terminal progress value.
        progress: TaskProgress,
    },
    /// The delegation's execution already submitted a final result (its
    /// `task_result` row exists, sealing it); no inference is claimed and no
    /// provider I/O happens. Distinct from revision staleness: the Task may
    /// still be `InProgress`.
    ExecutionSealed {
        /// The sealed delegation.
        delegation: DelegationId,
    },
    /// An adopted instruction's canonical History source does not exist, so
    /// the logical input cannot be resolved. The turn ends with this
    /// `Ok`-side domain outcome: no body is fabricated, no entry is removed,
    /// rewritten, or retired, no scrub or inference claim happens, and the
    /// provider receives nothing. This is not a corruption and not a
    /// data-use hold: a source that was readable and then came under an
    /// erasure condition is reported as [`TaskAgentNotSent::DataUseHeld`].
    InstructionSourceMissing {
        /// The adoption identity, in context order, whose body could not be
        /// resolved.
        entry: TaskContextEntryId,
        /// The unresolved canonical source identity.
        source: RawId,
    },
    /// The use was refused before any provider I/O.
    NotSent(TaskAgentNotSent),
}

/// Orchestrates one Task Agent inference turn.
///
/// The work owner assembles the logical input and maps the port result; the
/// port's implementation owns the attempt claim and re-checks the delegation
/// and task premise inside that claim, which is the actual currentness
/// guarantee. The precheck here (delegation and held revision) is
/// informational, so a competing steering winner between the precheck and
/// the claim is still reported as stale by the port.
///
/// The logical input is a fixed response-format preamble, the relied
/// revision's adopted-purpose text, the adopted-instruction bodies in
/// `TaskRecord.context` order, and the execution-local Action exchanges the
/// caller replays (each request followed by its observation, oldest first).
/// The newest exchanges are kept within the port's
/// [`input_budget`](TaskAgentInference::input_budget): whole oldest exchanges
/// are dropped (with a fixed omission note) when the transcript would
/// outgrow the port, and an exchange that cannot fit even alone is left in
/// place so the port refuses the over-limit input instead of the model
/// answering from a silently shortened observation. The whole string is
/// scrubbed exactly once and only the scrubber's output crosses the port. Instruction bodies stay canonical in History: the
/// [`TaskInstructionSource`] port reads each adopted entry's source, the
/// source/role/companion correspondence is verified before the body is
/// used, and an absent source ends the turn as
/// [`TaskAgentTurnOutcome::InstructionSourceMissing`] without fabricating,
/// skipping, or rewriting anything. The exchange transcript is execution-local
/// evidence from the Action owner and is never persisted or treated as a
/// canonical source; it is reproduced for the provider only through this
/// turn.
///
/// The logical input's canonical source correlation (`data_use`) is the
/// purpose entry's `origin.source` followed by every adopted instruction's
/// `origin.source`, in the same order, duplicates retained. The Action
/// exchange transcript is execution-local and carries no canonical source, so
/// it adds no `data_use` entry. It travels to
/// the claim, which compares it against the canonical current
/// erasure-condition store in the same transaction as the task premise; a
/// covered source yields [`TaskAgentTurnOutcome::NotSent`] with
/// [`TaskAgentNotSent::DataUseHeld`] and no provider I/O. A scrub failure,
/// a source read failure, and a correspondence mismatch all fail closed as
/// [`TaskAgentTurnError::InputUnavailable`] with nothing sent.
///
/// The History reads and the scrub happen outside any transaction or lock
/// the claim uses: the claim alone is the linearization point.
///
/// Outcome mapping: `Produced` carries the output and consent flag without
/// adopting either, and `NotSent` reasons pass through unchanged. A
/// `StaleTaskPremise` refusal is re-read against durable state and mapped to
/// [`TaskAgentTurnOutcome::MissingDelegation`],
/// [`TaskAgentTurnOutcome::MissingTask`],
/// [`TaskAgentTurnOutcome::TaskTerminal`] (progress terminal),
/// [`TaskAgentTurnOutcome::ExecutionSealed`] (the delegation already has its
/// final result), or [`TaskAgentTurnOutcome::StaleTaskRevision`]. The precheck
/// terminal refusal and the claim's terminal/seal refusal both come from
/// durable comparisons, and neither sends a provider byte. Repository
/// technical errors map to [`TaskAgentTurnError::StorageUnavailable`] and port
/// technical errors to [`TaskAgentTurnError::InferenceUnavailable`]; domain
/// outcomes are never folded into either.
pub async fn orchestrate_task_agent_turn(
    repository: &impl TaskRepository,
    instructions: &impl TaskInstructionSource,
    inference: &impl TaskAgentInference,
    scrubber: &impl SecretScrubber,
    premise: TaskAgentTurnPremise,
) -> Result<TaskAgentTurnOutcome, TaskAgentTurnError> {
    let (delegation, record) = match load_premise(repository, premise.delegation).await? {
        PremiseLoad::Loaded(loaded) => (loaded.delegation, loaded.record),
        PremiseLoad::MissingDelegation => {
            return Ok(TaskAgentTurnOutcome::MissingDelegation {
                delegation: premise.delegation,
            });
        }
        PremiseLoad::MissingTask { task } => {
            return Ok(TaskAgentTurnOutcome::MissingTask { task });
        }
    };
    if record.task.reference != delegation.task {
        return Ok(TaskAgentTurnOutcome::StaleTaskRevision {
            current: record.task.reference,
        });
    }
    // The precheck reads the same durable Task unit the claim will compare
    // again; a terminal progress refuses before the scrub or the port call.
    // The claim remains the authoritative gate, so a race that commits after
    // this read is still refused there.
    if record.task.progress.is_terminal() {
        return Ok(TaskAgentTurnOutcome::TaskTerminal {
            task: delegation.task.task,
            progress: record.task.progress,
        });
    }
    // The precheck matched, so `record.revision` is the relied revision's
    // snapshot. The context walk carries the load-time ordering and
    // provenance invariants instead of re-deriving them: `load_task` returns
    // the in-force adopted-purpose entry first, followed by the adopted
    // instruction entries in `(revision, entry_id)` order.
    let mut purpose_text = None;
    let mut instruction_texts = Vec::new();
    let mut data_use = Vec::with_capacity(record.context.len());
    for (index, entry) in record.context.iter().enumerate() {
        match entry.item {
            TaskContextItem::AdoptedPurpose(_) if index == 0 => {
                purpose_text = Some(record.revision.purpose_text.text.as_str());
                data_use.push(entry.origin.source);
            }
            TaskContextItem::AdoptedPurpose(_) => {
                return Err(TaskAgentTurnError::InputUnavailable {
                    reason: String::from("task context purpose entry is out of order"),
                });
            }
            TaskContextItem::AdoptedInstruction => {
                // The body producer exists only for Owner conversation
                // sources; a Spontaneous / ScheduleOccurrence item stays
                // fail closed until its producer designs a body resolution,
                // and is never silently skipped.
                if entry.origin.kind != TaskContextOriginKind::OwnerConversation {
                    return Err(TaskAgentTurnError::InputUnavailable {
                        reason: String::from("unsupported instruction origin kind"),
                    });
                }
                let loaded = instructions
                    .load_owner_instruction(entry.origin.source)
                    .await
                    .map_err(|_| TaskAgentTurnError::InputUnavailable {
                        reason: String::from("instruction source read failed"),
                    })?;
                let Some(loaded) = loaded else {
                    return Ok(TaskAgentTurnOutcome::InstructionSourceMissing {
                        entry: entry.entry,
                        source: entry.origin.source,
                    });
                };
                if loaded.source != entry.origin.source
                    || loaded.role != TaskInstructionRole::Owner
                    || loaded.companion != record.task.assignee.companion
                {
                    return Err(TaskAgentTurnError::InputUnavailable {
                        reason: String::from("instruction source correspondence mismatch"),
                    });
                }
                data_use.push(entry.origin.source);
                instruction_texts.push(loaded.text);
            }
        }
    }
    let Some(purpose_text) = purpose_text else {
        return Err(TaskAgentTurnError::StorageUnavailable {
            reason: String::from("task context has no adopted purpose entry"),
        });
    };
    // The logical input is assembled in full before the single scrub: the
    // scrubber sees purpose, every resolved instruction body, and the
    // execution-local Action transcript once, and only its output may cross
    // the port. A scrub failure fails closed with no provider I/O and never
    // logs the raw input. The transcript is execution-local, so the oldest
    // exchanges are dropped (with a fixed note) when the port's input budget
    // would otherwise be outgrown; an exchange that cannot fit even alone is
    // kept so the port refuses the over-limit input instead of the model
    // answering from a silently shortened transcript.
    let (kept_exchanges, omitted) = fit_exchanges(
        purpose_text,
        &instruction_texts,
        &premise.exchanges,
        inference.input_budget(),
    );
    let raw_input =
        assemble_logical_input(purpose_text, &instruction_texts, kept_exchanges, omitted);
    let Ok(prompt) = scrubber.scrub(&raw_input).await else {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("credential scrub failed"),
        });
    };
    let outcome = inference
        .infer(TaskAgentInferencePremise {
            delegation: delegation.delegation,
            task: delegation.task,
            prompt,
            data_use,
        })
        .await
        .map_err(inference_error)?;
    Ok(match outcome {
        TaskAgentInferenceOutcome::Produced {
            output,
            adoption_consent_current,
        } => TaskAgentTurnOutcome::Produced(TaskAgentInferenceProduced {
            delegation: delegation.delegation,
            task: delegation.task,
            output,
            adoption_consent_current,
        }),
        TaskAgentInferenceOutcome::StaleTaskPremise => {
            re_read_stale_premise(repository, premise.delegation).await?
        }
        TaskAgentInferenceOutcome::NotSent(reason) => TaskAgentTurnOutcome::NotSent(reason),
    })
}

/// The fixed response-format preamble every logical input starts with.
const RESPONSE_FORMAT_PREAMBLE: &str = "[RESPONSE FORMAT]\n\
     Respond with exactly one JSON object and no other text. One of:\n\
     {\"tool\":\"list\",\"path\":\"<workspace-relative directory>\"}\n\
     {\"tool\":\"read\",\"path\":\"<workspace-relative file>\"}\n\
     {\"tool\":\"create\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
     {\"tool\":\"edit\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
     {\"final\":\"<final answer>\"}\n\
     [PURPOSE]\n";

const INSTRUCTION_MARKER: &str = "\n[INSTRUCTION]\n";
const TOOL_CALL_MARKER: &str = "\n[TOOL CALL]\n";
const TOOL_RESULT_MARKER: &str = "\n[TOOL RESULT]\n";

/// The fixed-class marker that tells the model older exchanges were dropped.
const OMISSION_NOTE: &str = "\n[NOTE] earlier tool exchanges were omitted to fit the input bound";

/// Assembles the fixed logical-input framing for one turn.
///
/// The protocol preamble comes first (it tells the model how to answer and
/// never varies), then the purpose, then each resolved instruction body in
/// `TaskRecord.context` order, then the omission note when older exchanges
/// were dropped, then the kept Action exchanges of this execution in order.
/// The boundary markers are identical for every turn (including a turn with
/// no instructions or exchanges), so the provider-visible boundaries are
/// never body text and never vary by caller. Instructions are not sorted,
/// deduplicated, or filtered: repeated adoption of the same source remains
/// repeated input.
fn assemble_logical_input(
    purpose: &str,
    instructions: &[String],
    exchanges: &[TaskAgentActionExchange],
    omitted: bool,
) -> String {
    let mut input = String::from(RESPONSE_FORMAT_PREAMBLE);
    input.push_str(purpose);
    for instruction in instructions {
        input.push_str(INSTRUCTION_MARKER);
        input.push_str(instruction);
    }
    if omitted {
        input.push_str(OMISSION_NOTE);
    }
    for exchange in exchanges {
        input.push_str(TOOL_CALL_MARKER);
        input.push_str(exchange.request.text());
        input.push_str(TOOL_RESULT_MARKER);
        input.push_str(exchange.observation.text());
    }
    input
}

/// Fits the execution-local transcript into the port's input budget.
///
/// Exchanges are kept newest-first (the model most needs what just happened)
/// and whole oldest exchanges are dropped; dropping them loses no canonical
/// source because the transcript is execution-local, and the assembled input
/// carries [`OMISSION_NOTE`] so the omission is visible to the model. When the
/// newest exchange alone cannot fit, the transcript is left unchanged and the
/// port refuses the over-limit input: the model must never answer from a
/// silently shortened observation. The omitted note's length is reserved up
/// front, so adding it cannot push the input back over the budget.
fn fit_exchanges<'a>(
    purpose: &str,
    instructions: &[String],
    exchanges: &'a [TaskAgentActionExchange],
    budget: usize,
) -> (&'a [TaskAgentActionExchange], bool) {
    let prefix = RESPONSE_FORMAT_PREAMBLE.chars().count()
        + purpose.chars().count()
        + instructions
            .iter()
            .map(|text| INSTRUCTION_MARKER.chars().count() + text.chars().count())
            .sum::<usize>();
    if prefix >= budget {
        return (exchanges, false);
    }
    let reserved = budget - prefix;
    let mut used = 0usize;
    let mut start = exchanges.len();
    for exchange in exchanges.iter().rev() {
        let length = exchange_input_len(exchange);
        if used + length > reserved {
            break;
        }
        used += length;
        start -= 1;
    }
    if start == 0 {
        return (exchanges, false);
    }
    if start == exchanges.len() {
        // Even the newest exchange alone does not fit: keep the transcript so
        // the port refuses it instead of fabricating an answer.
        return (exchanges, false);
    }
    // Older exchanges are dropped, so the omission note is included: refit
    // with its length reserved so the note cannot push the input back over.
    let Some(available) = reserved.checked_sub(OMISSION_NOTE.chars().count()) else {
        return (exchanges, false);
    };
    let mut used = 0usize;
    let mut start = exchanges.len();
    for exchange in exchanges.iter().rev() {
        let length = exchange_input_len(exchange);
        if used + length > available {
            break;
        }
        used += length;
        start -= 1;
    }
    if start == exchanges.len() {
        // The note cannot fit alongside the newest exchange; keep everything
        // and let the port refuse rather than dropping the observation.
        return (exchanges, false);
    }
    (&exchanges[start..], true)
}

/// The assembled length of one transcript exchange, in Unicode scalar values.
fn exchange_input_len(exchange: &TaskAgentActionExchange) -> usize {
    TOOL_CALL_MARKER.chars().count()
        + exchange.request.text().chars().count()
        + TOOL_RESULT_MARKER.chars().count()
        + exchange.observation.text().chars().count()
}

/// Maps one stale claim refusal to the durable reason, re-reading only
/// bounded state.
///
/// Terminal progress is reported before the execution seal: a terminal Task
/// explains the refusal regardless of whether the delegation is also sealed.
/// A sealed delegation is reported before revision staleness because the
/// execution can never contribute new work even while the Task is
/// `InProgress`.
async fn re_read_stale_premise(
    repository: &impl TaskRepository,
    delegation: DelegationId,
) -> Result<TaskAgentTurnOutcome, TaskAgentTurnError> {
    match load_premise(repository, delegation).await? {
        PremiseLoad::MissingDelegation => {
            Ok(TaskAgentTurnOutcome::MissingDelegation { delegation })
        }
        PremiseLoad::MissingTask { task } => Ok(TaskAgentTurnOutcome::MissingTask { task }),
        PremiseLoad::Loaded(loaded) => {
            if loaded.record.task.progress.is_terminal() {
                return Ok(TaskAgentTurnOutcome::TaskTerminal {
                    task: loaded.delegation.task.task,
                    progress: loaded.record.task.progress,
                });
            }
            let sealed = repository
                .load_delegation_result(delegation)
                .await
                .map_err(storage_error)?;
            if sealed.is_some() {
                return Ok(TaskAgentTurnOutcome::ExecutionSealed { delegation });
            }
            Ok(TaskAgentTurnOutcome::StaleTaskRevision {
                current: loaded.record.task.reference,
            })
        }
    }
}

fn storage_error(error: TaskTechnicalError) -> TaskAgentTurnError {
    match error {
        TaskTechnicalError::StorageUnavailable { reason } => {
            TaskAgentTurnError::StorageUnavailable { reason }
        }
    }
}

/// One durable premise load, shared by the precheck and the stale re-read.
///
/// The loaded payload is boxed so the rejection variants stay small.
enum PremiseLoad {
    Loaded(Box<LoadedPremise>),
    MissingDelegation,
    MissingTask { task: TaskId },
}

struct LoadedPremise {
    delegation: DelegationRef,
    record: TaskRecord,
}

async fn load_premise(
    repository: &impl TaskRepository,
    delegation: DelegationId,
) -> Result<PremiseLoad, TaskAgentTurnError> {
    let Some(delegation) = repository
        .load_delegation(delegation)
        .await
        .map_err(storage_error)?
    else {
        return Ok(PremiseLoad::MissingDelegation);
    };
    let Some(record) = repository
        .load_task(delegation.task.task)
        .await
        .map_err(storage_error)?
    else {
        return Ok(PremiseLoad::MissingTask {
            task: delegation.task.task,
        });
    };
    Ok(PremiseLoad::Loaded(Box::new(LoadedPremise {
        delegation,
        record,
    })))
}

fn inference_error(error: TaskAgentInferenceError) -> TaskAgentTurnError {
    match error {
        TaskAgentInferenceError::InferenceUnavailable { reason } => {
            TaskAgentTurnError::InferenceUnavailable { reason }
        }
    }
}
