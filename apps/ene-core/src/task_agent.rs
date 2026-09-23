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
            TaskContextOriginKind::Spontaneous | TaskContextOriginKind::ScheduleOccurrence => {
                Err(TaskInstructionSourceError::SourceUnavailable {
                    reason: String::from("unsupported instruction origin kind"),
                })
            }
        }
    }
}

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

fn map_activity_record(activity: ManagementActivity) -> TaskInstructionSourceRecord {
    TaskInstructionSourceRecord {
        kind: TaskContextOriginKind::OwnerManagement,
        source: activity.id.as_raw(),
        companion: activity.companion.as_raw(),
        role: TaskInstructionRole::Owner,
        text: activity.body,
    }
}

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
