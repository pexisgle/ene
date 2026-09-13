//! Task Agent inference turn boundary checks (IB §4 H-A / K-E).
//!
//! The Task repository and the inference port are faked so each test scripts
//! `load_delegation` / `load_task` replies and one port result, then inspects
//! the exact `TaskAgentInferencePremise` the harness composed. The checks pin
//! the precheck outcomes that must not reach the port (missing delegation,
//! missing Task, advanced revision, scrub failure), the purpose-text-only
//! logical input assembled from the relied `task_revision` snapshot through
//! the injected scrubber, the stale mapping that re-reads delegation before
//! task, and the outcome mapping: domain outcomes stay on the `Ok` side while
//! repository and port technical failures stay `Err`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
    DelegationScope, Task, TaskAgentEphemeralId, TaskAgentInference, TaskAgentInferenceError,
    TaskAgentInferenceOutcome, TaskAgentInferencePremise, TaskAgentInferenceProduced,
    TaskAgentNotSent, TaskAgentOutput, TaskAgentResultArrival, TaskAgentTurnError,
    TaskAgentTurnOutcome, TaskAgentTurnPremise, TaskCommitOutcome, TaskCommitPremise,
    TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskInstructionRole, TaskInstructionSource,
    TaskInstructionSourceError, TaskInstructionSourceRecord, TaskProgress, TaskPurpose,
    TaskPurposeRef, TaskRecord, TaskRef, TaskRepository, TaskResultAcceptance,
    TaskResultAdoptionClaim, TaskResultId, TaskResultRecord, TaskRevision, TaskRevisionRecord,
    TaskTechnicalError, orchestrate_task_agent_turn,
};

/// Instruction source for turns whose context carries no adopted instruction:
/// the harness must never read a source for a purpose-only input.
struct NoInstructionSource;

impl TaskInstructionSource for NoInstructionSource {
    async fn load_owner_instruction(
        &self,
        _source: RawId,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        panic!("a purpose-only context must not read an instruction source")
    }
}

/// Scripted instruction source: one reply per read plus the ordered capture
/// of every source identity asked for.
struct FakeInstructionSource {
    replies:
        Mutex<VecDeque<Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>>>,
    loaded: Mutex<Vec<RawId>>,
}

impl FakeInstructionSource {
    fn new() -> Self {
        Self {
            replies: Mutex::new(VecDeque::new()),
            loaded: Mutex::new(Vec::new()),
        }
    }

    fn script(
        &self,
        reply: Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>,
    ) {
        self.replies
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn loaded(&self) -> Vec<RawId> {
        self.loaded
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

impl TaskInstructionSource for FakeInstructionSource {
    async fn load_owner_instruction(
        &self,
        source: RawId,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        self.loaded
            .lock()
            .expect("fixture capture is never poisoned")
            .push(source);
        self.replies
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised source read has a scripted reply")
    }
}

/// Scripted `TaskRepository`: queued `load_delegation` / `load_task` /
/// seal replies plus a capture of every identity each read was asked for.
struct FakeTaskRepository {
    delegations: Mutex<VecDeque<Result<Option<DelegationRef>, TaskTechnicalError>>>,
    tasks: Mutex<VecDeque<Result<Option<TaskRecord>, TaskTechnicalError>>>,
    /// Scripted seal reads; an unscripted read answers `None` (not sealed),
    /// which is the state of every delegation that has not finalized.
    delegation_results: Mutex<VecDeque<Result<Option<TaskResultRecord>, TaskTechnicalError>>>,
    loaded_delegations: Mutex<Vec<DelegationId>>,
    loaded_tasks: Mutex<Vec<TaskId>>,
    loaded_delegation_results: Mutex<Vec<DelegationId>>,
}

impl FakeTaskRepository {
    fn new() -> Self {
        Self {
            delegations: Mutex::new(VecDeque::new()),
            tasks: Mutex::new(VecDeque::new()),
            delegation_results: Mutex::new(VecDeque::new()),
            loaded_delegations: Mutex::new(Vec::new()),
            loaded_tasks: Mutex::new(Vec::new()),
            loaded_delegation_results: Mutex::new(Vec::new()),
        }
    }

    fn loaded_delegation_results(&self) -> Vec<DelegationId> {
        self.loaded_delegation_results
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn script_delegation_result(
        &self,
        reply: Result<Option<TaskResultRecord>, TaskTechnicalError>,
    ) {
        self.delegation_results
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn script_delegation(&self, reply: Result<Option<DelegationRef>, TaskTechnicalError>) {
        self.delegations
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn script_task(&self, reply: Result<Option<TaskRecord>, TaskTechnicalError>) {
        self.tasks
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn loaded_delegations(&self) -> Vec<DelegationId> {
        self.loaded_delegations
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn loaded_tasks(&self) -> Vec<TaskId> {
        self.loaded_tasks
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

impl TaskRepository for FakeTaskRepository {
    async fn create_task(
        &self,
        _premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("create_task is outside this fixture's scope"),
        })
    }

    async fn forward_steering(
        &self,
        _premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("forward_steering is outside this fixture's scope"),
        })
    }

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        self.loaded_tasks
            .lock()
            .expect("fixture capture is never poisoned")
            .push(task);
        self.tasks
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised load_task call has a scripted reply")
    }

    async fn create_delegation(
        &self,
        _premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("create_delegation is outside this fixture's scope"),
        })
    }

    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        self.loaded_delegations
            .lock()
            .expect("fixture capture is never poisoned")
            .push(delegation);
        self.delegations
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised load_delegation call has a scripted reply")
    }

    async fn record_task_result_arrival(
        &self,
        _arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultRecord, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("record_task_result_arrival is outside this fixture's scope"),
        })
    }

    async fn load_task_result(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("load_task_result is outside this fixture's scope"),
        })
    }

    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        self.loaded_delegation_results
            .lock()
            .expect("fixture capture is never poisoned")
            .push(delegation);
        match self
            .delegation_results
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
        {
            Some(reply) => reply,
            None => Ok(None),
        }
    }

    async fn adopt_result(
        &self,
        _claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("adopt_result is outside this fixture's scope"),
        })
    }
}

/// Recording inference port: one scripted reply, the premises it received.
struct ScriptedInference {
    replies: Mutex<VecDeque<Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>>>,
    premises: Mutex<Vec<TaskAgentInferencePremise>>,
}

impl ScriptedInference {
    fn new(reply: Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>) -> Self {
        Self {
            replies: Mutex::new(VecDeque::from([reply])),
            premises: Mutex::new(Vec::new()),
        }
    }

    fn premises(&self) -> Vec<TaskAgentInferencePremise> {
        self.premises
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the port contract"
)]
impl TaskAgentInference for ScriptedInference {
    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
        self.premises
            .lock()
            .expect("fixture capture is never poisoned")
            .push(premise);
        self.replies
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised infer call has a scripted reply")
    }
}

/// The scrubber-side script for one `scrub` call.
enum FakeScrubReply {
    Scrubbed,
    Failed(SecretScrubError),
}

/// Recording scrubber: returns a marked copy of its input (so a test can
/// prove the port received the scrubber's output, not the raw text) or the
/// scripted failure, and records every input.
struct FakeScrubber {
    replies: Mutex<VecDeque<FakeScrubReply>>,
    inputs: Mutex<Vec<String>>,
}

impl FakeScrubber {
    fn new(reply: FakeScrubReply) -> Self {
        Self {
            replies: Mutex::new(VecDeque::from([reply])),
            inputs: Mutex::new(Vec::new()),
        }
    }

    fn inputs(&self) -> Vec<String> {
        self.inputs
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the scrubber contract"
)]
impl SecretScrubber for FakeScrubber {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        self.inputs
            .lock()
            .expect("fixture capture is never poisoned")
            .push(text.to_owned());
        match self
            .replies
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised scrub call has a scripted reply")
        {
            FakeScrubReply::Scrubbed => Ok(ScrubbedText {
                text: format!("[scrubbed] {text}"),
                credential_set: CredentialSetRevision::from_u64(7),
            }),
            FakeScrubReply::Failed(error) => Err(error),
        }
    }
}

fn revision(value: u64) -> TaskRevision {
    TaskRevision::from_u64(value)
}

fn reference(task: TaskId, value: u64) -> TaskRef {
    TaskRef {
        task,
        revision: revision(value),
    }
}

fn delegation(task: TaskRef) -> DelegationRef {
    DelegationRef {
        delegation: DelegationId::generate(),
        task,
        delegator: AssigneeRef {
            companion: RawId::new(),
        },
        agent: TaskAgentEphemeralId::generate(),
        scope: DelegationScope { workspace: None },
    }
}

fn record(task: TaskId, current_revision: TaskRevision, purpose_text: &str) -> TaskRecord {
    let reference = reference(task, current_revision.as_u64());
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: current_revision,
    };
    let assignee = AssigneeRef {
        companion: RawId::new(),
    };
    TaskRecord {
        task: Task {
            reference,
            purpose,
            assignee,
            progress: TaskProgress::InProgress,
            adopted_result: None,
        },
        revision: TaskRevisionRecord {
            reference,
            purpose,
            purpose_text: TaskPurpose {
                text: purpose_text.to_owned(),
            },
            assignee,
        },
        context: vec![TaskContextEntry {
            entry: TaskContextEntryId::generate(),
            reference,
            item: TaskContextItem::AdoptedPurpose(purpose),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
        }],
        workspace: None,
    }
}

fn premise(delegation: DelegationId) -> TaskAgentTurnPremise {
    TaskAgentTurnPremise { delegation }
}

fn instruction_entry(
    reference: TaskRef,
    source: RawId,
    kind: TaskContextOriginKind,
) -> TaskContextEntry {
    TaskContextEntry {
        entry: TaskContextEntryId::generate(),
        reference,
        item: TaskContextItem::AdoptedInstruction,
        origin: TaskContextOrigin { kind, source },
        acquired_at: WallClockWithTz::now(),
    }
}

fn source_record(
    source: RawId,
    companion: RawId,
    role: TaskInstructionRole,
    text: &str,
) -> TaskInstructionSourceRecord {
    TaskInstructionSourceRecord {
        source,
        companion,
        role,
        text: text.to_owned(),
    }
}

/// Loads one delegation and one record, returning the capture handles the
/// instruction tests inspect.
struct TurnFixture {
    repository: FakeTaskRepository,
    instructions: FakeInstructionSource,
    inference: ScriptedInference,
    scrubber: FakeScrubber,
    delegation: DelegationId,
    task: TaskId,
}

fn turn_fixture(loaded: TaskRecord, delegation_ref: DelegationRef) -> TurnFixture {
    let task = delegation_ref.task.task;
    let delegation = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(loaded)));
    TurnFixture {
        repository,
        instructions: FakeInstructionSource::new(),
        inference: ScriptedInference::new(produced_reply()),
        scrubber: FakeScrubber::new(FakeScrubReply::Scrubbed),
        delegation,
        task,
    }
}

fn produced_reply() -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
    Ok(TaskAgentInferenceOutcome::Produced {
        output: TaskAgentOutput::new(String::from("probe provider output")),
        adoption_consent_current: true,
    })
}

#[tokio::test]
async fn produced_turn_carries_the_scrubbed_purpose_prompt_and_the_output() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let purpose_text = "probe adopted purpose text";
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    let loaded = record(task, revision(1), purpose_text);
    let purpose_source = loaded.context[0].origin.source;
    repository.script_task(Ok(Some(loaded)));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::Produced {
        output: TaskAgentOutput::new(String::from("probe provider output")),
        adoption_consent_current: true,
    }));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a produced turn is a domain outcome, not a technical error");

    let captured = inference.premises();
    assert_eq!(captured.len(), 1, "one turn runs exactly one inference");
    let received = captured.first().expect("the premise was captured");
    assert_eq!(received.delegation, delegation_id);
    assert_eq!(received.task, relied, "the port gets the relied revision");
    assert_eq!(
        received.prompt.text,
        format!("[scrubbed] [PURPOSE]\n{purpose_text}"),
        "the port gets the scrubber's output, not the raw purpose text"
    );
    assert_eq!(
        received.prompt.credential_set,
        CredentialSetRevision::from_u64(7),
        "the scrub premise crosses unchanged"
    );
    assert_eq!(
        received.data_use,
        vec![purpose_source],
        "the logical input's canonical source is the in-force purpose entry's origin"
    );
    assert_eq!(
        scrubber.inputs(),
        vec![format!("[PURPOSE]\n{purpose_text}")],
        "the scrubber is asked to scrub exactly the relied snapshot's purpose text in the fixed framing"
    );
    assert_eq!(repository.loaded_delegations(), vec![delegation_id]);
    assert_eq!(repository.loaded_tasks(), vec![task]);

    match outcome {
        TaskAgentTurnOutcome::Produced(produced) => {
            assert_eq!(produced.delegation, delegation_id);
            assert_eq!(produced.task, relied);
            assert_eq!(produced.output.text(), "probe provider output");
            assert!(
                produced.adoption_consent_current,
                "the consent flag maps through unchanged"
            );
        }
        other => panic!("expected Produced, got {other:?}"),
    }
}

#[tokio::test]
async fn advanced_revision_is_stale_before_any_inference() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let current = reference(task, 2);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(2), "probe purpose"))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::Produced {
        output: TaskAgentOutput::new(String::from("never produced")),
        adoption_consent_current: true,
    }));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a stale revision is a domain outcome, not a technical error");

    assert_eq!(outcome, TaskAgentTurnOutcome::StaleTaskRevision { current });
    assert!(
        inference.premises().is_empty(),
        "an advanced revision must not reach the port"
    );
    assert!(
        scrubber.inputs().is_empty(),
        "an advanced revision must not even be scrubbed"
    );
}

#[tokio::test]
async fn port_stale_premise_with_a_forwarded_revision_maps_to_stale_with_current() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let current = reference(task, 2);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let purpose_text = "probe adopted purpose text";
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref.clone())));
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), purpose_text))));
    repository.script_task(Ok(Some(record(task, revision(2), purpose_text))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a lost claim is a domain outcome, not a technical error");

    assert_eq!(outcome, TaskAgentTurnOutcome::StaleTaskRevision { current });
    assert_eq!(
        inference.premises().len(),
        1,
        "the stale signal comes from the port, not a retry"
    );
    assert_eq!(
        repository.loaded_delegations(),
        vec![delegation_id, delegation_id],
        "the stale mapping re-reads the delegation before the task"
    );
    assert_eq!(repository.loaded_tasks(), vec![task, task]);
}

#[tokio::test]
async fn port_stale_premise_with_a_gone_delegation_maps_to_missing_delegation() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_delegation(Ok(None));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a gone delegation is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::MissingDelegation {
            delegation: delegation_id
        }
    );
    assert_eq!(inference.premises().len(), 1);
    assert_eq!(
        repository.loaded_tasks(),
        vec![task],
        "a gone delegation on reload is reported without any task re-read"
    );
}

fn result_record(task: TaskRef, delegation: DelegationId) -> TaskResultRecord {
    TaskResultRecord {
        result: TaskResultId::generate(),
        task,
        delegation,
        body: TaskAgentOutput::new(String::from("final body")),
        attempt_refs: Vec::new(),
        adopted_revision: None,
        recorded_at: WallClockWithTz::now(),
    }
}

#[tokio::test]
async fn terminal_task_is_reported_before_any_scrub_or_port_call() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    let mut loaded = record(task, revision(1), "probe purpose");
    loaded.task.progress = TaskProgress::Completed;
    repository.script_task(Ok(Some(loaded)));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("terminal progress is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::TaskTerminal {
            task,
            progress: TaskProgress::Completed,
        }
    );
    assert!(
        scrubber.inputs().is_empty(),
        "a terminal Task must not even be scrubbed"
    );
    assert!(
        inference.premises().is_empty(),
        "a terminal Task must not reach the port"
    );
}

#[tokio::test]
async fn port_stale_premise_with_terminal_progress_maps_to_task_terminal() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref.clone())));
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let mut terminal = record(task, revision(1), "probe purpose");
    terminal.task.progress = TaskProgress::Failed;
    repository.script_task(Ok(Some(terminal)));
    // A seal that exists at the same time must not win over terminal.
    repository.script_delegation_result(Ok(Some(result_record(relied, delegation_id))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a lost claim is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::TaskTerminal {
            task,
            progress: TaskProgress::Failed,
        }
    );
    assert!(
        repository.loaded_delegation_results().is_empty(),
        "terminal progress is checked before the execution seal"
    );
}

#[tokio::test]
async fn port_stale_premise_with_a_sealed_delegation_maps_to_execution_sealed() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref.clone())));
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    repository.script_delegation_result(Ok(Some(result_record(relied, delegation_id))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a sealed execution is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::ExecutionSealed {
            delegation: delegation_id,
        },
        "the seal is reported instead of revision staleness while the Task is InProgress"
    );
    assert_eq!(
        repository.loaded_delegation_results(),
        vec![delegation_id],
        "the stale re-read uses the bounded seal read"
    );
}

#[tokio::test]
async fn missing_delegation_is_reported_without_reading_the_task_or_inferring() {
    let delegation_id = DelegationId::generate();
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(None));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a missing delegation is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::MissingDelegation {
            delegation: delegation_id
        }
    );
    assert!(inference.premises().is_empty());
    assert!(scrubber.inputs().is_empty());
    assert!(
        repository.loaded_tasks().is_empty(),
        "a missing delegation must not trigger a task read"
    );
}

#[tokio::test]
async fn missing_task_is_reported_without_inferring() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(None));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a missing Task is a domain outcome, not a technical error");

    assert_eq!(outcome, TaskAgentTurnOutcome::MissingTask { task });
    assert!(inference.premises().is_empty());
    assert!(scrubber.inputs().is_empty());
}

#[tokio::test]
async fn scrub_failure_fails_closed_without_sending_or_leaking_the_purpose() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let purpose_text = "probe purpose text that must never leak";
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), purpose_text))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Failed(
        SecretScrubError::RegistryUnavailable,
    ));

    let error = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect_err("a scrub failure is a technical error");

    match &error {
        TaskAgentTurnError::InputUnavailable { reason } => {
            assert_eq!(reason, "credential scrub failed");
        }
        other => panic!("expected InputUnavailable, got {other:?}"),
    }
    assert_eq!(
        scrubber.inputs(),
        vec![format!("[PURPOSE]\n{purpose_text}")],
        "the scrub was attempted on the relied purpose text in the fixed framing"
    );
    assert!(
        inference.premises().is_empty(),
        "a scrub failure must fail closed before the port"
    );
    let rendered = error.to_string();
    assert!(
        !rendered.contains(purpose_text),
        "the failure must not carry the purpose text: {rendered}"
    );
    assert!(
        !format!("{error:?}").contains(purpose_text),
        "the failure must not carry the purpose text in Debug"
    );
}

#[tokio::test]
async fn every_not_sent_reason_maps_through_unchanged() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let reasons = [
        TaskAgentNotSent::SetupIncomplete,
        TaskAgentNotSent::NotInAllowlist,
        TaskAgentNotSent::ConsentStale,
        TaskAgentNotSent::OverLimit,
        TaskAgentNotSent::EvaluationConsumed,
    ];

    for reason in reasons {
        let delegation_ref = delegation(relied);
        let delegation_id = delegation_ref.delegation;
        let repository = FakeTaskRepository::new();
        repository.script_delegation(Ok(Some(delegation_ref)));
        repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
        let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::NotSent(reason)));
        let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

        let outcome = orchestrate_task_agent_turn(
            &repository,
            &NoInstructionSource,
            &inference,
            &scrubber,
            premise(delegation_id),
        )
        .await
        .expect("a refused use is a domain outcome, not a technical error");

        assert_eq!(outcome, TaskAgentTurnOutcome::NotSent(reason));
        assert_eq!(inference.premises().len(), 1);
    }
}

#[tokio::test]
async fn technical_failures_stay_errors() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;

    let repository = FakeTaskRepository::new();
    repository.script_delegation(Err(TaskTechnicalError::StorageUnavailable {
        reason: String::from("load_delegation unavailable"),
    }));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);
    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await;
    assert_eq!(
        outcome,
        Err(TaskAgentTurnError::StorageUnavailable {
            reason: String::from("load_delegation unavailable"),
        }),
        "a repository failure is not a missing delegation"
    );
    assert!(inference.premises().is_empty());

    let task_error = TaskTechnicalError::StorageUnavailable {
        reason: String::from("load_task unavailable"),
    };
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref.clone())));
    repository.script_task(Err(task_error));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::StaleTaskPremise));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);
    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await;
    assert_eq!(
        outcome,
        Err(TaskAgentTurnError::StorageUnavailable {
            reason: String::from("load_task unavailable"),
        }),
        "a repository failure is not a missing Task"
    );
    assert!(scrubber.inputs().is_empty());

    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let inference = ScriptedInference::new(Err(TaskAgentInferenceError::InferenceUnavailable {
        reason: String::from("provider transport failed"),
    }));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);
    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await;
    match outcome {
        Err(TaskAgentTurnError::InferenceUnavailable { reason }) => {
            assert_eq!(reason, "provider transport failed");
        }
        other => panic!("expected InferenceUnavailable, got {other:?}"),
    }
    assert_eq!(inference.premises().len(), 1);
}

#[test]
fn debug_redacts_prompt_and_output_text() {
    let probe = "probe redaction text";
    let inference_premise = TaskAgentInferencePremise {
        delegation: DelegationId::generate(),
        task: reference(TaskId::generate(), 1),
        prompt: ScrubbedText {
            text: probe.to_owned(),
            credential_set: CredentialSetRevision::initial(),
        },
        data_use: vec![RawId::new()],
    };
    assert!(
        !format!("{inference_premise:?}").contains(probe),
        "the inference premise Debug must redact the prompt"
    );

    let output = TaskAgentOutput::new(probe.to_owned());
    assert!(
        !format!("{output:?}").contains(probe),
        "the output Debug must redact the text"
    );

    let outcome = TaskAgentInferenceOutcome::Produced {
        output: output.clone(),
        adoption_consent_current: true,
    };
    assert!(
        !format!("{outcome:?}").contains(probe),
        "the port outcome Debug must not leak the output"
    );

    let produced = TaskAgentTurnOutcome::Produced(TaskAgentInferenceProduced {
        delegation: DelegationId::generate(),
        task: reference(TaskId::generate(), 1),
        output,
        adoption_consent_current: true,
    });
    assert!(
        !format!("{produced:?}").contains(probe),
        "the turn outcome Debug must not leak the output"
    );
}

#[tokio::test]
async fn instruction_bodies_are_resolved_in_context_order_and_scrubbed_once() {
    let purpose_text = "probe adopted purpose";
    let loaded = record(TaskId::generate(), revision(2), purpose_text);
    let relied = loaded.task.reference;
    let companion = loaded.task.assignee.companion;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let purpose_source = loaded.context[0].origin.source;
    // Context order is the logical-input order: E1 then E2.
    let first = RawId::new();
    let second = RawId::new();
    loaded.context.push(instruction_entry(
        relied,
        first,
        TaskContextOriginKind::OwnerConversation,
    ));
    loaded.context.push(instruction_entry(
        relied,
        second,
        TaskContextOriginKind::OwnerConversation,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture.instructions.script(Ok(Some(source_record(
        first,
        companion,
        TaskInstructionRole::Owner,
        "first instruction",
    ))));
    fixture.instructions.script(Ok(Some(source_record(
        second,
        companion,
        TaskInstructionRole::Owner,
        "second instruction",
    ))));

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise(fixture.delegation),
    )
    .await
    .expect("a produced turn is a domain outcome");
    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));

    let raw_input = format!(
        "[PURPOSE]\n{purpose_text}\n[INSTRUCTION]\nfirst instruction\n[INSTRUCTION]\nsecond instruction"
    );
    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert_eq!(
        received.prompt.text,
        format!("[scrubbed] {raw_input}"),
        "the port receives the single scrub of the whole framed input"
    );
    assert_eq!(
        received.data_use,
        vec![purpose_source, first, second],
        "the correlation follows purpose then instruction order"
    );
    assert_eq!(
        fixture.scrubber.inputs(),
        vec![raw_input.clone()],
        "purpose and every resolved body are scrubbed exactly once, together"
    );
    assert_eq!(
        fixture.instructions.loaded(),
        vec![first, second],
        "sources are resolved in context order without dedupe"
    );
    assert_eq!(
        fixture.repository.loaded_tasks(),
        vec![fixture.task],
        "no Task re-read is inserted between assembly and the claim"
    );
    assert_eq!(
        fixture.repository.loaded_delegations(),
        vec![fixture.delegation]
    );
}

#[tokio::test]
async fn duplicate_source_adoption_keeps_both_bodies_and_correlations() {
    let loaded = record(TaskId::generate(), revision(2), "probe purpose");
    let relied = loaded.task.reference;
    let companion = loaded.task.assignee.companion;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let repeated = RawId::new();
    loaded.context.push(instruction_entry(
        relied,
        repeated,
        TaskContextOriginKind::OwnerConversation,
    ));
    loaded.context.push(instruction_entry(
        relied,
        repeated,
        TaskContextOriginKind::OwnerConversation,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    for _ in 0..2 {
        fixture.instructions.script(Ok(Some(source_record(
            repeated,
            companion,
            TaskInstructionRole::Owner,
            "repeat me",
        ))));
    }

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise(fixture.delegation),
    )
    .await
    .expect("a produced turn is a domain outcome");
    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));

    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert_eq!(
        received.prompt.text.matches("repeat me").count(),
        2,
        "a repeated adoption repeats the body instead of deduplicating"
    );
    let purpose_source = received.data_use[0];
    assert_eq!(
        received.data_use,
        vec![purpose_source, repeated, repeated],
        "entry-level correlation duplicates are preserved"
    );
    assert_eq!(fixture.instructions.loaded(), vec![repeated, repeated]);
}

#[tokio::test]
async fn missing_instruction_source_ends_the_turn_without_scrub_or_send() {
    let loaded = record(TaskId::generate(), revision(2), "probe purpose");
    let relied = loaded.task.reference;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let missing = RawId::new();
    let entry = instruction_entry(relied, missing, TaskContextOriginKind::OwnerConversation);
    let entry_id = entry.entry;
    loaded.context.push(entry);
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture.instructions.script(Ok(None));

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise(fixture.delegation),
    )
    .await
    .expect("a missing source is a domain outcome, not a technical error");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::InstructionSourceMissing {
            entry: entry_id,
            source: missing,
        },
        "the exact adopted entry and unresolved source are reported"
    );
    assert!(
        fixture.scrubber.inputs().is_empty(),
        "a missing instruction body is never scrubbed as if it existed"
    );
    assert!(
        fixture.inference.premises().is_empty(),
        "a missing instruction body never reaches the inference port"
    );
}

#[tokio::test]
async fn instruction_correspondence_mismatches_fail_closed() {
    type RecordBuilder = fn(RawId, RawId) -> TaskInstructionSourceRecord;
    // Each case violates exactly one correspondence condition: the loaded
    // source identity differs, the companion differs, or the role differs.
    let cases: [(&str, RecordBuilder); 3] = [
        ("wrong source identity", |_source, companion| {
            source_record(RawId::new(), companion, TaskInstructionRole::Owner, "body")
        }),
        ("foreign companion", |source, _companion| {
            source_record(source, RawId::new(), TaskInstructionRole::Owner, "body")
        }),
        ("wrong role", |source, companion| {
            source_record(source, companion, TaskInstructionRole::Companion, "body")
        }),
    ];
    for (name, build) in cases {
        let loaded = record(TaskId::generate(), revision(2), "probe purpose");
        let relied = loaded.task.reference;
        let companion = loaded.task.assignee.companion;
        let delegation_ref = delegation(relied);
        let mut loaded = loaded;
        let source = RawId::new();
        loaded.context.push(instruction_entry(
            relied,
            source,
            TaskContextOriginKind::OwnerConversation,
        ));
        let fixture = turn_fixture(loaded, delegation_ref);
        fixture
            .instructions
            .script(Ok(Some(build(source, companion))));

        let outcome = orchestrate_task_agent_turn(
            &fixture.repository,
            &fixture.instructions,
            &fixture.inference,
            &fixture.scrubber,
            premise(fixture.delegation),
        )
        .await;
        match outcome {
            Err(TaskAgentTurnError::InputUnavailable { reason }) => {
                assert_eq!(
                    reason, "instruction source correspondence mismatch",
                    "{name} must fail as a fixed body-free class"
                );
            }
            other => panic!("{name} must fail closed, got {other:?}"),
        }
        assert!(
            fixture.inference.premises().is_empty(),
            "{name} never reaches the port"
        );
        assert!(
            fixture.scrubber.inputs().is_empty(),
            "{name} is rejected before the scrub"
        );
    }
}

#[tokio::test]
async fn unsupported_instruction_origin_fails_closed_without_reading_a_body() {
    let loaded = record(TaskId::generate(), revision(2), "probe purpose");
    let relied = loaded.task.reference;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    loaded.context.push(instruction_entry(
        relied,
        RawId::new(),
        TaskContextOriginKind::Spontaneous,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise(fixture.delegation),
    )
    .await;
    match outcome {
        Err(TaskAgentTurnError::InputUnavailable { reason }) => {
            assert_eq!(reason, "unsupported instruction origin kind");
        }
        other => panic!("an unsupported origin kind must fail closed, got {other:?}"),
    }
    assert!(
        fixture.instructions.loaded().is_empty(),
        "no body producer exists for this origin kind, so none is read"
    );
    assert!(fixture.inference.premises().is_empty());
}

#[tokio::test]
async fn instruction_source_read_failure_fails_closed_without_leaking_the_reason() {
    let loaded = record(TaskId::generate(), revision(2), "probe purpose");
    let relied = loaded.task.reference;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    loaded.context.push(instruction_entry(
        relied,
        RawId::new(),
        TaskContextOriginKind::OwnerConversation,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture
        .instructions
        .script(Err(TaskInstructionSourceError::SourceUnavailable {
            reason: String::from("backend detail probe"),
        }));

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise(fixture.delegation),
    )
    .await;
    match outcome {
        Err(TaskAgentTurnError::InputUnavailable { reason }) => {
            assert_eq!(reason, "instruction source read failed");
            assert!(!reason.contains("backend detail probe"));
        }
        other => panic!("a source read failure must fail closed, got {other:?}"),
    }
    assert!(fixture.inference.premises().is_empty());
    assert!(fixture.scrubber.inputs().is_empty());
}

#[tokio::test]
async fn instruction_scrub_failure_fails_closed_without_leaking_the_input() {
    let probe = "probe instruction secret body";
    let loaded = record(TaskId::generate(), revision(2), "probe purpose");
    let relied = loaded.task.reference;
    let companion = loaded.task.assignee.companion;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let source = RawId::new();
    loaded.context.push(instruction_entry(
        relied,
        source,
        TaskContextOriginKind::OwnerConversation,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture.instructions.script(Ok(Some(source_record(
        source,
        companion,
        TaskInstructionRole::Owner,
        probe,
    ))));
    let scrubber = FakeScrubber::new(FakeScrubReply::Failed(
        SecretScrubError::RegistryUnavailable,
    ));

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &scrubber,
        premise(fixture.delegation),
    )
    .await;
    match outcome {
        Err(TaskAgentTurnError::InputUnavailable { reason }) => {
            assert_eq!(reason, "credential scrub failed");
        }
        other => panic!("a scrub failure must fail closed, got {other:?}"),
    }
    assert!(
        scrubber.inputs().iter().any(|input| input.contains(probe)),
        "the whole framed input, instruction body included, was the scrub target"
    );
    assert!(
        fixture.inference.premises().is_empty(),
        "a failed scrub never reaches the port"
    );
}
