use crate::Store;
mod publication;
use ene_companion::{
    AppendHistoryCommand, CompanionId, CompanionLifecycle, CompanionRepository,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, UndeliveredRepository,
};
use ene_credential::{
    CredentialIntentRepository as _, CredentialRef, CredentialRefRepository,
    CredentialSetRepository, CredentialSetRevision, CredentialTechnicalError,
    MemoryCredentialStore, RegistrationApply, RegistrationFingerprint, RegistrationState,
};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId,
    TaskAgentAttemptPremise, UsageFact, UsageRepository, UsageSource,
};
use ene_learning::{
    ChangeKind, ExperienceSourceKind, Importance, LearningRepository, LearningScope, MemoryChange,
    MemoryChangeCommit, MemoryChangeOutcome, MemoryId, MemoryRevision, MemoryTarget,
    SourceRangeRef, SummaryId, SummaryRecord, TemporalMeaning,
};
use ene_permission::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
    ConsumerKind, IntentFingerprint, IntentOutcomeRepository, IntentResolution, PurposeKind,
};
use ene_presence::{PresenceGeneration, PresenceRepository, PresenceState};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationScope, TaskAgentEphemeralId, TaskCommitOutcome, TaskCommitPremise,
    TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise, TaskId,
    TaskPurpose, TaskRef, TaskRepository, TaskResultArrivalOutcome, TaskResultRecord,
    TaskResultScrubPremise, TaskRevision, TaskTechnicalError, WorkspaceAssocId,
    WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef, orchestrate_result_arrival,
};
use rusqlite::params;

fn fixture_clock() -> WallClockWithTz {
    if let Ok(at) = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00") {
        at
    } else {
        WallClockWithTz::now()
    }
}

fn registration_fingerprint(
    intent_id: &str,
    provider: &str,
    label: &str,
) -> RegistrationFingerprint {
    RegistrationFingerprint {
        intent_id: intent_id.to_owned(),
        kind: String::from("register"),
        target: format!("credential:{provider}:{label}"),
        base: String::from("consent-none"),
        rationale_origin: String::from("management-surface"),
        rationale_quote: None,
    }
}

async fn approve_pair(store: &Store, provider: &str, label: &str, bearer: &str, intent_id: &str) {
    let applied = store
        .request_registration_with_intent(
            provider.to_owned(),
            label.to_owned(),
            registration_fingerprint(intent_id, provider, label),
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
}

fn history_command(
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
) -> AppendHistoryCommand {
    AppendHistoryCommand {
        companion,
        round: RawId::new(),
        role: HistoryRole::Owner,
        text: text.to_owned(),
        lang: String::from("en"),
        at: fixture_clock(),
        expected_generation: generation,
        expected_consent: None,
        expected_credential_set: None,
        expected_owner_message: None,
        command_id: None,
        round_wire: Some(RawId::new().as_uuid().to_string()),
        round_intent: None,
        incarnation: Some((1, 2)),
        local_id: None,
    }
}

async fn open_memory() -> Option<Store> {
    let opened = Store::open_in_memory().await;
    assert!(opened.is_ok(), "in-memory open must succeed");
    opened.ok()
}

async fn running_companion(store: &Store) -> Option<(CompanionId, PresenceGeneration)> {
    let ensured = store.ensure_running_companion().await;
    assert!(ensured.is_ok(), "ensure must succeed");
    let Ok(companion) = ensured else {
        return None;
    };
    let loaded = store.load_attribution(companion.as_raw()).await;
    assert!(loaded.is_ok(), "attribution load must succeed");
    let Ok(Some(attribution)) = loaded else {
        return None;
    };
    assert_eq!(attribution.state, PresenceState::NoActive);
    Some((companion, attribution.generation))
}

#[tokio::test]
async fn supersession_probe_is_index_backed_not_a_scan() {
    let store = open_memory().await.unwrap();
    let plan = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut statement = guard
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                crate::companion::SQL_EXISTS_NEWER_OWNER
            ))
            .expect("the probe must explain");
        statement
            .query_map(rusqlite::params!["companion", "owner", 1_i64], |row| {
                row.get::<_, String>(3)
            })
            .expect("the plan must read")
            .collect::<Result<Vec<_>, _>>()
            .expect("plan rows must decode")
    };
    assert!(!plan.is_empty(), "the probe plan must have steps");
    for step in &plan {
        assert!(
            !step.starts_with("SCAN history_message") && !step.contains("TEMP B-TREE"),
            "the probe must seek the index with no table scan or sort, got: {plan:?}"
        );
    }
    assert!(
        plan.iter()
            .any(|step| step.contains("idx_history_message_companion_role")),
        "the probe must use the companion-role index, got: {plan:?}"
    );
}

async fn save_consent(
    store: &Store,
    expected: Option<(String, ConsentRevision)>,
    record: ConsentRecord,
) -> ConsentCommitOutcome {
    let fingerprint = IntentFingerprint {
        intent_id: RawId::new().as_uuid().to_string(),
        kind: String::from("assign"),
        target: String::from("consent:test-seed"),
        base: String::from("consent-test-seed"),
        rationale_origin: String::from("management-surface"),
        rationale_quote: None,
    };
    match store
        .assign_with_intent(expected, record, fingerprint)
        .await
        .expect("the test consent write must answer")
    {
        IntentResolution::Decided(outcome) => outcome,
        IntentResolution::Replay(_) | IntentResolution::Conflict(_) => {
            panic!("a fresh intent id must decide")
        }
    }
}

#[tokio::test]
async fn load_message_reads_one_row_by_primary_key_and_fails_closed() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let appended = store
        .append_message(history_command(companion, generation, "single body"))
        .await;
    let HistoryAppendOutcome::CommittedAs { message } = appended.unwrap() else {
        panic!("the append must commit");
    };

    let found = store
        .load_message(message)
        .await
        .expect("the bounded read must answer")
        .expect("the addressed row must load");
    assert_eq!(found.id, message);
    assert_eq!(found.companion, companion);
    assert_eq!(found.text, "single body");
    assert_eq!(found.role, HistoryRole::Owner);
    assert_eq!(found.presence_generation, generation);

    assert_eq!(
        store.load_message(RawId::new()).await,
        Ok(None),
        "an absent identity is reported, never fabricated"
    );

    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let plan_sql = format!(
            "EXPLAIN QUERY PLAN {}",
            crate::companion::SQL_SELECT_HISTORY_BY_MESSAGE
        );
        let mut statement = guard.prepare(&plan_sql).expect("the plan must prepare");
        let details: Vec<String> = statement
            .query_map(params![crate::codec::encode_id(message)], |row| row.get(3))
            .expect("the plan must run")
            .collect::<Result<Vec<_>, _>>()
            .expect("the plan rows must decode");
        assert!(
            details.iter().all(|detail| !detail.contains("SCAN")),
            "the bounded read never scans: {details:?}"
        );
        assert!(
            details.iter().any(|detail| detail.contains("SEARCH")),
            "the bounded read uses the message identity: {details:?}"
        );
    }

    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE history_message SET at = 'not-a-timestamp' WHERE message_id = ?1",
                params![crate::codec::encode_id(message)],
            )
            .expect("the corruption must apply");
    }
    assert!(
        matches!(
            store.load_message(message).await,
            Err(ene_companion::CompanionTechnicalError::StorageUnavailable { .. })
        ),
        "a malformed row fails closed"
    );
}

#[tokio::test]
async fn concurrent_same_id_assigns_fork_nothing() {
    use ene_permission::{
        ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcomeRepository as _,
        IntentResolution,
    };

    let store = open_memory().await.unwrap();
    let attempt = |model: &'static str| {
        let store = &store;
        let fingerprint = IntentFingerprint {
            intent_id: String::from("race-1"),
            kind: String::from("assign"),
            target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
            base: String::from("consent-none"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        };
        async move {
            store
                .assign_with_intent(
                    None,
                    ConsentRecord {
                        capability: CapabilityKind::Dialogue,
                        id: String::from("consent-1"),
                        rev: ConsentRevision::from_u64(1),
                        provider: String::from("openai"),
                        model: String::from(model),
                        credential_id: String::from("openai:main"),
                    },
                    fingerprint,
                )
                .await
        }
    };
    let (first, second) = tokio::join!(attempt("dialogue-1"), attempt("dialogue-1"));
    let decided_count = [&first, &second]
        .iter()
        .filter(|result| {
            matches!(
                result,
                Ok(IntentResolution::Decided(
                    ConsentCommitOutcome::Committed { .. }
                ))
            )
        })
        .count();
    assert_eq!(
        decided_count, 1,
        "exactly one racer must decide, got {first:?} / {second:?}"
    );
    for result in [&first, &second] {
        assert!(
            matches!(
                result,
                Ok(
                    IntentResolution::Decided(ConsentCommitOutcome::Committed { .. })
                        | IntentResolution::Replay(_)
                )
            ),
            "the loser must replay, never conflict or fail, got {result:?}"
        );
    }
}

fn learning_summary(companion: RawId, content: &str) -> SummaryRecord {
    SummaryRecord {
        id: SummaryId::generate(),
        scope: LearningScope::companion(companion),
        content: content.to_owned(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: RawId::new(),
            end: RawId::new(),
        },
        formed_at: fixture_clock(),
    }
}

fn learning_change(
    companion: RawId,
    target: MemoryTarget,
    content: &str,
    change: ChangeKind,
    recall_suppressed: bool,
) -> MemoryChange {
    MemoryChange {
        target,
        scope: LearningScope::companion(companion),
        content: content.to_owned(),
        importance: Importance::clamped(4),
        temporal: TemporalMeaning::Enduring,
        change,
        recall_suppressed,
        at: fixture_clock(),
    }
}

fn commit(summary: Option<SummaryRecord>, change: MemoryChange) -> MemoryChangeCommit {
    MemoryChangeCommit {
        summary,
        secret_premise: None,
        claim: None,
        change,
    }
}

#[tokio::test]
async fn memory_revision_pages_and_summary_batches_are_bounded() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "shared grounds");
    let first = store
        .commit_memory_change(commit(
            Some(evidence.clone()),
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "revision one",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(first, Ok(MemoryChangeOutcome::Committed { .. })));

    let mut expected = vec![MemoryRevision::initial()];
    let mut revision = MemoryRevision::initial();
    for index in 1..=205_u64 {
        let outcome = store
            .commit_memory_change(commit(
                Some(evidence.clone()),
                learning_change(
                    companion,
                    MemoryTarget::Existing {
                        id: memory,
                        expected_revision: revision,
                    },
                    &format!("revision {index}"),
                    ChangeKind::Refined,
                    false,
                ),
            ))
            .await
            .unwrap();
        let MemoryChangeOutcome::Committed { revision: next, .. } = outcome else {
            panic!("revision {index} must commit: {outcome:?}");
        };
        expected.push(next);
        revision = next;
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list_memory_revisions(memory, cursor, 7)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        for revision in &page {
            seen.push(revision.revision);
        }
        cursor = page.last().map(|revision| revision.revision);
    }
    assert_eq!(seen, expected, "pages cover every revision without skips");
    assert!(
        store
            .list_memory_revisions(memory, None, 0)
            .await
            .unwrap()
            .is_empty(),
        "a zero limit reads no revisions"
    );

    let ids: Vec<SummaryId> = expected
        .iter()
        .map(|_| evidence.id)
        .chain(std::iter::once(SummaryId::generate()))
        .collect();
    let loaded = store.load_summaries(&ids).await.unwrap();
    assert_eq!(
        loaded.len(),
        1,
        "the shared grounds and the missing id read as one and nothing"
    );
    assert!(store.load_summaries(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn recall_candidates_are_bounded_and_reach_old_relevant_rows() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let old = MemoryId::generate();
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: old },
                "The owner likes jasmine tea.",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    for index in 0..300 {
        let id = MemoryId::generate();
        let outcome = store
            .commit_memory_change(commit(
                None,
                learning_change(
                    companion,
                    MemoryTarget::New { id },
                    &format!("filler memory {index}"),
                    ChangeKind::Initial,
                    false,
                ),
            ))
            .await;
        assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    }
    let suppressed = MemoryId::generate();
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: suppressed },
                "suppressed jasmine note",
                ChangeKind::Initial,
                true,
            ),
        ))
        .await;
    assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));

    let candidates = store
        .recall_candidates(companion, &[String::from("jasmine")], 3)
        .await
        .unwrap();
    assert!(
        candidates.len() <= 9,
        "three arms of at most three rows each, got {}",
        candidates.len()
    );
    assert!(
        candidates
            .iter()
            .any(|memory| memory.content.contains("jasmine tea")),
        "the oldest relevant memory must be a candidate"
    );
    assert!(
        candidates.iter().all(|memory| !memory.recall_suppressed),
        "suppressed rows never consume candidate slots"
    );

    let generic = store.recall_candidates(companion, &[], 3).await.unwrap();
    assert!(
        generic.len() <= 9 && !generic.is_empty(),
        "an empty query still answers bounded background, got {}",
        generic.len()
    );
}

#[tokio::test]
async fn startup_sweep_fails_closed_when_a_registered_value_is_unreadable() {
    let store = open_memory().await.unwrap();
    let Some((companion, generation)) = running_companion(&store).await else {
        panic!("the running companion must resolve");
    };
    let readable = CredentialRef::new("openai", "main").unwrap();
    let unreadable = CredentialRef::new("openai", "other").unwrap();
    approve_pair(
        &store,
        "openai",
        "main",
        "sk-legacy",
        "reg-startup-readable",
    )
    .await;
    approve_pair(
        &store,
        "openai",
        "other",
        "sk-other",
        "reg-startup-unreadable",
    )
    .await;
    store
        .append_message(history_command(
            companion,
            generation,
            "the old key is sk-legacy",
        ))
        .await
        .unwrap();
    let premise = store.current_set_revision().await.unwrap();

    let values = MemoryCredentialStore::new();
    values.insert(readable.clone(), "sk-legacy");
    assert!(
        store
            .sweep_registered_values(&[readable.clone(), unreadable], &values)
            .is_err(),
        "an unreadable registered value must fail the startup boundary"
    );
    assert_eq!(
        store.current_set_revision().await,
        Ok(premise),
        "a failed sweep must not advance the credential-set revision"
    );
    let timeline = store.load_recent_timeline(companion, 10).await.unwrap();
    assert_eq!(
        timeline[0].text, "the old key is sk-legacy",
        "a failed sweep must leave the earlier replacement rolled back"
    );

    store
        .sweep_registered_values(&[readable], &values)
        .expect("a readable boundary must complete");
    assert!(
        store.current_set_revision().await.unwrap() > premise,
        "the successful boundary advances the revision"
    );
    let timeline = store.load_recent_timeline(companion, 10).await.unwrap();
    assert_eq!(timeline[0].text, "the old key is [credential]");
}

#[tokio::test]
async fn rotation_between_scrub_and_provider_claim_refuses_and_a_rescrub_claims() {
    use ene_credential::{CredentialScrubber, SecretScrubber as _};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scrub-claim.db");
    let store = Store::open(&path).await.unwrap();
    let values = MemoryCredentialStore::new();
    let credential = CredentialRef::new("openai", "main").unwrap();
    values.insert(credential.clone(), "sk-a");
    approve_pair(&store, "openai", "main", "sk-a", "reg-scrub-claim").await;
    let seeded = save_consent(
        &store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(seeded, ConsentCommitOutcome::Committed { .. }));

    let scrubber = CredentialScrubber {
        refs: &store,
        store: &values,
    };
    let stale_proof = scrubber
        .scrub("my key is sk-b")
        .await
        .expect("the registry is readable");
    assert_eq!(
        stale_proof.credential_set(),
        store.current_set_revision().await.unwrap(),
        "the proof names the revision it was scrubbed under"
    );
    assert!(
        stale_proof.text().contains("sk-b"),
        "a value that is not registered yet stays ordinary text"
    );

    assert!(matches!(
        store.approve_credential_with_sweep("openai", "main", "sk-b"),
        Ok(true)
    ));
    values.insert(credential, "sk-b");

    let stale = store
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: stale_proof.credential_set(),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            data_use: Vec::new(),
            task_agent: None,
            pricing: None,
            usage_estimate: None,
        })
        .await;
    assert_eq!(
        stale,
        Ok(AttemptBeginOutcome::Stale),
        "the stale premise must refuse the claim after the rotation"
    );
    assert!(
        !format!("{stale:?}").contains("sk-b"),
        "the stale refusal carries no secret material"
    );
    assert_eq!(
        task_table_count(&store, "inference_attempt"),
        0,
        "a refused claim starts no attempt"
    );

    let fresh_proof = scrubber
        .scrub("my key is sk-b")
        .await
        .expect("the registry is readable");
    assert_eq!(fresh_proof.text(), "my key is [credential]");
    assert_eq!(
        fresh_proof.credential_set(),
        store.current_set_revision().await.unwrap()
    );
    let fresh = store
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: fresh_proof.credential_set(),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            data_use: Vec::new(),
            task_agent: None,
            pricing: None,
            usage_estimate: None,
        })
        .await;
    assert_eq!(
        fresh,
        Ok(AttemptBeginOutcome::Started),
        "the re-scrubbed proof claims the send"
    );
}

fn task_table_count(store: &Store, table: &str) -> i64 {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
            row.get(0)
        })
        .expect("the row count must read")
}

fn task_workspace(folder: &str, save_target: Option<&str>) -> WorkspaceAssociationPremise {
    WorkspaceAssociationPremise {
        assoc: WorkspaceAssocId::generate(),
        need: WorkspaceNeedRef {
            folder: WorkspaceFolderRef {
                path: folder.to_owned(),
            },
            save_target: save_target.map(|path| WorkspaceFolderRef {
                path: path.to_owned(),
            }),
        },
    }
}

fn task_premise(workspace: Option<WorkspaceAssociationPremise>) -> TaskCreationPremise {
    TaskCreationPremise {
        task: TaskId::generate(),
        purpose: TaskPurpose {
            text: String::from("write the AU2 slice"),
        },
        entry: TaskContextEntryId::generate(),
        origin: TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source: RawId::new(),
        },
        acquired_at: fixture_clock(),
        assignee: AssigneeRef {
            companion: RawId::new(),
        },
        workspace,
    }
}

fn delegation_scope(workspace: Option<DelegatedWorkspace>) -> DelegationScope {
    DelegationScope { workspace }
}

fn delegated_workspace(
    assoc: WorkspaceAssocId,
    folder: &str,
    save_target: Option<&str>,
) -> DelegatedWorkspace {
    DelegatedWorkspace {
        assoc,
        folder: WorkspaceFolderRef {
            path: folder.to_owned(),
        },
        save_target: save_target.map(|path| WorkspaceFolderRef {
            path: path.to_owned(),
        }),
    }
}

fn delegation_premise(
    delegation: DelegationId,
    task: TaskRef,
    agent: TaskAgentEphemeralId,
    scope_copy: DelegationScope,
) -> DelegationCreationPremise {
    DelegationCreationPremise {
        delegation,
        task,
        agent,
        scope_copy,
    }
}

struct EmptyRevisionRegistry(CredentialSetRevision);

impl CredentialRefRepository for EmptyRevisionRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        Ok(Vec::new())
    }
}

impl CredentialSetRepository for EmptyRevisionRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn current_set_revision(
        &self,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
        Ok(self.0)
    }
}

async fn scrubbed_result_at(revision: CredentialSetRevision, text: &str) -> TaskResultScrubPremise {
    use ene_credential::SecretScrubber as _;

    let registry = EmptyRevisionRegistry(revision);
    let values = MemoryCredentialStore::new();
    TaskResultScrubPremise::from_scrubbed(
        ene_credential::CredentialScrubber {
            refs: &registry,
            store: &values,
        }
        .scrub(text)
        .await
        .expect("the empty fixture registry is readable"),
    )
}

async fn scrubbed_result(store: &Store, text: &str) -> TaskResultScrubPremise {
    let revision = store
        .current_set_revision()
        .await
        .expect("the fixture credential-set revision reads");
    scrubbed_result_at(revision, text).await
}

async fn record_result(store: &Store, delegation: DelegationId, text: &str) -> TaskResultRecord {
    match orchestrate_result_arrival(store, delegation, scrubbed_result(store, text).await)
        .await
        .expect("the arrival must answer")
    {
        TaskResultArrivalOutcome::Recorded(record) => record,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("the fixture scrubbed at the current revision")
        }
    }
}

mod report_reads;
mod task_result;
mod usage_query;
