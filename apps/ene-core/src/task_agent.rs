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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ene_credential::{CredentialSetRevision, ScrubbedText};
    use ene_inference::{AuthorizedInference, DeltaSink};
    use ene_primitive::RawId;
    use ene_task::{DelegationId, TaskId, TaskRef, TaskRevision};

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Mutex<Vec<&'static str>>,
        premises: Mutex<Vec<TaskAgentAttemptPremise>>,
    }

    impl RecordingExecutor {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().expect("call capture lock").clone()
        }

        fn premised(&self) -> Vec<TaskAgentAttemptPremise> {
            self.premises.lock().expect("premise capture lock").clone()
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().expect("call capture lock").push(call);
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "in-test fake; async matches the executor contract"
    )]
    impl InferenceExecutor for RecordingExecutor {
        async fn admit_dialogue(&self) -> Result<Admission, InferenceTechnicalError> {
            self.record("admit_dialogue");
            Ok(Admission::Declined(NotSentReason::NotInAllowlist))
        }

        async fn admit_learning(&self) -> Result<Admission, InferenceTechnicalError> {
            self.record("admit_learning");
            Ok(Admission::Declined(NotSentReason::NotInAllowlist))
        }

        async fn admit_task_agent(
            &self,
            task_agent: TaskAgentAttemptPremise,
        ) -> Result<Admission, InferenceTechnicalError> {
            self.record("admit_task_agent");
            self.premises
                .lock()
                .expect("premise capture lock")
                .push(task_agent);
            Ok(Admission::Declined(NotSentReason::SetupIncomplete))
        }

        async fn dispatch(
            &self,
            _authorized: AuthorizedInference,
            _prompt: ScrubbedText,
            _sink: &mut (dyn DeltaSink + Send),
            _abort: Option<&DispatchAbort>,
        ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
            self.record("dispatch");
            Err(InferenceTechnicalError::ResponseLost)
        }
    }

    struct ScrubRefs(ene_credential::CredentialSetRevision);

    impl ene_credential::CredentialRefRepository for ScrubRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn list_refs(
            &self,
        ) -> Result<Vec<ene_credential::CredentialRef>, ene_credential::CredentialTechnicalError>
        {
            Ok(Vec::new())
        }
    }

    impl ene_credential::CredentialSetRepository for ScrubRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn current_set_revision(
            &self,
        ) -> Result<ene_credential::CredentialSetRevision, ene_credential::CredentialTechnicalError>
        {
            Ok(self.0)
        }
    }

    async fn scrub_fixture(
        text: &str,
        revision: ene_credential::CredentialSetRevision,
    ) -> ScrubbedText {
        use ene_credential::SecretScrubber as _;
        ene_credential::CredentialScrubber {
            refs: &ScrubRefs(revision),
            store: &ene_credential::MemoryCredentialStore::new(),
        }
        .scrub(text)
        .await
        .expect("fixture registry is readable")
    }
    async fn task_agent_premise() -> TaskAgentInferencePremise {
        TaskAgentInferencePremise {
            delegation: DelegationId::generate(),
            task: TaskRef {
                task: TaskId::generate(),
                revision: TaskRevision::initial(),
            },
            prompt: scrub_fixture("delegated input", CredentialSetRevision::initial()).await,
            data_use: vec![RawId::new(), RawId::new()],
        }
    }

    #[tokio::test]
    async fn adapter_admits_only_as_task_agent_and_maps_the_correlation() {
        let executor = RecordingExecutor::default();
        let adapter = TaskAgentInferenceAdapter::new(&executor, None);
        let premise = task_agent_premise().await;
        let outcome = adapter
            .infer(premise.clone())
            .await
            .expect("a decline is a domain outcome, not a technical error");
        assert_eq!(
            outcome,
            TaskAgentInferenceOutcome::NotSent(TaskAgentNotSent::SetupIncomplete),
            "the inference decline mirrors into the task vocabulary"
        );
        assert_eq!(
            executor.calls(),
            vec!["admit_task_agent"],
            "a Task Agent turn never admits as dialogue or learning"
        );
        let mapped = executor.premised();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].delegation, premise.delegation.as_raw());
        assert_eq!(mapped[0].task, premise.task.task.as_raw());
        assert_eq!(
            mapped[0].task_revision.as_u64(),
            premise.task.revision.as_u64()
        );
        assert_eq!(
            mapped[0].data_use, premise.data_use,
            "the ordered source correlation travels to the claim unchanged"
        );
    }

    #[test]
    fn not_sent_reasons_mirror_one_for_one() {
        use TaskAgentInferenceOutcome as Outcome;
        use TaskAgentNotSent as NotSent;

        for (reason, expected) in [
            (
                NotSentReason::SetupIncomplete,
                Outcome::NotSent(NotSent::SetupIncomplete),
            ),
            (
                NotSentReason::NotInAllowlist,
                Outcome::NotSent(NotSent::NotInAllowlist),
            ),
            (
                NotSentReason::ConsentStale,
                Outcome::NotSent(NotSent::ConsentStale),
            ),
            (
                NotSentReason::OverLimit,
                Outcome::NotSent(NotSent::OverLimit),
            ),
            (
                NotSentReason::EvaluationConsumed,
                Outcome::NotSent(NotSent::EvaluationConsumed),
            ),
            (
                NotSentReason::DataUseHeld,
                Outcome::NotSent(NotSent::DataUseHeld),
            ),
            (NotSentReason::TaskPremiseStale, Outcome::StaleTaskPremise),
        ] {
            assert_eq!(mirror_not_sent(reason), expected);
        }
    }

    #[test]
    fn inference_errors_map_to_fixed_classes_without_payloads() {
        for (error, expected) in [
            (
                InferenceTechnicalError::ProviderTransportFailed(String::from("secret body")),
                "provider transport failed",
            ),
            (
                InferenceTechnicalError::StreamAborted {
                    reason: String::from("secret body"),
                },
                "stream aborted",
            ),
            (
                InferenceTechnicalError::HttpClientBuildFailed,
                "http client build failed",
            ),
            (
                InferenceTechnicalError::ResponseLost,
                "provider response lost",
            ),
            (
                InferenceTechnicalError::StorageUnavailable {
                    reason: String::from("secret body"),
                },
                "inference storage unavailable",
            ),
        ] {
            let mapped = inference_unavailable(error);
            let TaskAgentInferenceError::InferenceUnavailable { reason } = mapped;
            assert_eq!(reason, expected);
            assert!(
                !reason.contains("secret body"),
                "the technical class never carries a transport payload"
            );
        }
    }
}
