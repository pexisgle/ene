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

use ene_action::{ActionAttemptRepository as _, ActionCertainty};
use ene_credential::{
    CredentialIntentRepository as _, CredentialRef, CredentialScrubber, CredentialSetRevision,
    MemoryCredentialStore, RegistrationApply, RegistrationFingerprint, RegistrationState,
    ScrubbedText, SecretScrubError, SecretScrubber,
};
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
    DEFAULT_MAX_TURNS, TaskAgentRunOutcome, TaskAgentRunRefusal, TaskExecutionRegistry,
    run_task_agent_execution,
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

struct ScrubRefs(ene_credential::CredentialSetRevision);

impl ene_credential::CredentialRefRepository for ScrubRefs {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn list_refs(
        &self,
    ) -> Result<Vec<ene_credential::CredentialRef>, ene_credential::CredentialTechnicalError> {
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

impl SecretScrubber for MarkerScrubber {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        self.inputs
            .lock()
            .expect("scrub capture lock")
            .push(text.to_owned());
        Ok(scrub_fixture(
            &format!("[scrubbed] {text}"),
            CredentialSetRevision::initial(),
        )
        .await)
    }
}

/// The fixture Tasks have no adopted instructions, so no body is ever read.
struct NoInstructions;

impl TaskInstructionSource for NoInstructions {
    async fn load_owner_instruction(
        &self,
        _origin: TaskContextOrigin,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        Ok(None)
    }
}

async fn run<S: SecretScrubber>(
    fixture: &Fixture,
    inference: &ScriptedInference,
    scrubber: &S,
    max_turns: u32,
) -> Result<TaskAgentRunOutcome, super::TaskAgentRunError> {
    let registry = TaskExecutionRegistry::default();
    let registration = registry
        .register(fixture.delegation, fixture.task.task)
        .expect("the fixture delegation is not running");
    run_task_agent_execution(
        &fixture.store,
        &NoInstructions,
        inference,
        scrubber,
        max_turns,
        &registration,
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
        premises[2].prompt.text().contains("read ok:\nnotes"),
        "the file content is replayed as an observation, got {}",
        premises[2].prompt.text()
    );
    assert!(
        premises[2].prompt.text().contains("create ok"),
        "the create observation is replayed"
    );
    assert!(
        premise_exchange_count(&premises[1]) == 1 && premise_exchange_count(&premises[2]) == 2,
        "each turn replays exactly the prior exchanges"
    );
    assert!(
        !premises[0].prompt.text().contains("[TOOL RESULT]"),
        "the first turn has no transcript"
    );
    let scrubbed = scrubber.inputs();
    assert_eq!(
        scrubbed.len(),
        4,
        "each turn is scrubbed exactly once, and the final result body once more"
    );
    assert!(
        scrubbed[2].contains("read ok:\nnotes") && scrubbed[2].contains("[TOOL CALL]"),
        "the raw transcript is scrubbed as one string, got {}",
        scrubbed[2]
    );
}

/// Counts the replayed tool exchanges in one captured premise.
fn premise_exchange_count(premise: &TaskAgentInferencePremise) -> usize {
    premise.prompt.text().matches("[TOOL RESULT]").count()
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
        premises[1].prompt.text().contains("refused:"),
        "the refusal class is replayed, got {}",
        premises[1].prompt.text()
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

#[test]
fn concurrent_registrations_admit_exactly_one() {
    let task = ene_task::TaskId::generate();
    let delegation = ene_task::DelegationId::generate();
    let registry = TaskExecutionRegistry::default();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let after_attempt = std::sync::Arc::new(std::sync::Barrier::new(2));
    let admitted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    std::thread::scope(|scope| {
        for _ in 0..2 {
            let start = std::sync::Arc::clone(&start);
            let after_attempt = std::sync::Arc::clone(&after_attempt);
            let admitted = std::sync::Arc::clone(&admitted);
            let registry = &registry;
            scope.spawn(move || {
                start.wait();
                let registration = registry.register(delegation, task);
                if registration.is_some() {
                    admitted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                // The winner holds its slot until both threads have attempted
                // the registration, so the loser always races a live entry.
                after_attempt.wait();
                drop(registration);
            });
        }
    });
    assert_eq!(
        admitted.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly one of two racing registrations is admitted"
    );
}

/// Registers one usable pair through the production register-then-approve
/// path and provisions its bearer in the memory store.
async fn register_scrub_pair(
    store: &Store,
    values: &MemoryCredentialStore,
    provider: &str,
    label: &str,
    bearer: &str,
    intent_id: &str,
) {
    let applied = store
        .request_registration_with_intent(
            provider.to_owned(),
            label.to_owned(),
            RegistrationFingerprint {
                intent_id: intent_id.to_owned(),
                kind: String::from("register"),
                target: format!("credential:{provider}:{label}"),
                base: String::from("consent-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
        )
        .await;
    assert_eq!(
        applied,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "a fresh pair must pend Owner approval"
    );
    assert!(
        store
            .approve_credential_with_sweep(provider, label, bearer)
            .expect("the approval write must commit"),
        "the approval makes the pair usable"
    );
    values.insert(
        CredentialRef::new(provider, label).expect("valid fixture ref"),
        bearer,
    );
}

/// The deterministic race barrier for the final result commit: a real
/// credential scrubber whose final-answer scrub advances the credential set
/// (and provisions the new bearer) `rotate_every`-th time, so the stale
/// refusal and the re-scrub are exercised without any timing dependence.
struct RotateAfterFinalScrub<'a> {
    store: &'a Store,
    values: &'a MemoryCredentialStore,
    provider: &'a str,
    label: &'a str,
    rotated: &'a str,
    final_body: &'a str,
    rotate_every: bool,
    final_scrubs: Mutex<u32>,
}

impl RotateAfterFinalScrub<'_> {
    /// Advances the set with one approval; the sweep itself is production's.
    fn rotate(&self) {
        assert!(
            self.store
                .approve_credential_with_sweep(self.provider, self.label, self.rotated)
                .expect("the rotation approval must commit"),
            "the fixture pair is usable and the rotation applies"
        );
        self.values.insert(
            CredentialRef::new(self.provider, self.label).expect("valid fixture ref"),
            self.rotated,
        );
    }

    fn final_scrubs(&self) -> u32 {
        *self.final_scrubs.lock().expect("scrub count lock")
    }
}

impl SecretScrubber for RotateAfterFinalScrub<'_> {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        let proof = CredentialScrubber {
            refs: self.store,
            store: self.values,
        }
        .scrub(text)
        .await?;
        if text == self.final_body {
            let first = {
                let mut scrubs = self.final_scrubs.lock().expect("scrub count lock");
                *scrubs += 1;
                *scrubs == 1
            };
            if self.rotate_every || first {
                self.rotate();
            }
        }
        Ok(proof)
    }
}

/// The required race: the final answer is scrubbed under revision N, the
/// credential set advances to N+1 before the durable result commit, the
/// commit refuses the stale premise with zero writes, the execution re-scrubs
/// the original answer under N+1, and only the newly scrubbed result commits.
/// The raw value never lands durably and the diagnostics stay body-free.
#[tokio::test]
async fn a_rotation_between_result_scrub_and_commit_rescrubs_before_recording() {
    let secret = "sk-stage6-c3-race-secret-marker";
    let fixture = fixture().await;
    let values = MemoryCredentialStore::new();
    register_scrub_pair(
        &fixture.store,
        &values,
        "acme",
        "main",
        "sk-placeholder-value",
        "reg-c3-race",
    )
    .await;

    let answer = format!("the final report quotes {secret}");
    let inference = ScriptedInference::new(vec![&format!(r#"{{"final":"{answer}"}}"#)]);
    let scrubber = RotateAfterFinalScrub {
        store: &fixture.store,
        values: &values,
        provider: "acme",
        label: "main",
        rotated: secret,
        final_body: &answer,
        rotate_every: false,
        final_scrubs: Mutex::new(0),
    };

    let outcome = run(&fixture, &inference, &scrubber, DEFAULT_MAX_TURNS)
        .await
        .expect("the re-scrub must finalize");
    assert!(
        !format!("{outcome:?}").contains(secret),
        "the final outcome diagnostics carry no secret"
    );
    let result = match outcome {
        TaskAgentRunOutcome::Finalized { result, .. } => result,
        other => panic!("the execution must finalize, got {other:?}"),
    };
    assert_eq!(
        scrubber.final_scrubs(),
        2,
        "the stale body is dropped and the answer re-scrubbed exactly once"
    );
    assert_eq!(
        result.body.text(),
        format!(
            "the final report quotes {}",
            ene_credential::REDACTED_CREDENTIAL
        )
    );
    assert_eq!(
        fixture
            .store
            .count_exact_text_remainder_for_tests(secret)
            .await
            .expect("the remainder probe must answer"),
        0,
        "the raw value never lands durably"
    );
    assert!(
        fixture
            .store
            .load_delegation_result(fixture.delegation)
            .await
            .expect("the seal reads")
            .is_some(),
        "only the re-scrubbed result seals the execution"
    );
}
