//! Host adapter from the Task-owned Task Agent inference port to the
//! inference boundary.
//!
//! Composition only: the task layer keeps the port and never sees permission,
//! credential, or provider internals. This adapter maps the opaque delegation
//! correlation into the inference-owned premise and mirrors the inference
//! outcomes without adding behavior. The provider output is handed back as a
//! Task Agent output, never as Task completion.

use ene_companion::{
    ActivityId, ActivityRepository, HistoryMessage, HistoryRepository, HistoryRole,
    ManagementActivity,
};
use ene_inference::{
    Admission, DiscardSink, DispatchAbort, InferenceDispatchOutcome, InferenceExecutor,
    InferenceTechnicalError, NotSentReason, TaskAgentAttemptPremise,
};
use ene_primitive::RevisionInner;
use ene_task::{
    TaskAgentInference, TaskAgentInferenceError, TaskAgentInferenceOutcome,
    TaskAgentInferencePremise, TaskAgentNotSent, TaskAgentOutput, TaskContextOrigin,
    TaskContextOriginKind, TaskInstructionRole, TaskInstructionSource, TaskInstructionSourceError,
    TaskInstructionSourceRecord,
};

/// Adapts one [`InferenceExecutor`] to the Task Agent port.
///
/// `abort` is the running execution's local cooperative stop token, when the
/// caller runs under one: the adapter forwards it into the dispatch boundary,
/// which owns the post-claim accounting, so an abort stops the provider wait
/// without losing the attempt's usage fact. A bare turn outside an execution
/// passes `None`.
pub struct TaskAgentInferenceAdapter<'a, I> {
    executor: &'a I,
    abort: Option<&'a DispatchAbort>,
}

impl<'a, I> TaskAgentInferenceAdapter<'a, I> {
    #[must_use]
    pub fn new(executor: &'a I, abort: Option<&'a DispatchAbort>) -> Self {
        Self { executor, abort }
    }
}

impl<I: InferenceExecutor> TaskAgentInference for TaskAgentInferenceAdapter<'_, I> {
    fn input_budget(&self) -> usize {
        ene_inference::MAX_INPUT_CHARS
    }

    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
        let correlation = TaskAgentAttemptPremise {
            delegation: premise.delegation.as_raw(),
            task: premise.task.task.as_raw(),
            task_revision: RevisionInner::from_u64(premise.task.revision.as_u64()),
            data_use: premise.data_use,
        };
        let admission = self
            .executor
            .admit_task_agent(correlation)
            .await
            .map_err(inference_unavailable)?;
        let authorized = match admission {
            Admission::Admitted(authorized) => authorized,
            Admission::Declined(reason) => return Ok(mirror_not_sent(reason)),
        };
        let mut sink = DiscardSink;
        match self
            .executor
            .dispatch(*authorized, premise.prompt, &mut sink, self.abort)
            .await
        {
            Ok(InferenceDispatchOutcome::Completed { arrival, adopted }) => {
                Ok(TaskAgentInferenceOutcome::Produced {
                    output: TaskAgentOutput::new(arrival.output_text),
                    adoption_consent_current: adopted,
                })
            }
            Ok(InferenceDispatchOutcome::NotSent(reason)) => Ok(mirror_not_sent(reason)),
            Ok(InferenceDispatchOutcome::Aborted) => Ok(TaskAgentInferenceOutcome::Aborted),
            Err(error) => Err(inference_unavailable(error)),
        }
    }
}

/// Adapts the companion-owned canonical sources to the Task-owned
/// instruction-source port.
///
/// The adapter resolves the origin kind to its table — Owner conversation
/// messages by History primary key, first-party management activities by
/// activity primary key — with one bounded single-record read each, and
/// maps the row into the Task-owned record. It adds no behavior: no body is
/// cached or copied into Task state, an absent row is `Ok(None)`, and a
/// malformed row or read failure becomes a fixed-class technical error
/// without the row or the body. Timeline loads, recent windows, and command
/// lookups are never a substitute for either read.
pub struct OwnerInstructionSource<'a, H, A> {
    history: &'a H,
    activity: &'a A,
}

impl<'a, H, A> OwnerInstructionSource<'a, H, A> {
    #[must_use]
    pub fn new(history: &'a H, activity: &'a A) -> Self {
        Self { history, activity }
    }
}

impl<H: HistoryRepository + Sync, A: ActivityRepository + Sync> TaskInstructionSource
    for OwnerInstructionSource<'_, H, A>
{
    async fn load_owner_instruction(
        &self,
        origin: TaskContextOrigin,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        match origin.kind {
            TaskContextOriginKind::OwnerConversation => {
                let message = self
                    .history
                    .load_message(origin.source)
                    .await
                    .map_err(|_| TaskInstructionSourceError::SourceUnavailable {
                        reason: String::from("history message read failed"),
                    })?;
                Ok(message.map(map_history_message))
            }
            TaskContextOriginKind::OwnerManagement => {
                let activity = self
                    .activity
                    .load_activity(ActivityId::from_raw(origin.source))
                    .await
                    .map_err(|_| TaskInstructionSourceError::SourceUnavailable {
                        reason: String::from("activity record read failed"),
                    })?;
                Ok(activity.map(map_activity_record))
            }
            // No body producer exists for these kinds: the turn fails
            // closed on the origin kind before any read is attempted.
            TaskContextOriginKind::Spontaneous | TaskContextOriginKind::ScheduleOccurrence => {
                Err(TaskInstructionSourceError::SourceUnavailable {
                    reason: String::from("unsupported instruction origin kind"),
                })
            }
        }
    }
}

/// The whole mapping is one direction: no `HistoryMessage` crosses into
/// `ene-task`, and only the fields the turn needs are carried.
fn map_history_message(message: HistoryMessage) -> TaskInstructionSourceRecord {
    TaskInstructionSourceRecord {
        kind: TaskContextOriginKind::OwnerConversation,
        source: message.id,
        companion: message.companion.as_raw(),
        role: match message.role {
            HistoryRole::Owner => TaskInstructionRole::Owner,
            HistoryRole::Companion => TaskInstructionRole::Companion,
        },
        text: message.text,
    }
}

/// A first-party management activity is an Owner input by construction: it
/// records an Owner-authored instruction the Host accepted on the trusted
/// inlet, so it maps to the Owner role with its recorded companion.
fn map_activity_record(activity: ManagementActivity) -> TaskInstructionSourceRecord {
    TaskInstructionSourceRecord {
        kind: TaskContextOriginKind::OwnerManagement,
        source: activity.id.as_raw(),
        companion: activity.companion.as_raw(),
        role: TaskInstructionRole::Owner,
        text: activity.body,
    }
}

/// Mirrors the inference not-sent vocabulary one-for-one. A moved Task
/// premise stays [`TaskAgentInferenceOutcome::StaleTaskPremise`] and is never
/// folded into consent staleness.
fn mirror_not_sent(reason: NotSentReason) -> TaskAgentInferenceOutcome {
    use TaskAgentInferenceOutcome as Outcome;
    use TaskAgentNotSent as NotSent;

    match reason {
        NotSentReason::TaskPremiseStale => Outcome::StaleTaskPremise,
        NotSentReason::SetupIncomplete => Outcome::NotSent(NotSent::SetupIncomplete),
        NotSentReason::NotInAllowlist => Outcome::NotSent(NotSent::NotInAllowlist),
        NotSentReason::ConsentStale => Outcome::NotSent(NotSent::ConsentStale),
        NotSentReason::OverLimit => Outcome::NotSent(NotSent::OverLimit),
        NotSentReason::EvaluationConsumed => Outcome::NotSent(NotSent::EvaluationConsumed),
        NotSentReason::DataUseHeld => Outcome::NotSent(NotSent::DataUseHeld),
        NotSentReason::UsageCapReached => Outcome::NotSent(NotSent::UsageCapReached),
        NotSentReason::UsageCapIndeterminate => Outcome::NotSent(NotSent::UsageCapIndeterminate),
    }
}

/// Short technical class; never a body or transport payload.
fn inference_unavailable(error: InferenceTechnicalError) -> TaskAgentInferenceError {
    let reason = match error {
        InferenceTechnicalError::ProviderTransportFailed(_) => "provider transport failed",
        InferenceTechnicalError::StreamAborted { .. } => "stream aborted",
        InferenceTechnicalError::HttpClientBuildFailed => "http client build failed",
        InferenceTechnicalError::ResponseLost => "provider response lost",
        InferenceTechnicalError::PricingCatalogUnavailable => "pricing catalog unavailable",
        InferenceTechnicalError::CostProjectionFailed { .. } => "usage cost projection failed",
        InferenceTechnicalError::StorageUnavailable { .. } => "inference storage unavailable",
    };
    TaskAgentInferenceError::InferenceUnavailable {
        reason: String::from(reason),
    }
}
