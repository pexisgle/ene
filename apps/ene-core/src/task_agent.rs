//! Host adapter from the Task-owned Task Agent inference port to the
//! inference boundary.
//!
//! Composition only: the task layer keeps the port and never sees permission,
//! credential, or provider internals. This adapter maps the opaque delegation
//! correlation into the inference-owned premise and mirrors the inference
//! outcomes without adding behavior. The provider output is handed back as a
//! Task Agent output, never as Task completion.

use ene_inference::{
    Admission, DiscardSink, InferenceDispatchOutcome, InferenceExecutor, InferenceTechnicalError,
    NotSentReason, TaskAgentAttemptPremise,
};
use ene_primitive::RevisionInner;
use ene_task::{
    TaskAgentInference, TaskAgentInferenceError, TaskAgentInferenceOutcome,
    TaskAgentInferencePremise, TaskAgentNotSent, TaskAgentOutput,
};

/// Adapts one [`InferenceExecutor`] to the Task Agent port.
pub struct TaskAgentInferenceAdapter<'a, I> {
    executor: &'a I,
}

impl<'a, I> TaskAgentInferenceAdapter<'a, I> {
    #[must_use]
    pub fn new(executor: &'a I) -> Self {
        Self { executor }
    }
}

impl<I: InferenceExecutor> TaskAgentInference for TaskAgentInferenceAdapter<'_, I> {
    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
        let correlation = TaskAgentAttemptPremise {
            delegation: premise.delegation.as_raw(),
            task: premise.task.task.as_raw(),
            task_revision: RevisionInner::from_u64(premise.task.revision.as_u64()),
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
            .dispatch(*authorized, premise.prompt, &mut sink)
            .await
        {
            Ok(InferenceDispatchOutcome::Completed { arrival, adopted }) => {
                Ok(TaskAgentInferenceOutcome::Produced {
                    output: TaskAgentOutput::new(arrival.output_text),
                    adoption_consent_current: adopted,
                })
            }
            Ok(InferenceDispatchOutcome::NotSent(reason)) => Ok(mirror_not_sent(reason)),
            Err(error) => Err(inference_unavailable(error)),
        }
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
    }
}

/// Short technical class; never a body or transport payload.
fn inference_unavailable(error: InferenceTechnicalError) -> TaskAgentInferenceError {
    let reason = match error {
        InferenceTechnicalError::ProviderTransportFailed(_) => "provider transport failed",
        InferenceTechnicalError::StreamAborted { .. } => "stream aborted",
        InferenceTechnicalError::HttpClientBuildFailed => "http client build failed",
        InferenceTechnicalError::ResponseLost => "provider response lost",
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
        ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
            self.record("dispatch");
            Err(InferenceTechnicalError::ResponseLost)
        }
    }

    fn task_agent_premise() -> TaskAgentInferencePremise {
        TaskAgentInferencePremise {
            delegation: DelegationId::generate(),
            task: TaskRef {
                task: TaskId::generate(),
                revision: TaskRevision::initial(),
            },
            prompt: ScrubbedText {
                text: String::from("delegated input"),
                credential_set: CredentialSetRevision::initial(),
            },
        }
    }

    #[tokio::test]
    async fn adapter_admits_only_as_task_agent_and_maps_the_correlation() {
        let executor = RecordingExecutor::default();
        let adapter = TaskAgentInferenceAdapter::new(&executor);
        let premise = task_agent_premise();
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
            premise.task.revision.as_u64(),
            "the relied revision travels as the (task, revision) pair"
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
