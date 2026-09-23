//! Task orchestration integration tests: delegation, steering, and resume.
//!
//! Deterministic boundary checks using an in-test fake repository. Exercises
//! parameter translation, identity minting, precheck gating, and outcome
//! mapping without database or network dependencies.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures assert loudly"
)]

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, ConversationTaskRepository, CreateDelegationCommand, DelegatedWorkspace,
    DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef, DelegationScope,
    OwnerMessageCurrentness, PastExecutedFactsPage, ResumeInstructionSource, ResumeTaskCommand,
    SteeringPremiseRef, SteeringProposalPremise, Task, TaskAgentEphemeralId,
    TaskAgentObservationId, TaskAgentObservationPremise, TaskAgentOutput, TaskAgentResultArrival,
    TaskCancelOutcome, TaskCommitOutcome, TaskCommitPremise, TaskContextEntry, TaskContextEntryId,
    TaskContextItem, TaskContextOrigin, TaskContextOriginKind, TaskCreationOutcome,
    TaskCreationPremise, TaskFailureOutcome, TaskFailurePremise, TaskHeadline, TaskId,
    TaskProgress, TaskProposalOutcome, TaskPurpose, TaskPurposeRef, TaskRecord, TaskRef,
    TaskReportRow, TaskReportRowCursor, TaskReportRowKind, TaskReportSourcePage,
    TaskReportSourceRef, TaskRepository, TaskResultAcceptance, TaskResultAdoptionClaim,
    TaskResultArrivalOutcome, TaskResultId, TaskResultRecord, TaskResumeCommitPremise,
    TaskResumeHold, TaskResumeOutcome, TaskResumeReadiness, TaskRevision, TaskRevisionRecord,
    TaskTechnicalError, UnadoptedResultCursor, WorkspaceAssocId, WorkspaceFolderRef,
    orchestrate_delegation, orchestrate_resume, orchestrate_resume_current, orchestrate_steering,
};

/// Unified scripted `TaskRepository` for orchestration boundary checks.
#[derive(Default)]
struct FakeTaskRepository {
    load: Mutex<Option<Result<Option<TaskRecord>, TaskTechnicalError>>>,
    record: Mutex<Option<TaskRecord>>,
    forward: Mutex<Option<Result<TaskCommitOutcome, TaskTechnicalError>>>,
    forwarded: Mutex<Vec<TaskCommitPremise>>,
    delegated: Mutex<Vec<DelegationCreationPremise>>,
    delegator: Mutex<Option<AssigneeRef>>,
    commits: Mutex<VecDeque<Result<TaskResumeOutcome, TaskTechnicalError>>>,
    premises: Mutex<Vec<TaskResumeCommitPremise>>,
    currentness: Mutex<Vec<OwnerMessageCurrentness>>,
    report_rows: Mutex<Vec<TaskReportRow>>,
    results: Mutex<Vec<TaskResultRecord>>,
    claims: Mutex<Vec<TaskResultAdoptionClaim>>,
}

impl FakeTaskRepository {
    fn for_delegation(
        load: Result<Option<TaskRecord>, TaskTechnicalError>,
        delegator: AssigneeRef,
    ) -> Self {
        let repo = Self::default();
        *repo.load.lock().unwrap() = Some(load);
        *repo.delegator.lock().unwrap() = Some(delegator);
        repo
    }

    fn for_steering(
        load: Result<Option<TaskRecord>, TaskTechnicalError>,
        forward: Result<TaskCommitOutcome, TaskTechnicalError>,
    ) -> Self {
        let repo = Self::default();
        *repo.load.lock().unwrap() = Some(load);
        *repo.forward.lock().unwrap() = Some(forward);
        repo
    }

    fn script_commit(&self, reply: Result<TaskResumeOutcome, TaskTechnicalError>) {
        self.commits.lock().unwrap().push_back(reply);
    }

    fn delegated(&self) -> Vec<DelegationCreationPremise> {
        self.delegated.lock().unwrap().clone()
    }

    fn forwarded(&self) -> Vec<TaskCommitPremise> {
        self.forwarded.lock().unwrap().clone()
    }

    fn premises(&self) -> Vec<TaskResumeCommitPremise> {
        self.premises.lock().unwrap().clone()
    }

    fn claims(&self) -> Vec<TaskResultAdoptionClaim> {
        self.claims.lock().unwrap().clone()
    }
}

fn unsupported<T>(method: &'static str) -> Result<T, TaskTechnicalError> {
    Err(TaskTechnicalError::StorageUnavailable {
        reason: format!("{method} is outside this fixture's scope"),
    })
}

impl TaskRepository for FakeTaskRepository {
    async fn record_task_agent_observation(
        &self,
        _premise: TaskAgentObservationPremise,
    ) -> Result<TaskAgentObservationId, TaskTechnicalError> {
        unsupported("record_task_agent_observation")
    }

    async fn create_task(
        &self,
        _premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        unsupported("create_task")
    }

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        self.forwarded.lock().unwrap().push(premise);
        self.forward
            .lock()
            .unwrap()
            .as_ref()
            .expect("forward_steering called without scripted forward")
            .clone()
    }

    async fn cancel_task(&self, _task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        unsupported("cancel_task")
    }

    async fn load_task(&self, _task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        if let Some(load) = self.load.lock().unwrap().as_ref() {
            load.clone()
        } else {
            Ok(self.record.lock().unwrap().clone())
        }
    }

    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        self.delegated.lock().unwrap().push(premise.clone());
        let delegator = self
            .delegator
            .lock()
            .unwrap()
            .expect("delegator must be set for create_delegation");
        Ok(DelegationOutcome::Delegated(DelegationRef {
            delegation: premise.delegation,
            task: premise.task,
            delegator,
            agent: premise.agent,
            scope: premise.scope_copy,
        }))
    }

    async fn load_delegation(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        unsupported("load_delegation")
    }

    async fn record_task_result_arrival(
        &self,
        _arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultArrivalOutcome, TaskTechnicalError> {
        unsupported("record_task_result_arrival")
    }

    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Ok(self
            .results
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.result == result)
            .cloned())
    }

    async fn load_delegation_result(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        unsupported("load_delegation_result")
    }

    async fn delegation_has_started_work(
        &self,
        _delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        unsupported("delegation_has_started_work")
    }

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        self.claims.lock().unwrap().push(claim);
        Ok(TaskResultAcceptance::WithheldByEffectFacts {
            attempts: Vec::new(),
        })
    }

    async fn fail_task(
        &self,
        _premise: TaskFailurePremise,
    ) -> Result<TaskFailureOutcome, TaskTechnicalError> {
        unsupported("fail_task")
    }

    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        Ok(Some(TaskResultAdoptionClaim {
            result,
            attempt_refs: Vec::new(),
        }))
    }

    async fn list_unadopted_results_after(
        &self,
        _after: Option<UnadoptedResultCursor>,
        _limit: u64,
    ) -> Result<Vec<UnadoptedResultCursor>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn load_task_action_attempts(
        &self,
        _task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn list_tasks_after(
        &self,
        _after: Option<TaskId>,
        _limit: u32,
    ) -> Result<Vec<TaskHeadline>, TaskTechnicalError> {
        unsupported("list_tasks_after")
    }

    async fn list_task_report_rows_after(
        &self,
        _task: TaskId,
        _after: Option<TaskReportRowCursor>,
        _limit: u32,
    ) -> Result<Vec<TaskReportRow>, TaskTechnicalError> {
        Ok(self.report_rows.lock().unwrap().clone())
    }

    async fn load_report_source_bounded(
        &self,
        _source: TaskReportSourceRef,
        _cursor_bytes: u64,
        _limit_bytes: u32,
    ) -> Result<Option<TaskReportSourcePage>, TaskTechnicalError> {
        unsupported("load_report_source_bounded")
    }

    async fn commit_task_resume(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.premises.lock().unwrap().push(premise);
        self.commits
            .lock()
            .unwrap()
            .pop_front()
            .expect("commit_task_resume called without scripted reply")
    }

    async fn load_past_executed_facts(
        &self,
        _task: TaskId,
    ) -> Result<PastExecutedFactsPage, TaskTechnicalError> {
        Ok(PastExecutedFactsPage {
            facts: Vec::new(),
            has_more: false,
        })
    }
}

impl ConversationTaskRepository for FakeTaskRepository {
    async fn create_task_from_conversation(
        &self,
        _premise: TaskCreationPremise,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCreationOutcome, TaskTechnicalError> {
        unsupported("create_task_from_conversation")
    }

    async fn forward_steering_from_conversation(
        &self,
        _premise: TaskCommitPremise,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        unsupported("forward_steering_from_conversation")
    }

    async fn cancel_task_from_conversation(
        &self,
        _task: TaskId,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        unsupported("cancel_task_from_conversation")
    }

    async fn commit_task_resume_from_conversation(
        &self,
        premise: TaskResumeCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.currentness.lock().unwrap().push(currentness);
        self.commit_task_resume(premise).await
    }
}

mod delegation {
    use super::*;

    fn clock() -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
            .expect("fixture timestamp parses")
    }

    fn revision(value: u64) -> TaskRevision {
        TaskRevision::from_u64(value)
    }

    fn assignee() -> AssigneeRef {
        AssigneeRef {
            companion: RawId::new(),
        }
    }

    fn record(
        task: TaskId,
        current_revision: TaskRevision,
        purpose: TaskPurposeRef,
        purpose_entry: TaskContextEntryId,
    ) -> TaskRecord {
        let reference = TaskRef {
            task,
            revision: current_revision,
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
                    text: String::from("loaded purpose body"),
                },
                assignee,
            },
            context: vec![TaskContextEntry {
                entry: purpose_entry,
                reference,
                item: TaskContextItem::AdoptedPurpose(purpose),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: clock(),
            }],
            workspace: None,
        }
    }

    fn command(task: TaskRef, scope_copy: DelegationScope) -> CreateDelegationCommand {
        CreateDelegationCommand { task, scope_copy }
    }

    #[tokio::test]
    async fn stale_revision_reports_current_without_creating_a_delegation() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let current = TaskRef {
            task,
            revision: revision(2),
        };
        let current_purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(2),
        };
        let repository = FakeTaskRepository::for_delegation(
            Ok(Some(record(
                task,
                revision(2),
                current_purpose,
                TaskContextEntryId::generate(),
            ))),
            assignee(),
        );

        let outcome = orchestrate_delegation(
            &repository,
            command(expected, DelegationScope { workspace: None }),
        )
        .await
        .expect("a stale premise is a domain outcome, not a technical error");

        match outcome {
            DelegationOutcome::StaleTaskRevision { current: found } => assert_eq!(found, current),
            other => panic!("expected StaleTaskRevision, got {other:?}"),
        }
        assert!(
            repository.delegated().is_empty(),
            "a stale revision must not create a delegation"
        );
    }

    #[tokio::test]
    async fn delegated_outcome_echoes_the_minted_identities_and_the_command() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(1),
        };
        let delegator = assignee();
        let scope_copy = DelegationScope {
            workspace: Some(DelegatedWorkspace {
                assoc: WorkspaceAssocId::generate(),
                folder: WorkspaceFolderRef {
                    path: String::from("/workspace/inbox/"),
                },
                save_target: Some(WorkspaceFolderRef {
                    path: String::from("/workspace/outbox/"),
                }),
            }),
        };
        let repository = FakeTaskRepository::for_delegation(
            Ok(Some(record(
                task,
                revision(1),
                purpose,
                TaskContextEntryId::generate(),
            ))),
            delegator,
        );

        let outcome = orchestrate_delegation(&repository, command(expected, scope_copy.clone()))
            .await
            .expect("a delegation decision is a domain outcome, not a technical error");

        let captured = repository.delegated();
        assert_eq!(
            captured.len(),
            1,
            "one command creates at most one delegation"
        );
        let premise = captured
            .first()
            .expect("the delegation premise was captured");

        match outcome {
            DelegationOutcome::Delegated(found) => {
                assert_eq!(
                    found.delegation, premise.delegation,
                    "the minted delegation identity is the one the repository committed"
                );
                assert_eq!(
                    found.agent, premise.agent,
                    "the minted ephemeral agent identity is the one the repository committed"
                );
                assert_eq!(
                    found.task, expected,
                    "the committed delegation relies on the command's revision"
                );
                assert_eq!(
                    found.delegator, delegator,
                    "the repository stamps the delegator; the orchestration never passes one"
                );
                assert_eq!(
                    found.scope, scope_copy,
                    "the command's scope copy crosses unchanged"
                );
            }
            other => panic!("expected Delegated, got {other:?}"),
        }
        assert_eq!(
            premise.task, expected,
            "the precheck's task token crosses unchanged"
        );
        assert_eq!(
            premise.scope_copy, scope_copy,
            "the precheck's scope copy crosses unchanged"
        );
    }
}

mod steering {
    use super::*;

    fn clock() -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
            .expect("fixture timestamp parses")
    }

    fn revision(value: u64) -> TaskRevision {
        TaskRevision::from_u64(value)
    }

    fn record(
        task: TaskId,
        current_revision: TaskRevision,
        purpose: TaskPurposeRef,
        purpose_entry: TaskContextEntryId,
    ) -> TaskRecord {
        let reference = TaskRef {
            task,
            revision: current_revision,
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
                    text: String::from("loaded purpose body"),
                },
                assignee,
            },
            context: vec![TaskContextEntry {
                entry: purpose_entry,
                reference,
                item: TaskContextItem::AdoptedPurpose(purpose),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: clock(),
            }],
            workspace: None,
        }
    }

    fn proposal(
        expected: TaskRef,
        purpose: TaskPurposeRef,
        new_purpose: Option<TaskPurpose>,
        instruction_source: RawId,
    ) -> SteeringProposalPremise {
        SteeringProposalPremise {
            premise: SteeringPremiseRef { expected, purpose },
            new_purpose,
            instruction_source,
        }
    }

    #[tokio::test]
    async fn accepted_steering_mints_fresh_entries_and_keeps_expected_unchanged() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let current_purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(1),
        };
        let existing_purpose_entry = TaskContextEntryId::generate();
        let instruction_source = RawId::new();
        let accepted = TaskRef {
            task,
            revision: revision(2),
        };
        let repository = FakeTaskRepository::for_steering(
            Ok(Some(record(
                task,
                revision(1),
                current_purpose,
                existing_purpose_entry,
            ))),
            Ok(TaskCommitOutcome::CommittedAs(accepted)),
        );

        let outcome = orchestrate_steering(
            &repository,
            proposal(
                expected,
                current_purpose,
                Some(TaskPurpose {
                    text: String::from("steer toward the new goal"),
                }),
                instruction_source,
            ),
        )
        .await
        .expect("a steering decision is a domain outcome, not a technical error");

        match outcome {
            TaskProposalOutcome::AcceptedAsSteering(found) => assert_eq!(found, accepted),
            other => panic!("expected AcceptedAsSteering, got {other:?}"),
        }

        let forwarded = repository.forwarded();
        assert_eq!(
            forwarded.len(),
            1,
            "one proposal commits at most one forward"
        );
        let premise = forwarded.first().expect("the forward premise was captured");
        assert_eq!(
            premise.expected, expected,
            "the caller's boundary token travels unchanged"
        );
        // Compile-level: `TaskCommitPremise` has no successor-revision field; the
        // repository derives `expected.revision + 1` only after the CAS succeeds.
        assert_eq!(
            accepted.revision,
            expected
                .revision
                .checked_next()
                .expect("revision 1 has a successor"),
            "the repository, not the caller, names the new revision"
        );

        let purpose_adoption = premise
            .new_purpose
            .as_ref()
            .expect("the new purpose is adopted");
        assert_eq!(
            purpose_adoption.purpose,
            TaskPurpose {
                text: String::from("steer toward the new goal"),
            }
        );
        let origin = TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source: instruction_source,
        };
        assert_eq!(
            purpose_adoption.origin, origin,
            "the new purpose entry is sourced from the steering utterance"
        );

        let instruction = premise
            .adopted_instruction
            .as_ref()
            .expect("this forward adopts the instruction");
        assert_eq!(
            instruction.origin, origin,
            "the adopted instruction is sourced from the same utterance"
        );
        assert_ne!(
            premise.adopted_purpose_entry, existing_purpose_entry,
            "the new revision records a fresh purpose entry"
        );
        assert_ne!(
            instruction.entry, existing_purpose_entry,
            "the instruction entry is not the predecessor's purpose entry"
        );
        assert_ne!(
            premise.adopted_purpose_entry, instruction.entry,
            "purpose and instruction entries are distinct identities"
        );
        assert_ne!(
            purpose_adoption.acquired_at,
            clock(),
            "the purpose adoption time is captured at adoption, not copied from the loaded record"
        );
        assert_ne!(
            instruction.acquired_at,
            clock(),
            "the instruction adoption time is captured at adoption"
        );
    }

    #[tokio::test]
    async fn unchanged_purpose_carries_forward_while_the_instruction_is_adopted() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let current_purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(1),
        };
        let existing_purpose_entry = TaskContextEntryId::generate();
        let accepted = TaskRef {
            task,
            revision: revision(2),
        };
        let repository = FakeTaskRepository::for_steering(
            Ok(Some(record(
                task,
                revision(1),
                current_purpose,
                existing_purpose_entry,
            ))),
            Ok(TaskCommitOutcome::CommittedAs(accepted)),
        );

        let outcome = orchestrate_steering(
            &repository,
            proposal(expected, current_purpose, None, RawId::new()),
        )
        .await
        .expect("a steering decision is a domain outcome, not a technical error");

        match outcome {
            TaskProposalOutcome::AcceptedAsSteering(found) => assert_eq!(found, accepted),
            other => panic!("expected AcceptedAsSteering, got {other:?}"),
        }

        let forwarded = repository.forwarded();
        assert_eq!(
            forwarded.len(),
            1,
            "an unchanged purpose is still one revision forward"
        );
        let premise = forwarded.first().expect("the forward premise was captured");
        assert!(
            premise.new_purpose.is_none(),
            "an unchanged purpose carries the adopted purpose identity forward"
        );
        assert!(
            premise.adopted_instruction.is_some(),
            "steering always adopts its instruction in this slice"
        );
        assert_ne!(
            premise.adopted_purpose_entry, existing_purpose_entry,
            "a carry-forward still records the new revision's own purpose entry"
        );
    }

    #[tokio::test]
    async fn stale_revision_reports_current_without_forwarding() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let current = TaskRef {
            task,
            revision: revision(2),
        };
        let current_purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(2),
        };
        let repository = FakeTaskRepository::for_steering(
            Ok(Some(record(
                task,
                revision(2),
                current_purpose,
                TaskContextEntryId::generate(),
            ))),
            Ok(TaskCommitOutcome::CommittedAs(TaskRef {
                task,
                revision: revision(3),
            })),
        );

        let outcome = orchestrate_steering(
            &repository,
            proposal(
                expected,
                TaskPurposeRef {
                    task,
                    adopted_revision: revision(1),
                },
                Some(TaskPurpose {
                    text: String::from("stale proposal"),
                }),
                RawId::new(),
            ),
        )
        .await
        .expect("a stale premise is a domain outcome, not a technical error");

        match outcome {
            TaskProposalOutcome::StalePremise { current: found } => assert_eq!(found, current),
            other => panic!("expected StalePremise, got {other:?}"),
        }
        assert!(
            repository.forwarded().is_empty(),
            "a stale revision must not forward"
        );
    }

    #[tokio::test]
    async fn terminal_task_is_reported_without_minting_or_forwarding() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(1),
        };
        let mut loaded = record(task, revision(1), purpose, TaskContextEntryId::generate());
        loaded.task.progress = TaskProgress::Completed;
        let repository = FakeTaskRepository::for_steering(
            Ok(Some(loaded)),
            Ok(TaskCommitOutcome::CommittedAs(TaskRef {
                task,
                revision: revision(2),
            })),
        );

        let outcome =
            orchestrate_steering(&repository, proposal(expected, purpose, None, RawId::new()))
                .await
                .expect("terminal progress is a domain outcome, not a technical error");

        assert_eq!(
            outcome,
            TaskProposalOutcome::TaskTerminal {
                task,
                progress: TaskProgress::Completed,
            }
        );
        assert!(
            repository.forwarded().is_empty(),
            "a terminal Task must not receive a steering commit"
        );
    }

    #[tokio::test]
    async fn cas_race_stale_expected_maps_to_stale_premise() {
        let task = TaskId::generate();
        let expected = TaskRef {
            task,
            revision: revision(1),
        };
        let current_purpose = TaskPurposeRef {
            task,
            adopted_revision: revision(1),
        };
        let raced = TaskRef {
            task,
            revision: revision(2),
        };
        let repository = FakeTaskRepository::for_steering(
            Ok(Some(record(
                task,
                revision(1),
                current_purpose,
                TaskContextEntryId::generate(),
            ))),
            Ok(TaskCommitOutcome::StaleExpected { current: raced }),
        );

        let outcome = orchestrate_steering(
            &repository,
            proposal(expected, current_purpose, None, RawId::new()),
        )
        .await
        .expect("a lost compare is a domain outcome, not a technical error");

        match outcome {
            TaskProposalOutcome::StalePremise { current: found } => assert_eq!(found, raced),
            other => panic!("expected StalePremise, got {other:?}"),
        }
        assert_eq!(
            repository.forwarded().len(),
            1,
            "the pre-check passed; the race is decided inside forward_steering"
        );
    }
}

mod resume {
    use super::*;

    fn readiness() -> TaskResumeReadiness {
        TaskResumeReadiness {
            permission_available: true,
            execution_free: true,
            launch_possible: true,
        }
    }

    fn task_ref(task: TaskId, revision: u64) -> TaskRef {
        TaskRef {
            task,
            revision: TaskRevision::from_u64(revision),
        }
    }

    fn command(task: TaskRef, purpose: TaskPurposeRef) -> ResumeTaskCommand {
        ResumeTaskCommand {
            premise: SteeringPremiseRef {
                expected: task,
                purpose,
            },
            instruction: ResumeInstructionSource::OwnerHistory {
                message: RawId::new(),
                currentness: OwnerMessageCurrentness {
                    companion: RawId::new(),
                    message: RawId::new(),
                },
            },
        }
    }

    fn resumed(task: TaskRef) -> TaskResumeOutcome {
        TaskResumeOutcome::Resumed {
            task,
            delegation: DelegationRef {
                delegation: DelegationId::generate(),
                task,
                delegator: AssigneeRef {
                    companion: RawId::new(),
                },
                agent: TaskAgentEphemeralId::generate(),
                scope: DelegationScope { workspace: None },
            },
        }
    }

    #[tokio::test]
    async fn resume_mints_fresh_identities_and_passes_the_command_through() {
        let repository = FakeTaskRepository::default();
        let task = TaskId::generate();
        let relied = task_ref(task, 3);
        let purpose = TaskPurposeRef {
            task,
            adopted_revision: TaskRevision::from_u64(3),
        };
        let instruction = ResumeInstructionSource::OwnerManagement {
            activity: RawId::new(),
        };
        repository.script_commit(Ok(resumed(task_ref(task, 4))));

        let outcome = orchestrate_resume(
            &repository,
            ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: relied,
                    purpose,
                },
                instruction,
            },
            readiness(),
        )
        .await
        .expect("a committed resume is a domain outcome");

        assert!(matches!(outcome, TaskResumeOutcome::Resumed { .. }));
        let premises = repository.premises();
        assert_eq!(premises.len(), 1);
        let premise = premises.first().expect("the premise was captured");
        assert_eq!(premise.command.premise.expected, relied);
        assert_eq!(premise.command.premise.purpose, purpose);
        assert_eq!(premise.command.instruction, instruction);
        assert_eq!(premise.readiness, readiness());

        // A second resume mints entirely fresh identities: the caller never
        // names an entry, delegation, or agent.
        repository.script_commit(Ok(resumed(task_ref(task, 5))));
        orchestrate_resume(
            &repository,
            ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: task_ref(task, 4),
                    purpose,
                },
                instruction,
            },
            readiness(),
        )
        .await
        .expect("a second resume is a domain outcome");
        let premises = repository.premises();
        assert_eq!(premises.len(), 2);
        assert_ne!(
            premises[0].adopted_purpose_entry,
            premises[1].adopted_purpose_entry
        );
        assert_ne!(
            premises[0].adopted_instruction_entry,
            premises[1].adopted_instruction_entry
        );
        assert_ne!(premises[0].delegation, premises[1].delegation);
        assert_ne!(premises[0].agent, premises[1].agent);
    }

    #[tokio::test]
    async fn refusals_pass_through_without_routing() {
        for outcome in [
            TaskResumeOutcome::StalePremise {
                current: task_ref(TaskId::generate(), 2),
            },
            TaskResumeOutcome::Superseded,
            TaskResumeOutcome::TaskTerminal {
                task: TaskId::generate(),
                progress: TaskProgress::Cancelled,
            },
            TaskResumeOutcome::AlreadyRunning {
                task: TaskId::generate(),
            },
            TaskResumeOutcome::HeldByUnknownEffects {
                task: TaskId::generate(),
            },
            TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::WorkspaceUnavailable),
            TaskResumeOutcome::MissingTask {
                task: TaskId::generate(),
            },
            TaskResumeOutcome::RevisionExhausted {
                task: TaskId::generate(),
            },
        ] {
            let repository = FakeTaskRepository::default();
            let task = TaskId::generate();
            repository.script_commit(Ok(outcome.clone()));
            let answered = orchestrate_resume(
                &repository,
                command(
                    task_ref(task, 1),
                    TaskPurposeRef {
                        task,
                        adopted_revision: TaskRevision::from_u64(1),
                    },
                ),
                readiness(),
            )
            .await
            .expect("a refusal is a domain outcome, not a technical error");
            assert_eq!(answered, outcome);
            assert!(
                repository.claims().is_empty(),
                "only ResultAvailable routes to adoption, got {outcome:?}"
            );
        }
    }

    #[tokio::test]
    async fn result_available_routes_the_current_revision_through_adoption() {
        let repository = FakeTaskRepository::default();
        let task = TaskId::generate();
        let current = task_ref(task, 2);
        let result = TaskResultId::generate();
        let delegation = DelegationId::generate();
        repository.script_commit(Ok(TaskResumeOutcome::ResultAvailable { task }));
        *repository
            .record
            .lock()
            .expect("fixture script is never poisoned") = Some(TaskRecord {
            task: Task {
                reference: current,
                purpose: TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(2),
                },
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
                progress: TaskProgress::InProgress,
                adopted_result: None,
            },
            revision: TaskRevisionRecord {
                reference: current,
                purpose: TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(2),
                },
                purpose_text: TaskPurpose {
                    text: String::from("probe purpose"),
                },
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
            },
            context: vec![TaskContextEntry {
                entry: TaskContextEntryId::generate(),
                reference: current,
                item: TaskContextItem::AdoptedPurpose(TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(2),
                }),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: WallClockWithTz::now(),
            }],
            workspace: None,
        });
        *repository
            .report_rows
            .lock()
            .expect("fixture script is never poisoned") = vec![TaskReportRow {
            kind: TaskReportRowKind::TaskResult,
            id: result.as_raw(),
            adopted_revision: None,
        }];
        *repository
            .results
            .lock()
            .expect("fixture script is never poisoned") = vec![TaskResultRecord {
            result,
            task: current,
            delegation,
            body: TaskAgentOutput::new(String::from("the sealed body")),
            attempt_refs: Vec::new(),
            adopted_revision: None,
            recorded_at: WallClockWithTz::now(),
        }];

        let outcome = orchestrate_resume(
            &repository,
            command(
                current,
                TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(2),
                },
            ),
            readiness(),
        )
        .await
        .expect("an available result is a domain outcome");

        assert_eq!(outcome, TaskResumeOutcome::ResultAvailable { task });
        let claims = repository.claims();
        assert_eq!(claims.len(), 1, "the sealed result is re-evaluated once");
        assert_eq!(
            claims.first().expect("the claim was captured").result,
            result
        );
    }

    /// Guarded-commit fake: the same scripted answers behind the
    /// conversation-sourced boundary.

    #[tokio::test]
    async fn guarded_resume_carries_the_currentness_to_the_commit() {
        let repository = FakeTaskRepository::default();
        let task = TaskId::generate();
        repository.script_commit(Ok(resumed(task_ref(task, 2))));
        let currentness = OwnerMessageCurrentness {
            companion: RawId::new(),
            message: RawId::new(),
        };

        let outcome = orchestrate_resume_current(
            &repository,
            command(
                task_ref(task, 1),
                TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(1),
                },
            ),
            readiness(),
            currentness,
        )
        .await
        .expect("a guarded resume is a domain outcome");

        assert!(matches!(outcome, TaskResumeOutcome::Resumed { .. }));
        assert_eq!(
            repository
                .currentness
                .lock()
                .expect("fixture capture is never poisoned")
                .clone(),
            vec![currentness]
        );
    }
}
