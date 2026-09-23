use ene_credential::{ScrubbedText, SecretScrubber};
use ene_primitive::RawId;

use crate::context::{TaskContextEntryId, TaskContextItem, TaskContextOriginKind};
use crate::delegation::{DelegationId, DelegationRef};
use crate::instruction::{TaskInstructionRole, TaskInstructionSource};
use crate::observation::TaskAgentObservationId;
use crate::repository::{TaskRepository, TaskTechnicalError};
use crate::task::{TaskId, TaskProgress, TaskRecord, TaskRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentTurnPremise {
    pub delegation: DelegationId,
    pub exchanges: Vec<TaskAgentActionExchange>,
}

#[derive(Clone)]
pub struct TaskAgentInferencePremise {
    pub delegation: DelegationId,
    pub task: TaskRef,
    pub prompt: ScrubbedText,
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

#[derive(Clone, PartialEq, Eq)]
pub struct TaskAgentOutput(String);

impl TaskAgentOutput {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

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

#[derive(Clone, PartialEq, Eq)]
pub struct TaskAgentObservation {
    occurrence: TaskAgentObservationId,
    text: String,
}

impl TaskAgentObservation {
    #[must_use]
    pub fn new(occurrence: TaskAgentObservationId, text: String) -> Self {
        Self { occurrence, text }
    }

    #[must_use]
    pub fn occurrence(&self) -> TaskAgentObservationId {
        self.occurrence
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentActionExchange {
    pub request: TaskAgentOutput,
    pub observation: TaskAgentObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskAgentNotSent {
    SetupIncomplete,
    NotInAllowlist,
    ConsentStale,
    OverLimit,
    EvaluationConsumed,
    DataUseHeld,
    UsageCapReached,
    UsageCapIndeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentInferenceOutcome {
    Produced {
        output: TaskAgentOutput,
        adoption_consent_current: bool,
    },
    StaleTaskPremise,
    NotSent(TaskAgentNotSent),
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentInferenceError {
    #[error("task agent inference unavailable: {reason}")]
    InferenceUnavailable { reason: String },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait TaskAgentInference: Send + Sync {
    fn input_budget(&self) -> usize;

    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAgentTurnError {
    /// The repository could not be read; no prompt was assembled.
    #[error(transparent)]
    StorageUnavailable(#[from] TaskTechnicalError),
    /// The port failed technically; never prompt or output text.
    #[error(transparent)]
    InferenceUnavailable(#[from] TaskAgentInferenceError),
    /// The logical input could not be proven scrubbed; nothing was sent.
    #[error("task agent input unavailable: {reason}")]
    InputUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentInferenceProduced {
    pub delegation: DelegationId,
    pub task: TaskRef,
    pub output: TaskAgentOutput,
    pub adoption_consent_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAgentTurnOutcome {
    Produced(TaskAgentInferenceProduced),
    StaleTaskRevision {
        current: TaskRef,
    },
    MissingTask {
        task: TaskId,
    },
    MissingDelegation {
        delegation: DelegationId,
    },
    /// The Task is terminal (`Completed` / `Failed` / `Cancelled`); no inference is
    /// claimed and no provider I/O happens.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    ExecutionSealed {
        delegation: DelegationId,
    },
    InstructionSourceMissing {
        entry: TaskContextEntryId,
        source: RawId,
    },
    NotSent(TaskAgentNotSent),
    Aborted,
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
/// `TaskRecord.context` order, the every-turn past-executed facts block, and
/// the execution-local Action exchanges the caller replays (each request
/// followed by its observation, oldest first).
/// The newest exchanges are kept within the port's
/// [`input_budget`](TaskAgentInference::input_budget): whole oldest exchanges
/// are dropped (with a fixed omission note) when the transcript would
/// outgrow the port, and an exchange that cannot fit even alone is left in
/// place so the port refuses the over-limit input instead of the model
/// answering from a silently shortened observation. The never-omitted
/// logical input head — the response-format preamble, the relied purpose,
/// every adopted instruction body, and the past-executed facts block — is
/// reserved up front; when that head alone reaches the port budget no
/// transcript trimming could help, so the turn fails as
/// [`TaskAgentTurnError::InputUnavailable`]. The whole string is
/// scrubbed exactly once and only the scrubber's output crosses the port. Instruction bodies stay canonical in History or in the first-party
/// activity record: the
/// [`TaskInstructionSource`] port reads each adopted entry's origin, the
/// kind/source/role/companion correspondence is verified before the body is
/// used, and an absent source ends the turn as
/// [`TaskAgentTurnOutcome::InstructionSourceMissing`] without fabricating,
/// skipping, or rewriting anything. The exchange transcript is execution-local
/// evidence from the Action owner and is never persisted or treated as a
/// canonical source; it is reproduced for the provider only through this
/// turn.
///
/// The logical input's canonical source correlation (`data_use`) is the
/// purpose entry's `origin.source` followed by every adopted instruction's
/// `origin.source`, then every past-executed fact's source, and finally the
/// durable occurrence identity of every kept Action exchange observation, in
/// the same order, duplicates retained. The Action exchange transcript is
/// execution-local, so the observation occurrence identity — minted and made
/// durable at observation time — is what carries its provenance; the exchange
/// request text itself adds no entry. It travels to
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
/// adopting either, `NotSent` reasons pass through unchanged, and the port's
/// `Aborted` passes through unchanged (the port completed any claimed
/// attempt's usage accounting before answering it). A
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
    if record.task.reference != delegation.task {
        return Ok(TaskAgentTurnOutcome::StaleTaskRevision {
            current: record.task.reference,
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
                if !matches!(
                    entry.origin.kind,
                    TaskContextOriginKind::OwnerConversation
                        | TaskContextOriginKind::OwnerManagement
                ) {
                    return Err(TaskAgentTurnError::InputUnavailable {
                        reason: String::from("unsupported instruction origin kind"),
                    });
                }
                let loaded = instructions
                    .load_owner_instruction(entry.origin)
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
                if loaded.kind != entry.origin.kind
                    || loaded.source != entry.origin.source
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
        return Err(TaskAgentTurnError::StorageUnavailable(
            TaskTechnicalError::StorageUnavailable {
                reason: String::from("task context has no adopted purpose entry"),
            },
        ));
    };
    let past = repository
        .load_past_executed_facts(record.task.reference.task)
        .await?;
    if past.has_more {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("past executed facts exceed the turn bound"),
        });
    }
    for fact in &past.facts {
        data_use.push(fact.source);
    }
    // The logical input is assembled in full before the single scrub: the
    // scrubber sees purpose, every resolved instruction body, and the
    // execution-local Action transcript once, and only its output may cross
    // the port. A scrub failure fails closed with no provider I/O and never
    // logs the raw input. The transcript is execution-local, so the oldest
    // exchanges are dropped (with a fixed note) when the port's input budget
    // would otherwise be outgrown; an exchange that cannot fit even alone is
    // kept so the port refuses the over-limit input instead of the model
    // answering from a silently shortened transcript.
    let mut raw_input =
        assemble_logical_input(purpose_text, &instruction_texts, &past.facts, &[], false);
    // The never-omitted head is measured from the assembled framing itself,
    // so the fit and the refusal guard below cannot drift from the bytes
    // actually sent.
    let head_len = raw_input.chars().count();
    let (kept_exchanges, omitted) =
        fit_exchanges(&premise.exchanges, head_len, inference.input_budget());
    // The kept transcript's observation occurrences join the ordered
    // correlation after the canonical sources, in the same order they appear
    // in the logical input. The occurrence identity is the durable ledger
    // identity minted at observation time: it lets the claim gate and the
    // deletion admission association name what this turn consumed without any
    // body, hash, or matcher being stored. A dropped exchange is not in the
    // input, so it adds no correlation.
    for exchange in kept_exchanges {
        data_use.push(exchange.observation.occurrence().as_raw());
    }
    if omitted {
        raw_input.push_str(OMISSION_NOTE);
    }
    for exchange in kept_exchanges {
        push_exchange(&mut raw_input, exchange);
    }
    // The never-omitted logical input head (preamble, purpose, instructions,
    // adopted facts) that alone outgrows the port budget is never silently
    // shortened and never sent: no transcript trimming could help, so the turn
    // refuses before scrubbing or reaching the port. A transcript overflow
    // around a fitting head keeps its existing port-refusal behavior: the port
    // refuses the over-limit input as `OverLimit` instead of the model
    // answering from a silently shortened transcript.
    if raw_input.chars().count() > inference.input_budget() && head_len >= inference.input_budget()
    {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("never-omitted logical input exceeds the input budget"),
        });
    }
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
        .await?;
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
        TaskAgentInferenceOutcome::Aborted => TaskAgentTurnOutcome::Aborted,
    })
}

const RESPONSE_FORMAT_PREAMBLE: &str = "[RESPONSE FORMAT]\n\
     Respond with exactly one JSON object and no other text. One of:\n\
     {\"tool\":\"list\",\"path\":\"<workspace-relative directory>\"}\n\
     {\"tool\":\"read\",\"path\":\"<workspace-relative file>\"}\n\
     {\"tool\":\"create\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
     {\"tool\":\"edit\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
     {\"final\":\"<final answer>\"}\n\
     [PURPOSE]\n";

const INSTRUCTION_MARKER: &str = "\n[INSTRUCTION]\n";
const PAST_FACTS_MARKER: &str = "\n[PAST EXECUTED FACTS]\n";
const TOOL_CALL_MARKER: &str = "\n[TOOL CALL]\n";
const TOOL_RESULT_MARKER: &str = "\n[TOOL RESULT]\n";

const OMISSION_NOTE: &str = "\n[NOTE] earlier tool exchanges were omitted to fit the input bound";

fn assemble_logical_input(
    purpose: &str,
    instructions: &[String],
    facts: &[crate::report::PastExecutedFact],
    exchanges: &[TaskAgentActionExchange],
    omitted: bool,
) -> String {
    let mut input = String::from(RESPONSE_FORMAT_PREAMBLE);
    input.push_str(purpose);
    for instruction in instructions {
        input.push_str(INSTRUCTION_MARKER);
        input.push_str(instruction);
    }
    input.push_str(PAST_FACTS_MARKER);
    for fact in facts {
        input.push_str(&fact.line);
        input.push('\n');
    }
    if omitted {
        input.push_str(OMISSION_NOTE);
    }
    for exchange in exchanges {
        push_exchange(&mut input, exchange);
    }
    input
}

/// Appends one Action exchange in the fixed tool-call / tool-result framing.
fn push_exchange(input: &mut String, exchange: &TaskAgentActionExchange) {
    input.push_str(TOOL_CALL_MARKER);
    input.push_str(exchange.request.text());
    input.push_str(TOOL_RESULT_MARKER);
    input.push_str(exchange.observation.text());
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
fn fit_exchanges(
    exchanges: &[TaskAgentActionExchange],
    head_len: usize,
    budget: usize,
) -> (&[TaskAgentActionExchange], bool) {
    if head_len >= budget {
        return (exchanges, false);
    }
    let reserved = budget - head_len;
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
        return (exchanges, false);
    }
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
        return (exchanges, false);
    }
    (&exchanges[start..], true)
}

/// The assembled length of one transcript exchange, in Unicode scalar values,
/// measured through the one appender so the framing cannot drift.
fn exchange_input_len(exchange: &TaskAgentActionExchange) -> usize {
    let mut scratch = String::new();
    push_exchange(&mut scratch, exchange);
    scratch.chars().count()
}

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
            let sealed = repository.load_delegation_result(delegation).await?;
            if sealed.is_some() {
                return Ok(TaskAgentTurnOutcome::ExecutionSealed { delegation });
            }
            Ok(TaskAgentTurnOutcome::StaleTaskRevision {
                current: loaded.record.task.reference,
            })
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
    let Some(delegation) = repository.load_delegation(delegation).await? else {
        return Ok(PremiseLoad::MissingDelegation);
    };
    let Some(record) = repository.load_task(delegation.task.task).await? else {
        return Ok(PremiseLoad::MissingTask {
            task: delegation.task.task,
        });
    };
    Ok(PremiseLoad::Loaded(Box::new(LoadedPremise {
        delegation,
        record,
    })))
}
