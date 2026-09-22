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

mod fixture;

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
    DelegationScope, Task, TaskAgentActionExchange, TaskAgentEphemeralId, TaskAgentInference,
    TaskAgentInferenceError, TaskAgentInferenceOutcome, TaskAgentInferencePremise,
    TaskAgentInferenceProduced, TaskAgentNotSent, TaskAgentObservation, TaskAgentObservationId,
    TaskAgentOutput, TaskAgentResultArrival, TaskAgentTurnError, TaskAgentTurnOutcome,
    TaskAgentTurnPremise, TaskCancelOutcome, TaskCommitOutcome, TaskCommitPremise,
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
        _origin: TaskContextOrigin,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        panic!("a purpose-only context must not read an instruction source")
    }
}

/// Scripted instruction source: one reply per read plus the ordered capture
/// of every source identity asked for.
struct FakeInstructionSource {
    replies:
        Mutex<VecDeque<Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>>>,
    loaded: Mutex<Vec<TaskContextOrigin>>,
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

    fn loaded(&self) -> Vec<TaskContextOrigin> {
        self.loaded
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn loaded_sources(&self) -> Vec<RawId> {
        self.loaded().iter().map(|origin| origin.source).collect()
    }
}

impl TaskInstructionSource for FakeInstructionSource {
    async fn load_owner_instruction(
        &self,
        origin: TaskContextOrigin,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
        self.loaded
            .lock()
            .expect("fixture capture is never poisoned")
            .push(origin);
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
    past_facts: Mutex<VecDeque<Result<ene_task::PastExecutedFactsPage, TaskTechnicalError>>>,
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
            past_facts: Mutex::new(VecDeque::new()),
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

    fn script_past_facts(
        &self,
        reply: Result<ene_task::PastExecutedFactsPage, TaskTechnicalError>,
    ) {
        self.past_facts
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
    async fn record_task_agent_observation(
        &self,
        _premise: ene_task::TaskAgentObservationPremise,
    ) -> Result<ene_task::TaskAgentObservationId, ene_task::TaskTechnicalError> {
        Err(fixture::unsupported("record_task_agent_observation"))
    }
    async fn create_task(
        &self,
        _premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        Err(fixture::unsupported("create_task"))
    }

    async fn forward_steering(
        &self,
        _premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("forward_steering"))
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

    async fn cancel_task(&self, _task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("cancel_task"))
    }

    async fn create_delegation(
        &self,
        _premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("create_delegation"))
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
    ) -> Result<ene_task::TaskResultArrivalOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("record_task_result_arrival"))
    }

    async fn load_task_result(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Err(fixture::unsupported("load_task_result"))
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

    async fn delegation_has_started_work(
        &self,
        _delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        Ok(false)
    }

    async fn adopt_result(
        &self,
        _claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        Err(fixture::unsupported("adopt_result"))
    }

    async fn fail_task(
        &self,
        _premise: ene_task::TaskFailurePremise,
    ) -> Result<ene_task::TaskFailureOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("fail_task"))
    }

    async fn load_result_adoption_claim(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        Err(fixture::unsupported("load_result_adoption_claim"))
    }

    async fn list_unadopted_results_after(
        &self,
        _after: Option<ene_task::UnadoptedResultCursor>,
        _limit: u64,
    ) -> Result<Vec<ene_task::UnadoptedResultCursor>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn load_task_action_attempts(
        &self,
        _task: TaskId,
    ) -> Result<Vec<ene_primitive::RawId>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn list_tasks_after(
        &self,
        _after: Option<TaskId>,
        _limit: u32,
    ) -> Result<Vec<ene_task::TaskHeadline>, TaskTechnicalError> {
        Err(fixture::unsupported("list_tasks_after"))
    }

    async fn list_task_report_rows_after(
        &self,
        _task: TaskId,
        _after: Option<ene_task::TaskReportRowCursor>,
        _limit: u32,
    ) -> Result<Vec<ene_task::TaskReportRow>, TaskTechnicalError> {
        Err(fixture::unsupported("list_task_report_rows_after"))
    }

    async fn load_report_source_bounded(
        &self,
        _source: ene_task::TaskReportSourceRef,
        _cursor_bytes: u64,
        _limit_bytes: u32,
    ) -> Result<Option<ene_task::TaskReportSourcePage>, TaskTechnicalError> {
        Err(fixture::unsupported("load_report_source_bounded"))
    }

    async fn commit_task_resume(
        &self,
        _premise: ene_task::TaskResumeCommitPremise,
    ) -> Result<ene_task::TaskResumeOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("commit_task_resume"))
    }

    async fn load_past_executed_facts(
        &self,
        _task: TaskId,
    ) -> Result<ene_task::PastExecutedFactsPage, TaskTechnicalError> {
        match self
            .past_facts
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
        {
            Some(reply) => reply,
            None => Ok(ene_task::PastExecutedFactsPage {
                facts: Vec::new(),
                has_more: false,
            }),
        }
    }
}

/// Recording inference port: one scripted reply, the premises it received.
struct ScriptedInference {
    replies: Mutex<VecDeque<Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>>>,
    premises: Mutex<Vec<TaskAgentInferencePremise>>,
    budget: usize,
}

impl ScriptedInference {
    fn new(reply: Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>) -> Self {
        Self::with_budget(reply, usize::MAX)
    }

    /// Scripts one reply under an explicit input budget.
    fn with_budget(
        reply: Result<TaskAgentInferenceOutcome, TaskAgentInferenceError>,
        budget: usize,
    ) -> Self {
        Self {
            replies: Mutex::new(VecDeque::from([reply])),
            premises: Mutex::new(Vec::new()),
            budget,
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
    fn input_budget(&self) -> usize {
        self.budget
    }

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

impl SecretScrubber for FakeScrubber {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        self.inputs
            .lock()
            .expect("fixture capture is never poisoned")
            .push(text.to_owned());
        let reply = {
            let mut replies = self
                .replies
                .lock()
                .expect("fixture script is never poisoned");
            replies.pop_front()
        }
        .expect("every exercised scrub call has a scripted reply");
        match reply {
            FakeScrubReply::Scrubbed => Ok(scrub_fixture(
                &format!("[scrubbed] {text}"),
                CredentialSetRevision::from_u64(7),
            )
            .await),
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
    TaskAgentTurnPremise {
        delegation,
        exchanges: Vec::new(),
    }
}

/// The fixed response-format preamble every Task Agent turn starts with.
///
/// Pinned here as the provider-visible contract the Host tool-loop parser is
/// built against; a change to the harness framing must update this constant
/// and the Host parser together.
const RESPONSE_FORMAT: &str = "[RESPONSE FORMAT]\n\
Respond with exactly one JSON object and no other text. One of:\n\
{\"tool\":\"list\",\"path\":\"<workspace-relative directory>\"}\n\
{\"tool\":\"read\",\"path\":\"<workspace-relative file>\"}\n\
{\"tool\":\"create\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
{\"tool\":\"edit\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
{\"final\":\"<final answer>\"}\n";

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
    source_record_with_kind(
        TaskContextOriginKind::OwnerConversation,
        source,
        companion,
        role,
        text,
    )
}

fn source_record_with_kind(
    kind: TaskContextOriginKind,
    source: RawId,
    companion: RawId,
    role: TaskInstructionRole,
    text: &str,
) -> TaskInstructionSourceRecord {
    TaskInstructionSourceRecord {
        kind,
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
        received.prompt.text(),
        format!("[scrubbed] {RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[PAST EXECUTED FACTS]\n"),
        "the port gets the scrubber's output, not the raw purpose text"
    );
    assert_eq!(
        received.prompt.credential_set(),
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
        vec![format!(
            "{RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[PAST EXECUTED FACTS]\n"
        )],
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
async fn an_over_budget_transcript_drops_oldest_exchanges_with_a_fixed_note() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let inference = ScriptedInference::with_budget(produced_reply(), 800);
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let old = TaskAgentActionExchange {
        request: TaskAgentOutput::new(String::from("{\"tool\":\"read\",\"path\":\"old.txt\"}")),
        observation: TaskAgentObservation::new(
            TaskAgentObservationId::generate(),
            format!("read ok:\n{}", "o".repeat(400)),
        ),
    };
    let newest = TaskAgentActionExchange {
        request: TaskAgentOutput::new(String::from("{\"tool\":\"read\",\"path\":\"new.txt\"}")),
        observation: TaskAgentObservation::new(
            TaskAgentObservationId::generate(),
            format!("read ok:\n{}", "n".repeat(100)),
        ),
    };
    let newest_occurrence = newest.observation.occurrence();
    let old_occurrence = old.observation.occurrence();
    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        TaskAgentTurnPremise {
            delegation: delegation_id,
            exchanges: vec![old, newest],
        },
    )
    .await
    .expect("a trimmed transcript is still a domain outcome");

    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));
    let captured = inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert!(
        received.data_use.contains(&newest_occurrence.as_raw()),
        "a kept observation occurrence joins the turn correlation"
    );
    assert!(
        !received.data_use.contains(&old_occurrence.as_raw()),
        "a dropped exchange is not in the logical input and adds no correlation"
    );
    let raw = scrubber.inputs();
    assert_eq!(raw.len(), 1);
    assert!(
        raw[0].chars().count() <= 800,
        "the assembled input stays within the port budget, got {}",
        raw[0].chars().count()
    );
    assert!(
        raw[0].contains("[NOTE] earlier tool exchanges were omitted to fit the input bound"),
        "the omission is explicit to the model, got {}",
        raw[0]
    );
    assert!(
        raw[0].contains(&format!("read ok:\n{}", "n".repeat(100))),
        "the newest exchange is kept whole, observation included"
    );
    assert!(
        !raw[0].contains("old.txt"),
        "the oldest exchange is dropped whole, never partially"
    );
}

#[tokio::test]
async fn an_exchange_that_cannot_fit_alone_is_not_replaced_by_a_note() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let inference = ScriptedInference::with_budget(produced_reply(), 800);
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let oversized = TaskAgentActionExchange {
        request: TaskAgentOutput::new(String::from("{\"tool\":\"read\",\"path\":\"huge.txt\"}")),
        observation: TaskAgentObservation::new(
            TaskAgentObservationId::generate(),
            format!("read ok:\n{}", "x".repeat(2_000)),
        ),
    };
    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        TaskAgentTurnPremise {
            delegation: delegation_id,
            exchanges: vec![oversized],
        },
    )
    .await
    .expect("an over-budget transcript is still a domain outcome");

    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));
    let raw = scrubber.inputs();
    assert!(
        raw[0].contains("huge.txt") && raw[0].contains("xxxx"),
        "the oversized observation is kept so the port can refuse it, never replaced by a note"
    );
    assert!(
        raw[0].contains(&format!("read ok:\n{}", "x".repeat(2_000))),
        "the oversized observation is kept byte-for-byte"
    );
    assert!(
        raw[0].chars().count() > 800,
        "the turn leaves the over-limit input for the port to refuse honestly"
    );
}

#[tokio::test]
async fn aborted_port_answer_stays_its_own_turn_outcome() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    repository.script_task(Ok(Some(record(task, revision(1), "probe purpose"))));
    let inference = ScriptedInference::new(Ok(TaskAgentInferenceOutcome::Aborted));
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &repository,
        &NoInstructionSource,
        &inference,
        &scrubber,
        premise(delegation_id),
    )
    .await
    .expect("a local abort is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::Aborted,
        "a stopped turn is neither Produced nor a pre-send NotSent refusal"
    );
    assert_eq!(
        inference.premises().len(),
        1,
        "the aborted turn is still one attempted turn"
    );
}

#[tokio::test]
async fn action_exchanges_are_replayed_in_order_and_scrubbed_once() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let purpose_text = "probe adopted purpose";
    let loaded = record(task, revision(1), purpose_text);
    let purpose_source = loaded.context[0].origin.source;
    let fixture = turn_fixture(loaded, delegation_ref);

    let read_occurrence = TaskAgentObservationId::generate();
    let create_occurrence = TaskAgentObservationId::generate();
    let premise = TaskAgentTurnPremise {
        delegation: fixture.delegation,
        exchanges: vec![
            TaskAgentActionExchange {
                request: TaskAgentOutput::new(String::from(
                    r#"{"tool":"read","path":"input.txt"}"#,
                )),
                observation: TaskAgentObservation::new(read_occurrence, String::from("notes")),
            },
            TaskAgentActionExchange {
                request: TaskAgentOutput::new(String::from(
                    "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# report\"}",
                )),
                observation: TaskAgentObservation::new(
                    create_occurrence,
                    String::from("created at the requested workspace path"),
                ),
            },
        ],
    };
    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &fixture.inference,
        &fixture.scrubber,
        premise,
    )
    .await
    .expect("a produced turn is a domain outcome");
    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));

    let raw_input = format!(
        "{RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[PAST EXECUTED FACTS]\n\n\
         [TOOL CALL]\n{{\"tool\":\"read\",\"path\":\"input.txt\"}}\n\
         [TOOL RESULT]\nnotes\n\
         [TOOL CALL]\n{{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# report\"}}\n\
         [TOOL RESULT]\ncreated at the requested workspace path"
    );
    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert_eq!(
        received.prompt.text(),
        format!("[scrubbed] {raw_input}"),
        "the whole transcript is one framed input with a single scrub"
    );
    assert_eq!(
        fixture.scrubber.inputs(),
        vec![raw_input],
        "the scrubber sees the exchange transcript exactly once"
    );
    assert_eq!(
        received.data_use,
        vec![
            purpose_source,
            read_occurrence.as_raw(),
            create_occurrence.as_raw()
        ],
        "observation occurrence identities join the correlation in logical-input order"
    );
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
async fn terminal_task_at_a_moved_revision_is_reported_as_terminal() {
    let task = TaskId::generate();
    let relied = reference(task, 1);
    let delegation_ref = delegation(relied);
    let delegation_id = delegation_ref.delegation;
    let repository = FakeTaskRepository::new();
    repository.script_delegation(Ok(Some(delegation_ref)));
    let mut loaded = record(task, revision(2), "probe purpose");
    loaded.task.progress = TaskProgress::Failed;
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
            progress: TaskProgress::Failed,
        }
    );
    assert!(
        scrubber.inputs().is_empty(),
        "a terminal Task must not even be scrubbed when the relied revision moved"
    );
    assert!(
        inference.premises().is_empty(),
        "a terminal Task must not reach the port when the relied revision moved"
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
        vec![format!(
            "{RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[PAST EXECUTED FACTS]\n"
        )],
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
        TaskAgentNotSent::DataUseHeld,
        TaskAgentNotSent::UsageCapReached,
        TaskAgentNotSent::UsageCapIndeterminate,
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
        Err(TaskAgentTurnError::StorageUnavailable(
            TaskTechnicalError::StorageUnavailable {
                reason: String::from("load_delegation unavailable"),
            },
        )),
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
        Err(TaskAgentTurnError::StorageUnavailable(
            TaskTechnicalError::StorageUnavailable {
                reason: String::from("load_task unavailable"),
            },
        )),
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
        Err(TaskAgentTurnError::InferenceUnavailable(
            TaskAgentInferenceError::InferenceUnavailable { reason },
        )) => {
            assert_eq!(reason, "provider transport failed");
        }
        other => panic!("expected InferenceUnavailable, got {other:?}"),
    }
    assert_eq!(inference.premises().len(), 1);
}

#[tokio::test]
async fn debug_redacts_prompt_and_output_text() {
    let probe = "probe redaction text";
    let inference_premise = TaskAgentInferencePremise {
        delegation: DelegationId::generate(),
        task: reference(TaskId::generate(), 1),
        prompt: scrub_fixture(probe, CredentialSetRevision::initial()).await,
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
        "{RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[INSTRUCTION]\nfirst instruction\n[INSTRUCTION]\nsecond instruction\n[PAST EXECUTED FACTS]\n"
    );
    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert_eq!(
        received.prompt.text(),
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
        fixture.instructions.loaded_sources(),
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
        received.prompt.text().matches("repeat me").count(),
        2,
        "a repeated adoption repeats the body instead of deduplicating"
    );
    let purpose_source = received.data_use[0];
    assert_eq!(
        received.data_use,
        vec![purpose_source, repeated, repeated],
        "entry-level correlation duplicates are preserved"
    );
    assert_eq!(
        fixture.instructions.loaded_sources(),
        vec![repeated, repeated]
    );
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
    // source identity differs, the companion differs, the role differs, or
    // the resolved kind differs from the entry's origin kind.
    let cases: [(&str, RecordBuilder); 4] = [
        ("wrong source identity", |_source, companion| {
            source_record(RawId::new(), companion, TaskInstructionRole::Owner, "body")
        }),
        ("foreign companion", |source, _companion| {
            source_record(source, RawId::new(), TaskInstructionRole::Owner, "body")
        }),
        ("wrong role", |source, companion| {
            source_record(source, companion, TaskInstructionRole::Companion, "body")
        }),
        ("wrong kind", |source, companion| {
            source_record_with_kind(
                TaskContextOriginKind::OwnerManagement,
                source,
                companion,
                TaskInstructionRole::Owner,
                "body",
            )
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

#[tokio::test]
async fn past_executed_facts_render_every_turn_with_sources_after_instructions() {
    let purpose_text = "probe adopted purpose";
    let loaded = record(TaskId::generate(), revision(1), purpose_text);
    let relied = loaded.task.reference;
    let companion = loaded.task.assignee.companion;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let purpose_source = loaded.context[0].origin.source;
    let instruction = RawId::new();
    loaded.context.push(instruction_entry(
        relied,
        instruction,
        TaskContextOriginKind::OwnerConversation,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture.instructions.script(Ok(Some(source_record(
        instruction,
        companion,
        TaskInstructionRole::Owner,
        "the instruction",
    ))));
    let attempt = RawId::new();
    let result = RawId::new();
    fixture.repository.script_past_facts(Ok(ene_task::PastExecutedFactsPage {
        facts: vec![
            ene_task::PastExecutedFact {
                source: attempt,
                line: String::from("action 11111111-1111-1111-1111-111111111111 create /tmp/a.txt confirmed_success"),
            },
            ene_task::PastExecutedFact {
                source: result,
                line: String::from("result 22222222-2222-2222-2222-222222222222 rev=1 adopted=none"),
            },
        ],
        has_more: false,
    }));

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
        "{RESPONSE_FORMAT}[PURPOSE]\n{purpose_text}\n[INSTRUCTION]\nthe instruction\n[PAST EXECUTED FACTS]\naction 11111111-1111-1111-1111-111111111111 create /tmp/a.txt confirmed_success\nresult 22222222-2222-2222-2222-222222222222 rev=1 adopted=none\n"
    );
    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert_eq!(
        received.prompt.text(),
        format!("[scrubbed] {raw_input}"),
        "the facts block follows the instructions in the fixed framing"
    );
    assert_eq!(
        received.data_use,
        vec![purpose_source, instruction, attempt, result],
        "fact sources join the correlation after the purpose and instructions"
    );
    assert_eq!(
        fixture.scrubber.inputs(),
        vec![raw_input],
        "facts are scrubbed once with the rest of the logical input"
    );
}

#[tokio::test]
async fn truncated_past_facts_refuse_instead_of_reasoning_from_a_prefix() {
    let task = TaskId::generate();
    let fixture = turn_fixture(
        record(task, revision(1), "probe purpose"),
        delegation(reference(task, 1)),
    );
    fixture
        .repository
        .script_past_facts(Ok(ene_task::PastExecutedFactsPage {
            facts: Vec::new(),
            has_more: true,
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
            assert_eq!(reason, "past executed facts exceed the turn bound");
        }
        other => panic!("a truncated history must fail closed, got {other:?}"),
    }
    assert!(
        fixture.scrubber.inputs().is_empty(),
        "a truncated history is never scrubbed"
    );
    assert!(
        fixture.inference.premises().is_empty(),
        "a truncated history never reaches the port"
    );
}

#[tokio::test]
async fn past_facts_that_outgrow_the_budget_refuse_without_silent_omission() {
    let task = TaskId::generate();
    let fixture = turn_fixture(
        record(task, revision(1), "probe purpose"),
        delegation(reference(task, 1)),
    );
    fixture
        .repository
        .script_past_facts(Ok(ene_task::PastExecutedFactsPage {
            facts: vec![ene_task::PastExecutedFact {
                source: RawId::new(),
                line: format!(
                    "action {} create /tmp/a.txt confirmed_success",
                    "f".repeat(600)
                ),
            }],
            has_more: false,
        }));
    // Re-budget the port below the facts block: the transcript is already
    // empty, so only the never-omitted block can be at fault.
    let inference = ScriptedInference::with_budget(produced_reply(), 500);
    let scrubber = FakeScrubber::new(FakeScrubReply::Scrubbed);

    let outcome = orchestrate_task_agent_turn(
        &fixture.repository,
        &fixture.instructions,
        &inference,
        &scrubber,
        premise(fixture.delegation),
    )
    .await;
    match outcome {
        Err(TaskAgentTurnError::InputUnavailable { reason }) => {
            assert_eq!(
                reason,
                "never-omitted logical input exceeds the input budget"
            );
        }
        other => panic!("an over-budget history must fail closed, got {other:?}"),
    }
    assert!(
        scrubber.inputs().is_empty(),
        "an over-budget history is never scrubbed"
    );
    assert!(
        inference.premises().is_empty(),
        "an over-budget history never reaches the port"
    );
}

#[tokio::test]
async fn owner_management_instructions_resolve_like_conversation_ones() {
    let purpose_text = "probe adopted purpose";
    let loaded = record(TaskId::generate(), revision(1), purpose_text);
    let relied = loaded.task.reference;
    let companion = loaded.task.assignee.companion;
    let delegation_ref = delegation(relied);
    let mut loaded = loaded;
    let activity = RawId::new();
    loaded.context.push(instruction_entry(
        relied,
        activity,
        TaskContextOriginKind::OwnerManagement,
    ));
    let fixture = turn_fixture(loaded, delegation_ref);
    fixture.instructions.script(Ok(Some(source_record_with_kind(
        TaskContextOriginKind::OwnerManagement,
        activity,
        companion,
        TaskInstructionRole::Owner,
        "the management instruction",
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

    let captured = fixture.inference.premises();
    let received = captured.first().expect("the premise was captured");
    assert!(
        received
            .prompt
            .text()
            .contains("the management instruction"),
        "the activity body resolves into the logical input"
    );
    assert_eq!(
        fixture.instructions.loaded(),
        vec![TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerManagement,
            source: activity,
        }],
        "the origin kind selects the read"
    );
}
