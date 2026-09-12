//! Task Agent inference turn harness (H-A / K-E).
//!
//! One turn runs exactly one delegated inference through the Task-owned
//! [`TaskAgentInference`] port. The harness assembles the logical input
//! itself: the relied revision's adopted-purpose text is read from the
//! `task_revision` snapshot, where the purpose text is canonical, and only
//! the injected [`SecretScrubber`]'s output reaches the port. This module
//! never constructs a [`ScrubbedText`] literal, never sends an instruction
//! body, and never sends workspace or file content.
//!
//! The port keeps `ene-task` free of permission and credential concrete
//! types: the Host composition root (`apps/ene-core`) implements it against
//! `ene-inference`. The delegation row is a durable correspondence, not a
//! liveness claim, and provider output is a fact about the call, not task
//! completion, adoption, or external-effect success.

use ene_credential::{ScrubbedText, SecretScrubber};

use crate::delegation::DelegationId;
use crate::repository::{TaskRepository, TaskTechnicalError};
use crate::task::{TaskId, TaskRef};

/// One turn request from the caller; carries no liveness claim.
///
/// The delegation identity names a correspondence row, and the row's
/// existence never proves the ephemeral agent is alive or that delegated work
/// is running; the turn re-checks the task premise instead of assuming it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskAgentTurnPremise {
    /// The delegation correspondence to run.
    pub delegation: DelegationId,
}

/// What the port receives.
///
/// `prompt` is only ever the injected scrubber's output; the harness never
/// assembles a `ScrubbedText` itself. [`core::fmt::Debug`] redacts the
/// prompt so diagnostic output cannot leak its text.
#[derive(Clone)]
pub struct TaskAgentInferencePremise {
    /// The delegation correspondence this turn runs under.
    pub delegation: DelegationId,
    /// The relied-on Task revision.
    pub task: TaskRef,
    /// Scrubbed logical input for this turn.
    pub prompt: ScrubbedText,
}

impl core::fmt::Debug for TaskAgentInferencePremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskAgentInferencePremise")
            .field("delegation", &self.delegation)
            .field("task", &self.task)
            .field("prompt", &"[redacted]")
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
    /// under before the attempt claim or before adoption.
    ConsentStale,
    /// The input exceeds the inference input bound.
    OverLimit,
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different fingerprint.
    EvaluationConsumed,
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
/// the claim is still reported as stale by the port. The logical input is
/// only the relied revision's adopted-purpose text, canonical in the
/// `task_revision` snapshot; no instruction body and no workspace or file
/// content is copied into the prompt. A scrub failure fails closed as
/// [`TaskAgentTurnError::InputUnavailable`] and nothing is sent.
///
/// Outcome mapping: `Produced` carries the output and consent flag without
/// adopting either, `StaleTaskPremise` is re-read as `MissingDelegation`,
/// `MissingTask`, or `StaleTaskRevision { current }`, and `NotSent` reasons
/// pass through unchanged. Repository technical errors map to
/// [`TaskAgentTurnError::StorageUnavailable`] and port technical errors to
/// [`TaskAgentTurnError::InferenceUnavailable`]; domain outcomes are never
/// folded into either.
pub async fn orchestrate_task_agent_turn(
    repository: &impl TaskRepository,
    inference: &impl TaskAgentInference,
    scrubber: &impl SecretScrubber,
    premise: TaskAgentTurnPremise,
) -> Result<TaskAgentTurnOutcome, TaskAgentTurnError> {
    let Some(delegation) = repository
        .load_delegation(premise.delegation)
        .await
        .map_err(storage_error)?
    else {
        return Ok(TaskAgentTurnOutcome::MissingDelegation {
            delegation: premise.delegation,
        });
    };
    let Some(record) = repository
        .load_task(delegation.task.task)
        .await
        .map_err(storage_error)?
    else {
        return Ok(TaskAgentTurnOutcome::MissingTask {
            task: delegation.task.task,
        });
    };
    if record.task.reference != delegation.task {
        return Ok(TaskAgentTurnOutcome::StaleTaskRevision {
            current: record.task.reference,
        });
    }
    // The precheck matched, so `record.revision` is the relied revision's
    // snapshot. The scrub stays ahead of the port call and its failure fails
    // closed: raw purpose text must never reach the port.
    let Ok(prompt) = scrubber.scrub(&record.revision.purpose_text.text).await else {
        return Err(TaskAgentTurnError::InputUnavailable {
            reason: String::from("credential scrub failed"),
        });
    };
    let outcome = inference
        .infer(TaskAgentInferencePremise {
            delegation: delegation.delegation,
            task: delegation.task,
            prompt,
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
            let Some(delegation) = repository
                .load_delegation(premise.delegation)
                .await
                .map_err(storage_error)?
            else {
                return Ok(TaskAgentTurnOutcome::MissingDelegation {
                    delegation: premise.delegation,
                });
            };
            let Some(record) = repository
                .load_task(delegation.task.task)
                .await
                .map_err(storage_error)?
            else {
                return Ok(TaskAgentTurnOutcome::MissingTask {
                    task: delegation.task.task,
                });
            };
            TaskAgentTurnOutcome::StaleTaskRevision {
                current: record.task.reference,
            }
        }
        TaskAgentInferenceOutcome::NotSent(reason) => TaskAgentTurnOutcome::NotSent(reason),
    })
}

fn storage_error(error: TaskTechnicalError) -> TaskAgentTurnError {
    match error {
        TaskTechnicalError::StorageUnavailable { reason } => {
            TaskAgentTurnError::StorageUnavailable { reason }
        }
    }
}

fn inference_error(error: TaskAgentInferenceError) -> TaskAgentTurnError {
    match error {
        TaskAgentInferenceError::InferenceUnavailable { reason } => {
            TaskAgentTurnError::InferenceUnavailable { reason }
        }
    }
}
