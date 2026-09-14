//! Autonomous Task Agent ↔ Action loop checks (Stage 4 slice D).
//!
//! The inference port is scripted and the Action side runs against a real
//! store and a real scratch workspace, so the checks pin the whole boundary
//! order: scripted provider output -> parsed directive -> AU14-claimed turn /
//! AU5-claimed Action -> observation replay -> explicit finalization (AU15a)
//! and adoption (AU15b). Malformed output, the turn bound, refusals, unknown
//! effects, cancel, and steering must all stop the loop without sealing a
//! result they did not earn.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_action::{ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, OperationKind};
use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
use ene_inference::DispatchAbort;
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationScope, TaskAgentEphemeralId, TaskAgentInference, TaskAgentInferenceError,
    TaskAgentInferenceOutcome, TaskAgentInferencePremise, TaskAgentOutput, TaskCancelOutcome,
    TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise,
    TaskInstructionSource, TaskInstructionSourceError, TaskInstructionSourceRecord, TaskProgress,
    TaskPurpose, TaskRef, TaskRepository as _, TaskResultAcceptance, WorkspaceAssocId,
    WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef,
};

use super::{
    CompletedFollowUp, DEFAULT_MAX_TURNS, TaskAgentDirective, TaskAgentProtocolViolation,
    TaskAgentRunOutcome, TaskAgentRunRefusal, TaskExecutionRegistry, completed_follow_up,
    parse_directive, run_task_agent_execution,
};

/// One Task with a real workspace association and one delegation.
struct Fixture {
    _store_dir: tempfile::TempDir,
    store: Store,
    workspace: tempfile::TempDir,
    task: TaskRef,
    delegation: DelegationId,
}

async fn fixture() -> Fixture {
    let store_dir = tempfile::tempdir().expect("store directory");
    let store = Store::open(&store_dir.path().join("task-run.db"))
        .await
        .expect("store opens");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let assoc = WorkspaceAssocId::generate();
    let folder = WorkspaceFolderRef {
        path: workspace.path().to_string_lossy().into_owned(),
    };
    let task = store
        .create_task(TaskCreationPremise {
            task: ene_task::TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("write the report"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: Some(WorkspaceAssociationPremise {
                assoc,
                need: WorkspaceNeedRef {
                    folder: folder.clone(),
                    save_target: None,
                },
            }),
        })
        .await
        .expect("task creation commits");
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope {
                workspace: Some(DelegatedWorkspace {
                    assoc,
                    folder,
                    save_target: None,
                }),
            },
        })
        .await
        .expect("delegation creation commits");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    Fixture {
        _store_dir: store_dir,
        store,
        workspace,
        task,
        delegation,
    }
}

/// Answers one provider output per turn and captures every premise.
struct ScriptedInference {
    replies: Mutex<VecDeque<Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>>>,
    premises: Mutex<Vec<TaskAgentInferencePremise>>,
    budget: usize,
}

impl ScriptedInference {
    fn new(replies: Vec<&str>) -> Self {
        Self::with_consent(replies, true)
    }

    /// Scripts one turn that reports the local abort before producing output.
    fn aborted() -> Self {
        Self {
            replies: Mutex::new(VecDeque::from(vec![Ok(TaskAgentInferenceOutcome::Aborted)])),
            premises: Mutex::new(Vec::new()),
        }
    }

    /// Scripts replies whose post-wait consent flag is `adoption_consent_current`.
    fn with_consent(replies: Vec<&str>, adoption_consent_current: bool) -> Self {
        Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .map(|text| {
                        Ok(TaskAgentInferenceOutcome::Produced {
                            output: TaskAgentOutput::new(text.to_owned()),
                            adoption_consent_current,
                        })
                    })
                    .collect(),
            ),
            premises: Mutex::new(Vec::new()),
            budget: ene_inference::MAX_INPUT_CHARS,
        }
    }

    /// Overrides the input budget this fake port admits.
    fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        self
    }

    fn premises(&self) -> Vec<TaskAgentInferencePremise> {
        self.premises.lock().expect("premise capture lock").clone()
    }

    fn calls(&self) -> usize {
        self.premises.lock().expect("premise capture lock").len()
    }
}

impl TaskAgentInference for ScriptedInference {
    fn input_budget(&self) -> usize {
        self.budget
    }

    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
        self.premises
            .lock()
            .expect("premise capture lock")
            .push(premise);
        self.replies
            .lock()
            .expect("script lock")
            .pop_front()
            .expect("every exercised turn has a scripted reply")
    }
}

/// The test scrubber: every input is replaced by a marker prefix, so tests can
/// assert the exact raw input the harness assembled.
#[derive(Default)]
struct MarkerScrubber {
    inputs: Mutex<Vec<String>>,
}

impl MarkerScrubber {
    fn inputs(&self) -> Vec<String> {
        self.inputs.lock().expect("scrub capture lock").clone()
    }
}

impl SecretScrubber for MarkerScrubber {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        self.inputs
            .lock()
            .expect("scrub capture lock")
            .push(text.to_owned());
        Ok(ScrubbedText {
            text: format!("[scrubbed] {text}"),
            credential_set: CredentialSetRevision::initial(),
        })
    }
}

/// The fixture Tasks have no adopted instructions, so no body is ever read.
struct NoInstructions;

impl TaskInstructionSource for NoInstructions {
    async fn load_owner_instruction(
        &self,
        _source: RawId,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        Ok(None)
    }
}

async fn run(
    fixture: &Fixture,
    inference: &ScriptedInference,
    scrubber: &MarkerScrubber,
    max_turns: u32,
) -> Result<TaskAgentRunOutcome, super::TaskAgentRunError> {
    run_task_agent_execution(
        &fixture.store,
        &NoInstructions,
        inference,
        scrubber,
        fixture.delegation,
        max_turns,
        &DispatchAbort::default(),
    )
    .await
}

#[tokio::test]
async fn a_consent_lapse_discards_the_output_without_action_or_result() {
    let fixture = fixture().await;
    let inference = ScriptedInference::with_consent(
        vec!["{\"tool\":\"create\",\"path\":\"never.md\",\"content\":\"x\"}"],
        false,
    );
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a lapsed consent is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::ConsentStaleAfterSend { turn: 1 },
        "a sent-but-discarded output is its own outcome, never the pre-send NotSent refusal"
    );
    assert_eq!(inference.calls(), 1, "the send itself was already started");
    assert!(
        !fixture.workspace.path().join("never.md").exists(),
        "a discarded output never drives an Action"
    );
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none(),
        "a discarded output never seals the execution"
    );
}

#[tokio::test]
async fn a_transcript_that_outgrows_the_budget_keeps_the_task_running() {
    let fixture = fixture().await;
    std::fs::write(fixture.workspace.path().join("input.txt"), "n".repeat(200))
        .expect("input fixture");
    let inference = ScriptedInference::new(vec![
        r#"{"tool":"read","path":"input.txt"}"#,
        r#"{"tool":"read","path":"input.txt"}"#,
        r#"{"final":"read the file twice"}"#,
    ])
    .with_budget(750);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a trimmed transcript is a domain outcome");
    assert!(
        matches!(outcome, TaskAgentRunOutcome::Finalized { .. }),
        "the execution continues instead of wedging on the input bound, got {outcome:?}"
    );
    let raw = scrubber.inputs();
    assert_eq!(raw.len(), 3, "one scrub per turn");
    for (index, input) in raw.iter().enumerate() {
        assert!(
            input.chars().count() <= 750,
            "turn {} stays within the port budget, got {}",
            index + 1,
            input.chars().count()
        );
    }
    assert_eq!(
        raw[2].matches("[TOOL CALL]").count(),
        1,
        "only the newest exchange is kept once the transcript outgrows the budget"
    );
    assert!(
        raw[2].contains("[NOTE] earlier tool exchanges were omitted to fit the input bound"),
        "the dropped exchanges are stated to the model"
    );
}

#[tokio::test]
async fn a_started_unsealed_execution_is_never_run_again() {
    let fixture = fixture().await;
    std::fs::write(fixture.workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // The first durable attempt of the delegation is its start marker, even
    // when it is an Action start rather than an inference claim: the
    // execution already began and may have stopped before its next turn.
    let started = crate::action::run_workspace_action(
        &fixture.store,
        fixture.delegation,
        OperationKind::Read,
        String::from("input.txt"),
        None,
    )
    .await
    .expect("the action answers a domain outcome");
    assert!(
        matches!(
            started,
            crate::action::WorkspaceActionHostOutcome::Completed { .. }
        ),
        "the fixture action must start and observe, got {started:?}"
    );
    assert!(
        fixture
            .store
            .delegation_has_started_work(fixture.delegation)
            .await
            .unwrap(),
        "the committed Action attempt is the durable start marker"
    );

    let inference = ScriptedInference::new(vec![r#"{"final":"never sent"}"#]);
    let scrubber = MarkerScrubber::default();
    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a spent execution is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::ExecutionAlreadyStarted {
            delegation: fixture.delegation,
        }),
        "a delegation that already started work is never run again"
    );
    assert_eq!(
        inference.calls(),
        0,
        "the refused run starts no provider call"
    );
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none(),
        "the one-shot refusal writes no seal"
    );
}

#[tokio::test]
async fn final_answer_is_recorded_and_adopted_as_completion() {
    let fixture = fixture().await;
    let inference = ScriptedInference::new(vec![r#"{"final":"report written"}"#]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a final answer is a domain outcome");
    let TaskAgentRunOutcome::Finalized { result, acceptance } = outcome else {
        panic!("expected Finalized, got {outcome:?}");
    };
    assert_eq!(
        acceptance,
        TaskResultAcceptance::AdoptedAsCompletion(fixture.task)
    );
    assert_eq!(result.body.text(), "report written");
    assert_eq!(result.attempt_refs, Vec::<RawId>::new());
    let loaded = fixture
        .store
        .load_task(fixture.task.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Completed);
    assert_eq!(loaded.task.adopted_result, Some(result.result));
    assert_eq!(inference.calls(), 1);
    let premises = inference.premises();
    assert!(
        premises[0].prompt.text.contains("write the report"),
        "the turn carries the relied purpose"
    );
}

#[tokio::test]
async fn read_then_create_then_final_runs_the_whole_loop() {
    let fixture = fixture().await;
    std::fs::write(fixture.workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let inference = ScriptedInference::new(vec![
        r#"{"tool":"read","path":"input.txt"}"#,
        "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# report\"}",
        r#"{"final":"report.md was created"}"#,
    ]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a completed loop is a domain outcome");
    let TaskAgentRunOutcome::Finalized { result, acceptance } = outcome else {
        panic!("expected Finalized, got {outcome:?}");
    };
    assert_eq!(
        acceptance,
        TaskResultAcceptance::AdoptedAsCompletion(fixture.task)
    );
    assert_eq!(
        result.attempt_refs.len(),
        0,
        "arrival records no correlation"
    );
    let stamped = fixture
        .store
        .load_task_result(result.result)
        .await
        .unwrap()
        .expect("the adopted result is readable");
    assert_eq!(
        stamped.attempt_refs.len(),
        2,
        "read and create are stamped as relied on"
    );
    assert_eq!(
        std::fs::read(fixture.workspace.path().join("report.md")).expect("report exists"),
        b"# report"
    );
    // The executed attempts kept their own identities and delegation.
    for attempt in &stamped.attempt_refs {
        let record = fixture
            .store
            .load_attempt(ene_action::ActionAttemptId::from_raw(*attempt))
            .await
            .unwrap()
            .expect("every relied attempt is durable");
        assert_eq!(record.delegation, fixture.delegation.as_raw());
        assert_eq!(record.certainty, ActionCertainty::ConfirmedSuccess);
    }
    assert_eq!(inference.calls(), 3);
    let premises = inference.premises();
    assert!(
        premises[2].prompt.text.contains("read ok:\nnotes"),
        "the file content is replayed as an observation, got {}",
        premises[2].prompt.text
    );
    assert!(
        premises[2].prompt.text.contains("create ok"),
        "the create observation is replayed"
    );
    assert!(
        premise_exchange_count(&premises[1]) == 1 && premise_exchange_count(&premises[2]) == 2,
        "each turn replays exactly the prior exchanges"
    );
    assert!(
        !premises[0].prompt.text.contains("[TOOL RESULT]"),
        "the first turn has no transcript"
    );
    let scrubbed = scrubber.inputs();
    assert_eq!(scrubbed.len(), 3, "each turn is scrubbed exactly once");
    assert!(
        scrubbed[2].contains("read ok:\nnotes") && scrubbed[2].contains("[TOOL CALL]"),
        "the raw transcript is scrubbed as one string, got {}",
        scrubbed[2]
    );
}

/// Counts the replayed tool exchanges in one captured premise.
fn premise_exchange_count(premise: &TaskAgentInferencePremise) -> usize {
    premise.prompt.text.matches("[TOOL RESULT]").count()
}

#[tokio::test]
async fn malformed_output_stops_without_action_or_result_record() {
    let fixture = fixture().await;
    let inference = ScriptedInference::new(vec!["I will read the file now."]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a protocol violation is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::ProtocolViolation {
            turn: 1,
            reason: TaskAgentProtocolViolation::NotAJsonObject,
        }
    );
    assert_eq!(inference.calls(), 1);
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none(),
        "a malformed turn never seals the execution"
    );
    let task = fixture
        .store
        .load_task(fixture.task.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.task.progress, TaskProgress::InProgress);
}

#[tokio::test]
async fn turn_bound_stops_without_sealing() {
    let fixture = fixture().await;
    let inference = ScriptedInference::new(vec![
        r#"{"tool":"list","path":""}"#,
        r#"{"final":"never reached"}"#,
    ]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, 1)
        .await
        .expect("the bound is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::TurnLimitReached { turns: 1 });
    assert_eq!(inference.calls(), 1);
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn refused_action_is_replayed_and_the_model_can_finish() {
    let fixture = fixture().await;
    let inference = ScriptedInference::new(vec![
        r#"{"tool":"read","path":"../escape.txt"}"#,
        r#"{"final":"nothing to read"}"#,
    ]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("a refused tool call is not a technical failure");
    let TaskAgentRunOutcome::Finalized { result, acceptance } = outcome else {
        panic!("expected Finalized, got {outcome:?}");
    };
    assert_eq!(
        acceptance,
        TaskResultAcceptance::AdoptedAsCompletion(fixture.task)
    );
    assert!(
        result.attempt_refs.is_empty(),
        "a refusal before the AU5 claim leaves no attempt"
    );
    let premises = inference.premises();
    assert!(
        premises[1].prompt.text.contains("refused:"),
        "the refusal class is replayed, got {}",
        premises[1].prompt.text
    );
}

#[tokio::test]
async fn steering_before_the_loop_refuses_without_provider_io() {
    let fixture = fixture().await;
    let advanced = fixture
        .store
        .forward_steering(ene_task::TaskCommitPremise {
            expected: fixture.task,
            new_purpose: Some(ene_task::TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: String::from("new direction"),
                },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: WallClockWithTz::now(),
            }),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let ene_task::TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the steering must advance the revision");
    };
    let inference = ScriptedInference::new(vec![r#"{"final":"never sent"}"#]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("stale is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::StaleTaskRevision { current })
    );
    assert_eq!(
        inference.calls(),
        0,
        "no provider I/O under a moved revision"
    );
}

#[tokio::test]
async fn cancelled_task_refuses_the_next_turn_without_provider_io() {
    let fixture = fixture().await;
    assert_eq!(
        fixture.store.cancel_task(fixture.task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    let inference = ScriptedInference::new(vec![r#"{"final":"never sent"}"#]);
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("the terminal gate is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::TaskTerminal {
            task: fixture.task.task,
            progress: TaskProgress::Cancelled,
        })
    );
    assert_eq!(inference.calls(), 0);
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_aborted_turn_stops_the_loop_as_cancelled() {
    let fixture = fixture().await;
    let inference = ScriptedInference::aborted();
    let scrubber = MarkerScrubber::default();

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("an abort is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::Cancelled);
    assert_eq!(inference.calls(), 1, "the aborted turn was the last turn");
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none(),
        "an aborted turn never seals the execution"
    );
}

#[tokio::test]
async fn cancellation_before_the_loop_stops_without_provider_io() {
    let fixture = fixture().await;
    let inference = ScriptedInference::new(vec![r#"{"final":"never sent"}"#]);
    let scrubber = MarkerScrubber::default();
    let cancellation = DispatchAbort::default();
    cancellation.abort();

    let outcome = run_task_agent_execution(
        &fixture.store,
        &NoInstructions,
        &inference,
        &scrubber,
        fixture.delegation,
        DEFAULT_MAX_TURNS,
        &cancellation,
    )
    .await
    .expect("the local stop is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::Cancelled);
    assert_eq!(inference.calls(), 0, "no provider I/O after the signal");
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .unwrap()
            .is_none(),
        "a local stop never seals the execution"
    );
}

#[test]
fn a_second_registration_of_one_delegation_is_refused_atomically() {
    let task = ene_task::TaskId::generate();
    let delegation = ene_task::DelegationId::generate();
    let registry = TaskExecutionRegistry::default();

    let first = registry
        .register(delegation, task)
        .expect("the first registration is accepted");
    assert!(
        registry.register(delegation, task).is_none(),
        "a concurrent second execution of the same delegation is refused"
    );
    let sibling = registry
        .register(ene_task::DelegationId::generate(), task)
        .expect("a sibling delegation of the same Task is still allowed");
    assert!(
        registry.cancel(task),
        "both running executions are signalled"
    );
    assert!(first.cancellation.is_aborted());
    assert!(sibling.cancellation.is_aborted());
    drop(first);
    assert!(
        registry.register(delegation, task).is_some(),
        "the delegation slot is reusable once its registration is dropped"
    );
}

#[test]
fn registry_signals_every_running_execution_of_the_task_only() {
    let task = ene_task::TaskId::generate();
    let other_task = ene_task::TaskId::generate();
    let delegation = ene_task::DelegationId::generate();
    let sibling = ene_task::DelegationId::generate();
    let elsewhere = ene_task::DelegationId::generate();
    let registry = TaskExecutionRegistry::default();
    assert!(!registry.cancel(task), "no token is registered yet");

    let registration = registry
        .register(delegation, task)
        .expect("the first registration is accepted");
    let sibling_registration = registry
        .register(sibling, task)
        .expect("a sibling delegation is accepted");
    let other = registry
        .register(elsewhere, other_task)
        .expect("another Task's delegation is accepted");
    assert!(registry.cancel(task));
    assert!(registration.cancellation.is_aborted());
    assert!(
        sibling_registration.cancellation.is_aborted(),
        "every running execution of the Task is signalled"
    );
    assert!(
        !other.cancellation.is_aborted(),
        "another Task's execution is untouched"
    );

    drop(registration);
    assert!(
        registry.cancel(task),
        "the sibling registration is still signalled after one drop"
    );
    drop(sibling_registration);
    assert!(
        !registry.cancel(task),
        "each registration removes exactly its own token"
    );
}

#[test]
fn an_unconfirmable_effect_stops_and_is_never_replayed() {
    use ene_action::{ActionOutput, EffectGrounds, ObservedEffect};

    let attempt = ActionAttemptId::generate();
    let unknown = ObservedEffect {
        certainty: ActionCertainty::Unknown,
        grounds: EffectGrounds::OutcomeUnverified,
        output: None,
    };
    match completed_follow_up(
        TaskAgentOutput::new(String::from("{}")),
        attempt,
        &unknown,
        true,
    ) {
        CompletedFollowUp::Stop(TaskAgentRunOutcome::EffectUnresolved { attempt: stopped }) => {
            assert_eq!(stopped, attempt);
        }
        other => panic!("an unknown effect must stop the loop, got {other:?}"),
    }

    // A confirmed pre-effect refusal continues with a fixed-class observation.
    let refused = ObservedEffect {
        certainty: ActionCertainty::ConfirmedFailure,
        grounds: EffectGrounds::RefusedBeforeEffect,
        output: None,
    };
    match completed_follow_up(
        TaskAgentOutput::new(String::from("{}")),
        attempt,
        &refused,
        true,
    ) {
        CompletedFollowUp::Continue(exchange) => {
            assert!(exchange.observation.text().contains("refused"));
        }
        other => panic!("a confirmed refusal is replayable, got {other:?}"),
    }

    // A confirmed success whose fact could not be recorded still continues,
    // but the observation states the durable record is unverified.
    let confirmed = ObservedEffect {
        certainty: ActionCertainty::ConfirmedSuccess,
        grounds: EffectGrounds::ObservedAtTarget,
        output: Some(ActionOutput::Updated),
    };
    match completed_follow_up(
        TaskAgentOutput::new(String::from("{}")),
        attempt,
        &confirmed,
        false,
    ) {
        CompletedFollowUp::Continue(exchange) => {
            assert!(exchange.observation.text().contains("edit ok"));
            assert!(exchange.observation.text().contains("could not be stored"));
        }
        other => panic!("a confirmed success continues, got {other:?}"),
    }
}

#[test]
fn protocol_parser_accepts_exactly_the_closed_world() {
    assert_eq!(
        parse_directive(r#"{"tool":"list","path":""}"#).unwrap(),
        TaskAgentDirective::Act(super::TaskAgentActionRequest {
            operation: OperationKind::List,
            path: String::new(),
            content: None,
        })
    );
    assert_eq!(
        parse_directive(r#"{"tool":"read","path":"a.txt"}"#).unwrap(),
        TaskAgentDirective::Act(super::TaskAgentActionRequest {
            operation: OperationKind::Read,
            path: String::from("a.txt"),
            content: None,
        })
    );
    assert_eq!(
        parse_directive(r#"{"tool":"create","path":"a.txt","content":"body"}"#).unwrap(),
        TaskAgentDirective::Act(super::TaskAgentActionRequest {
            operation: OperationKind::Create,
            path: String::from("a.txt"),
            content: Some(String::from("body")),
        })
    );
    assert_eq!(
        parse_directive(r#"{"tool":"edit","path":"a.txt","content":""}"#).unwrap(),
        TaskAgentDirective::Act(super::TaskAgentActionRequest {
            operation: OperationKind::Edit,
            path: String::from("a.txt"),
            content: Some(String::new()),
        })
    );
    assert_eq!(
        parse_directive("{\"final\":\"done\"}").unwrap(),
        TaskAgentDirective::Finish {
            body: String::from("done"),
        }
    );
    // Surrounding whitespace is tolerated; the body is not trimmed.
    assert_eq!(
        parse_directive("  {\"final\":\" padded \"} \n").unwrap(),
        TaskAgentDirective::Finish {
            body: String::from(" padded "),
        }
    );
}

#[test]
fn protocol_parser_refuses_everything_else() {
    use TaskAgentProtocolViolation as Violation;

    for (text, expected) in [
        ("not json", Violation::NotAJsonObject),
        ("[1,2,3]", Violation::NotAJsonObject),
        ("{}", Violation::MissingDirective),
        (r#"{"path":"a.txt"}"#, Violation::MissingDirective),
        (
            r#"{"tool":"read","path":"a.txt","final":"done"}"#,
            Violation::ConflictingDirective,
        ),
        (
            r#"{"final":"done","extra":1}"#,
            Violation::ConflictingDirective,
        ),
        (
            r#"{"tool":"read","path":"a.txt","extra":1}"#,
            Violation::ConflictingDirective,
        ),
        (
            r#"{"tool":"delete","path":"a.txt"}"#,
            Violation::UnknownTool,
        ),
        (r#"{"tool":"read"}"#, Violation::InvalidFields),
        (r#"{"tool":"read","path":7}"#, Violation::InvalidFields),
        (
            r#"{"tool":"create","path":"a.txt"}"#,
            Violation::InvalidFields,
        ),
        (
            r#"{"tool":"list","path":"","content":"x"}"#,
            Violation::ConflictingDirective,
        ),
        (r#"{"final":7}"#, Violation::InvalidFields),
    ] {
        assert_eq!(parse_directive(text), Err(expected), "input: {text}");
    }
}
