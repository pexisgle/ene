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
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    #[error("task agent inference unavailable: {reason}")]
    InferenceUnavailable { reason: String },
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
        return Err(TaskAgentTurnError::StorageUnavailable {
            reason: String::from("task context has no adopted purpose entry"),
        });
    };
    let past = repository
        .load_past_executed_facts(record.task.reference.task)
        .await
        .map_err(storage_error)?;
    if past.has_more {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("past executed facts exceed the turn bound"),
        });
    }
    for fact in &past.facts {
        data_use.push(fact.source);
    }
    let (kept_exchanges, omitted) = fit_exchanges(
        purpose_text,
        &instruction_texts,
        &past.facts,
        &premise.exchanges,
        inference.input_budget(),
    );
    for exchange in kept_exchanges {
        data_use.push(exchange.observation.occurrence().as_raw());
    }
    let raw_input = assemble_logical_input(
        purpose_text,
        &instruction_texts,
        &past.facts,
        kept_exchanges,
        omitted,
    );
    if raw_input.chars().count() > inference.input_budget()
        && fixed_input_len(purpose_text, &instruction_texts, &past.facts)
            >= inference.input_budget()
    {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("past executed facts exceed the input budget"),
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
        input.push_str(TOOL_CALL_MARKER);
        input.push_str(exchange.request.text());
        input.push_str(TOOL_RESULT_MARKER);
        input.push_str(exchange.observation.text());
    }
    input
}

/// The never-omitted head of the logical input: the protocol preamble,
/// the relied purpose, every resolved instruction body, and the
/// past-executed facts block. When this alone reaches the port budget, no
/// transcript trimming could produce a fitting input.
fn fixed_input_len(
    purpose: &str,
    instructions: &[String],
    facts: &[crate::report::PastExecutedFact],
) -> usize {
    RESPONSE_FORMAT_PREAMBLE.chars().count()
        + purpose.chars().count()
        + instructions
            .iter()
            .map(|text| INSTRUCTION_MARKER.chars().count() + text.chars().count())
            .sum::<usize>()
        + PAST_FACTS_MARKER.chars().count()
        + facts
            .iter()
            .map(|fact| fact.line.chars().count() + 1)
            .sum::<usize>()
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
    facts: &[crate::report::PastExecutedFact],
    exchanges: &'a [TaskAgentActionExchange],
    budget: usize,
) -> (&'a [TaskAgentActionExchange], bool) {
    let prefix = fixed_input_len(purpose, instructions, facts);
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

fn exchange_input_len(exchange: &TaskAgentActionExchange) -> usize {
    TOOL_CALL_MARKER.chars().count()
        + exchange.request.text().chars().count()
        + TOOL_RESULT_MARKER.chars().count()
        + exchange.observation.text().chars().count()
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
