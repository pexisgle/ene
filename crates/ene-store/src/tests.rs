use crate::Store;
mod publication;
use ene_companion::{
    ActivityId, ActivityRepository as _, AppendHistoryCommand, CompanionId, CompanionLifecycle,
    CompanionRepository, CompanionTechnicalError, HistoryAppendOutcome, HistoryRepository,
    HistoryRole, RecordResumeActivityCommand, ResumeActivityOutcome, UndeliveredRepository,
};
use ene_credential::{
    CredentialApprovalRepository, CredentialIntentRepository as _, CredentialRef,
    CredentialRefRepository, CredentialSetRepository, CredentialSetRevision,
    CredentialTechnicalError, DevicePairingRepository, MemoryCredentialStore, RegistrationApply,
    RegistrationFingerprint, RegistrationState,
};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _,
    InferenceTechnicalError, InferenceTicketId, TaskAgentAttemptPremise, UsageFact,
    UsageRepository, UsageSource,
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
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceCheckRef,
    PresenceGeneration, PresenceRepository, PresenceState, ThinMoveReason,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationScope, TaskAgentEphemeralId, TaskCommitOutcome, TaskCommitPremise,
    TaskContextEntryId, TaskContextItem, TaskContextOrigin, TaskContextOriginKind,
    TaskCreationPremise, TaskId, TaskInstructionAdoptionPremise, TaskPurpose,
    TaskPurposeAdoptionPremise, TaskPurposeRef, TaskRef, TaskRepository, TaskResultArrivalOutcome,
    TaskResultRecord, TaskResultScrubPremise, TaskRevision, TaskTechnicalError, WorkspaceAssocId,
    WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef, orchestrate_result_arrival,
};
use rusqlite::OptionalExtension;
use rusqlite::params;

fn fixture_clock() -> WallClockWithTz {
    if let Ok(at) = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00") {
        at
    } else {
        WallClockWithTz::now()
    }
}

/// Builds the durable fingerprint for one credential-registration intent.
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

/// Registers one pair through the production register-then-approve path: the
/// registration intent records the pending row and the approval write creates
/// the usable ref with the sweep.
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

/// Records one resume-instruction activity and returns its identity.
///
/// The A4 held path (`HeldForErasure`) is exercised by its own boundary
/// tests; this helper is the committed path the existing AU17 coverage
/// expects, and reports a held outcome as an unavailability so those legacy
/// assertions keep their `Result` shape without conflating the outcome in
/// production code.
async fn record_activity_id(
    store: &Store,
    cmd: RecordResumeActivityCommand,
) -> Result<ActivityId, CompanionTechnicalError> {
    match store.record_resume_activity(cmd).await {
        Ok(ResumeActivityOutcome::Recorded(activity)) => Ok(activity),
        Ok(ResumeActivityOutcome::HeldForErasure) => {
            Err(CompanionTechnicalError::StorageUnavailable {
                reason: String::from("activity held for erasure"),
            })
        }
        Err(error) => Err(error),
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

fn lock_for_test(store: &Store) -> rusqlite::Result<i64> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.query_row("SELECT 1", (), |row| row.get(0))
}

#[tokio::test]
async fn reopen_is_idempotent_and_seeds_running_companion() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    let first = opened.unwrap();
    let ensured = first.ensure_running_companion().await;
    let companion = ensured.unwrap();
    let lifecycle = first.load_lifecycle(companion).await;
    assert!(
        matches!(lifecycle, Ok(Some(CompanionLifecycle::Running))),
        "seeded companion must run"
    );
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let ensured_again = second.ensure_running_companion().await;
    let same = ensured_again.unwrap();
    assert_eq!(same, companion, "seed must be idempotent");
    let ping = lock_for_test(&second);
    assert!(ping.is_ok(), "reopened handle must serve queries");
}

#[tokio::test]
async fn append_with_stale_generation_is_rejected() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let stale_number = generation.as_u64() + 1;
    let appended = store
        .append_message(history_command(
            companion,
            PresenceGeneration::from_u64(stale_number),
            "stale body",
        ))
        .await;
    let outcome = appended.unwrap();
    assert_eq!(
        outcome,
        HistoryAppendOutcome::StaleExpected {
            current: generation
        },
        "stale expectation must carry the current generation"
    );
    let loaded = store.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert!(timeline.is_empty(), "stale append must store nothing");
}

/// A reply naming a superseded Owner message is refused inside the same
/// transaction that would insert it: the newer accepted Owner input wins
/// by commit order, even inside one round, and the refused reply registers
/// nothing.
#[tokio::test]
async fn reply_after_newer_owner_input_is_stale() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let HistoryAppendOutcome::CommittedAs { message: owner1 } = store
        .append_message(history_command(companion, generation, "first"))
        .await
        .unwrap()
    else {
        panic!("the first owner input must commit");
    };
    let HistoryAppendOutcome::CommittedAs { message: owner2 } = store
        .append_message(history_command(companion, generation, "second"))
        .await
        .unwrap()
    else {
        panic!("the second owner input must commit");
    };
    let mut stale_reply = history_command(companion, generation, "stale reply");
    stale_reply.role = HistoryRole::Companion;
    stale_reply.expected_owner_message = Some(owner1);
    let (outcome, registered) = store
        .append_reply_with_undelivered(stale_reply, true, None)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        HistoryAppendOutcome::StaleOwnerInput,
        "a reply to a superseded owner must not adopt"
    );
    assert!(
        registered.is_none(),
        "a refused reply registers no undelivered entry"
    );
    let mut current_reply = history_command(companion, generation, "current reply");
    current_reply.role = HistoryRole::Companion;
    current_reply.expected_owner_message = Some(owner2);
    let (outcome, _) = store
        .append_reply_with_undelivered(current_reply, true, None)
        .await
        .unwrap();
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "a reply to the latest owner still commits, got {outcome:?}"
    );
    let loaded = store
        .load_timeline(companion, None, None, 10)
        .await
        .unwrap();
    assert_eq!(
        loaded.len(),
        3,
        "two owners plus the current reply, never the stale one"
    );
    assert!(
        loaded.iter().all(|item| item.text != "stale reply"),
        "the refused reply leaves no row"
    );
}

/// The supersession probe is an index seek, not a History scan: it stops
/// at the first newer Owner row, so proving recency costs one index step
/// in the common current case no matter how long the timeline grows.
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

/// Commits one consent row through the intent-atomic write path, minting a
/// fresh intent id per call so nothing replays.
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

    // The implemented query is a primary-key point lookup, not a scan: the
    // plan proves the read stays bounded to the addressed row.
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

    // A malformed durable row is a technical error, never a composed
    // substitute.
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
async fn credential_registration_pends_until_approved() {
    let store = open_memory().await.unwrap();
    let unknown = store
        .approve_credential_with_sweep("acme", "nope", "sk-nope")
        .expect("an unknown pair is not an error");
    assert!(!unknown, "approving an unknown pair must yield false");
    let listed_empty = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed_empty, Ok(ref items) if items.is_empty()),
        "fresh store pends no credential approvals"
    );
    let requested = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-cycle-1", "acme", "main"),
        )
        .await;
    assert_eq!(
        requested,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "first request must record a pending entry"
    );
    let repeated = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-cycle-2", "acme", "main"),
        )
        .await;
    assert_eq!(
        repeated,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "a repeat request must not double-record the pair"
    );
    let listed = CredentialApprovalRepository::list_pending(&store).await;
    let items = listed.unwrap();
    assert_eq!(items.len(), 1, "one approval must pend");
    assert_eq!(items[0].provider.as_str(), "acme");
    assert_eq!(items[0].label.as_str(), "main");
    let refs_before = store.list_refs().await;
    assert!(
        matches!(refs_before, Ok(ref refs) if refs.is_empty()),
        "a pending-only pair must not read as usable"
    );
    let approved = store
        .approve_credential_with_sweep("acme", "main", "sk-main")
        .expect("approval must commit");
    assert!(approved, "approval of a pending pair must succeed");
    let drained = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(drained, Ok(ref items) if items.is_empty()),
        "approval must drain the pending entry"
    );
    let refs = store.list_refs().await.unwrap();
    assert_eq!(
        refs,
        vec![CredentialRef::new("acme", "main").expect("valid test fixture")],
        "approval atomically records the usable ref in the same transaction"
    );
    let reapproved = store
        .approve_credential_with_sweep("acme", "main", "sk-main")
        .expect("re-approval must commit");
    assert!(reapproved, "re-approving a usable pair must stay true");
    let again = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-cycle-3", "acme", "main"),
        )
        .await;
    assert_eq!(
        again,
        Ok(RegistrationApply::Decided(
            RegistrationState::AppliedAsOneTime
        )),
        "request after usable must answer from the usable ref"
    );
    let listed_after = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed_after, Ok(ref items) if items.is_empty()),
        "usable pair must never linger as pending"
    );
}

#[tokio::test]
async fn assign_with_intent_commits_marker_atomically() {
    use ene_permission::{
        ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint, IntentOutcome,
        IntentOutcomeRecord, IntentOutcomeRepository as _, IntentResolution,
    };

    fn intent() -> IntentOutcomeRecord {
        IntentOutcomeRecord {
            fingerprint: IntentFingerprint {
                intent_id: String::from("assign-1"),
                kind: String::from("assign"),
                target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
                base: String::from("consent-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
            outcome: IntentOutcome::StoredAsRuleView {
                revision: String::from("consent-dialogue-rev-1"),
            },
        }
    }

    let store = open_memory().await.unwrap();
    let committed = store
        .assign_with_intent(
            None,
            ConsentRecord {
                capability: CapabilityKind::Dialogue,
                id: String::from("consent-1"),
                rev: ConsentRevision::from_u64(1),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                credential_id: String::from("openai:main"),
            },
            intent().fingerprint,
        )
        .await;
    assert!(
        matches!(
            committed,
            Ok(IntentResolution::Decided(
                ConsentCommitOutcome::Committed { .. }
            ))
        ),
        "fresh assign must commit, got {committed:?}"
    );
    let found = store.lookup_intent_outcome("assign-1").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == intent()),
        "commit must leave its replay row, got {found:?}"
    );
    let stale = store
        .assign_with_intent(
            None,
            ConsentRecord {
                capability: CapabilityKind::Dialogue,
                id: String::from("consent-1"),
                rev: ConsentRevision::from_u64(2),
                provider: String::from("openai"),
                model: String::from("dialogue-2"),
                credential_id: String::from("openai:main"),
            },
            IntentFingerprint {
                intent_id: String::from("assign-2"),
                ..intent().fingerprint
            },
        )
        .await;
    assert!(
        matches!(
            stale,
            Ok(IntentResolution::Decided(
                ConsentCommitOutcome::StaleCurrent { .. }
            ))
        ),
        "stale assign must not commit, got {stale:?}"
    );
    let stale_row = store.lookup_intent_outcome("assign-2").await;
    assert!(
        matches!(
            stale_row,
            Ok(Some(ref stored))
                if stored.outcome
                    == IntentOutcome::StaleBaseView {
                        current: String::from("consent-dialogue-rev-1"),
                    }
        ),
        "stale assign must leave its stale snapshot, got {stale_row:?}"
    );
    // Same id and fingerprint: replays without re-running compare-and-save
    // or touching consent.
    let replayed = store
        .assign_with_intent(
            None,
            ConsentRecord {
                capability: CapabilityKind::Dialogue,
                id: String::from("consent-9"),
                rev: ConsentRevision::from_u64(9),
                provider: String::from("other"),
                model: String::from("other"),
                credential_id: String::from("other"),
            },
            intent().fingerprint,
        )
        .await;
    assert!(
        matches!(
            replayed,
            Ok(IntentResolution::Replay(ref stored)) if *stored == intent()
        ),
        "same-id retry must replay, got {replayed:?}"
    );
    let conflicted = store
        .assign_with_intent(
            None,
            ConsentRecord {
                capability: CapabilityKind::Dialogue,
                id: String::from("consent-9"),
                rev: ConsentRevision::from_u64(9),
                provider: String::from("other"),
                model: String::from("other"),
                credential_id: String::from("other"),
            },
            IntentFingerprint {
                target: String::from("consent:dialogue:openai:changed:openai:main"),
                ..intent().fingerprint
            },
        )
        .await;
    assert!(
        matches!(
            conflicted,
            Ok(IntentResolution::Conflict(ref stored)) if *stored == intent()
        ),
        "same-id reuse must conflict, got {conflicted:?}"
    );
    let preserved = store.lookup_intent_outcome("assign-1").await;
    assert!(
        matches!(preserved, Ok(Some(ref stored)) if *stored == intent()),
        "conflict must not rewrite the row, got {preserved:?}"
    );
    let timeline = store.load_current(CapabilityKind::Dialogue).await;
    assert!(
        matches!(
            timeline,
            Ok(Some(ref record))
                if record.rev == ConsentRevision::from_u64(1)
                    && record.model == "dialogue-1"
        ),
        "conflict must not move consent, got {timeline:?}"
    );
}

#[tokio::test]
async fn concurrent_same_id_assigns_fork_nothing() {
    use ene_permission::{
        ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcomeRepository as _,
        IntentResolution,
    };

    let store = open_memory().await.unwrap();
    // Two sends of one logical intent race through the same store: the
    // shared-connection mutex serializes whole transactions, so exactly one
    // decides and the loser replays — the answer never forks and the row is
    // never rewritten.
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

// --- Learning: Memory / Summary / revision persistence ---

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

/// Revision pages bound the rows read and still cover every revision exactly
/// once; a shared Summary is one row in the batch read, and a missing id
/// stays absent instead of being fabricated.
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

/// Recall candidate retrieval is a bounded multi-arm query: an old relevant
/// memory is reachable past any newest window, suppressed rows never become
/// candidates, and each arm caps the rows read.
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
async fn learning_update_appends_a_revision_and_keeps_the_previous_one() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let first = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner lives in Tokyo",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(first, Ok(MemoryChangeOutcome::Committed { .. })));

    let second = store
        .commit_memory_change(commit(
            Some(learning_summary(companion, "owner moved to Osaka")),
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "owner lives in Osaka",
                ChangeKind::ChangedSince,
                false,
            ),
        ))
        .await;
    assert_eq!(
        second,
        Ok(MemoryChangeOutcome::Committed {
            memory,
            revision: MemoryRevision::from_u64(2),
        })
    );
    let memories = store
        .list_current_memories(companion, None, 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1, "the memory must be current");
    assert_eq!(memories[0].content, "owner lives in Osaka");
    assert_eq!(memories[0].revision, MemoryRevision::from_u64(2));

    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 2, "the old revision is kept");
    assert_eq!(revisions[0].content, "owner lives in Tokyo");
    assert_eq!(revisions[1].content, "owner lives in Osaka");
    assert_eq!(revisions[1].change, ChangeKind::ChangedSince);
}

#[tokio::test]
async fn learning_forgetting_suppresses_recall_and_keeps_content_and_revisions() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "a shared worry");
    let seeded = store
        .commit_memory_change(commit(
            Some(evidence),
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner was worried about the launch",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(seeded, Ok(MemoryChangeOutcome::Committed { .. })));
    let forgotten = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "owner was worried about the launch",
                ChangeKind::Forgotten,
                true,
            ),
        ))
        .await;
    assert!(matches!(
        forgotten,
        Ok(MemoryChangeOutcome::Committed { .. })
    ));
    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 2, "revision history is kept");
    assert_eq!(revisions[0].content, "owner was worried about the launch");
    assert!(revisions[1].recall_suppressed, "recall is suppressed");
    assert_eq!(revisions[1].change, ChangeKind::Forgotten);

    // A later reinforcement clears the suppression without deleting it.
    let remembered = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::from_u64(2),
                },
                "owner was worried about the launch",
                ChangeKind::Reinforced,
                false,
            ),
        ))
        .await;
    assert!(matches!(
        remembered,
        Ok(MemoryChangeOutcome::Committed { .. })
    ));
    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert!(!revisions[2].recall_suppressed);
    assert_eq!(revisions.len(), 3);
}

#[tokio::test]
async fn approval_sweep_redacts_history_and_learning_content() {
    let store = open_memory().await.unwrap();
    let Some((companion, generation)) = running_companion(&store).await else {
        panic!("the running companion must resolve");
    };
    store
        .append_message(history_command(
            companion,
            generation,
            "the key is sk-test-only",
        ))
        .await
        .unwrap();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion.as_raw(), "evidence mentions sk-test-only");
    let committed = store
        .commit_memory_change(commit(
            Some(evidence.clone()),
            learning_change(
                companion.as_raw(),
                MemoryTarget::New { id: memory },
                "owner key sk-test-only",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(
        committed,
        Ok(MemoryChangeOutcome::Committed { .. })
    ));
    approve_pair(
        &store,
        "openai",
        "main",
        "sk-test-only",
        "reg-sweep-content",
    )
    .await;

    let timeline = store.load_recent_timeline(companion, 10).await.unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].text, "the key is [credential]");
    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(revisions[0].content, "owner key [credential]");
    let stored = store
        .load_summaries(&[evidence.id])
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(stored.content, "evidence mentions [credential]");
}

/// The approval sweep also covers the Task and activity bodies that the Task
/// report, management view, and undelivered excerpts read back: a value
/// recorded as ordinary text before it became a registered credential must
/// be redacted in the current Task revision, the revision history, the
/// recorded final result, and the first-party resume instruction, or those
/// readers would keep serving the raw value out of the owner row.
#[tokio::test]
async fn approval_sweep_redacts_task_and_activity_bodies() {
    use ene_companion::{
        ActivityRepository as _, RecordResumeActivityCommand, TaskFact, UndeliveredSource,
    };
    use ene_task::TaskReportSourceRef;

    let secret = "sk-sweep-task-body";
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: format!("the old key is {secret}"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: fixture_clock(),
            assignee: AssigneeRef { companion },
            workspace: None,
        })
        .await
        .expect("the task must commit");
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(ene_task::TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: format!("now the key is {secret}"),
                },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: fixture_clock(),
            }),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("the steering must answer");
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the purpose update must commit, got {advanced:?}");
    };
    let delegation = DelegationId::generate();
    assert!(matches!(
        store
            .create_delegation(DelegationCreationPremise {
                delegation,
                task: current,
                agent: TaskAgentEphemeralId::generate(),
                scope_copy: DelegationScope { workspace: None },
            })
            .await
            .expect("the delegation must commit"),
        DelegationOutcome::Delegated(_)
    ));
    let arrival = record_result(
        &store,
        delegation,
        &format!("final report mentions {secret}"),
    )
    .await;
    let activity = record_activity_id(
        &store,
        RecordResumeActivityCommand {
            companion: CompanionId::from_raw(companion),
            task: current,
            purpose: TaskPurposeRef {
                task: current.task,
                adopted_revision: TaskRevision::initial(),
            },
            body: format!("continue from the key {secret}"),
            command: RawId::new(),
        },
    )
    .await
    .expect("the activity must record");

    approve_pair(&store, "openai", "main", secret, "reg-sweep-task-body").await;

    let record = store
        .load_task(current.task)
        .await
        .expect("the task must load")
        .expect("the task still exists");
    assert!(
        !record.revision.purpose_text.text.contains(secret),
        "the current revision purpose is swept: {}",
        record.revision.purpose_text.text
    );
    assert!(
        record.revision.purpose_text.text.contains("[credential]"),
        "the swept position stays visible: {}",
        record.revision.purpose_text.text
    );
    let original = store
        .load_report_source_bounded(
            TaskReportSourceRef::RevisionPurpose {
                task: current.task,
                revision: TaskRevision::initial(),
            },
            0,
            4096,
        )
        .await
        .expect("the revision history must read")
        .expect("the original revision is retained");
    assert!(
        !original.text.contains(secret),
        "the previous revision purpose is swept: {}",
        original.text
    );
    let result = store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(arrival.result), 0, 4096)
        .await
        .expect("the result body must read")
        .expect("the result row exists");
    assert!(
        !result.text.contains(secret),
        "the recorded result body is swept: {}",
        result.text
    );
    let stored_activity = store
        .load_activity(activity)
        .await
        .expect("the activity must load")
        .expect("the activity still exists");
    assert!(
        !stored_activity.body.contains(secret),
        "the resume instruction body is swept: {}",
        stored_activity.body
    );
    let excerpt = store
        .load_undelivered_excerpt(
            UndeliveredSource::TaskRecord {
                task: current.task.as_raw(),
                fact: TaskFact::ResultRecorded(arrival.result.as_raw()),
            },
            4096,
        )
        .await
        .expect("the excerpt must read")
        .expect("the result source carries a bounded body");
    assert!(
        !excerpt.text.contains(secret),
        "the presentation-facing excerpt reads the swept row: {}",
        excerpt.text
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
    // Register both refs first, then store raw content: the boundary under
    // test is the startup sweep below.
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

    // `readable` sweeps first inside the transaction, then `unreadable`
    // fails: the whole boundary must roll back, not keep a partial sweep or
    // a revision advance.
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

    // With every value readable the same boundary sweeps and advances.
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

/// The provider claim is the send boundary of the credential premise: a proof
/// produced by the real scrub boundary before a rotation must be refused with
/// zero provider bytes, and only a re-scrub under the new revision may claim.
/// The refusal itself carries no secret material.
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

    // The value changes between the scrub and the claim: the request builder
    // now carries the rotated bearer while the old proof names the old set.
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

    // Only the re-scrubbed proof claims: its text has the newly registered
    // value redacted and its premise names the current set.
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

// --- Task: AU2 creation / reload ---

/// Counts rows of one test-probed table. The table name is a literal from
/// this test module, never caller input.
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

// --- Task: AU4 helpers ---

pub(super) fn task_purpose_adoption(text: &str) -> TaskPurposeAdoptionPremise {
    TaskPurposeAdoptionPremise {
        purpose: TaskPurpose {
            text: text.to_owned(),
        },
        origin: TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source: RawId::new(),
        },
        acquired_at: fixture_clock(),
    }
}

fn task_instruction_adoption(source: RawId) -> TaskInstructionAdoptionPremise {
    TaskInstructionAdoptionPremise {
        entry: TaskContextEntryId::generate(),
        origin: TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source,
        },
        acquired_at: fixture_clock(),
    }
}

fn task_revision_rows(store: &Store, task: TaskId) -> Vec<(i64, i64, String)> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut statement = guard
        .prepare(
            "SELECT revision, purpose_adopted_revision, purpose_text FROM task_revision WHERE task_id = ?1 ORDER BY revision",
        )
        .expect("the revision probe must prepare");
    statement
        .query_map(params![crate::codec::encode_id(task.as_raw())], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("the revision probe must query")
        .collect::<Result<Vec<_>, _>>()
        .expect("the revision probe must read")
}

type ContextRow = (i64, Option<i64>, String, String, String, String, String);

fn task_context_rows(store: &Store, task: TaskId) -> Vec<ContextRow> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut statement = guard
        .prepare(
            "SELECT revision, purpose_adopted_revision, entry_id, item_kind, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 ORDER BY rowid",
        )
        .expect("the context probe must prepare");
    statement
        .query_map(params![crate::codec::encode_id(task.as_raw())], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .expect("the context probe must query")
        .collect::<Result<Vec<_>, _>>()
        .expect("the context probe must read")
}

// --- Task: AU4 adopted instructions ---

#[tokio::test]
async fn task_instruction_adoption_round_trips_at_one_revision() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    let adoption = task_purpose_adoption("purpose with instruction");
    let purpose_entry = TaskContextEntryId::generate();
    let instruction = task_instruction_adoption(RawId::new());
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(adoption.clone()),
            adopted_purpose_entry: purpose_entry,
            adopted_instruction: Some(instruction.clone()),
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(committed) = outcome else {
        panic!("expected CommittedAs, got {outcome:?}");
    };
    assert_eq!(committed.revision, TaskRevision::from_u64(2));

    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.reference, committed);
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: created.task,
            adopted_revision: committed.revision,
        }
    );
    assert_eq!(
        record.context.len(),
        2,
        "the purpose entry comes first, then the instruction entry"
    );
    let purpose = &record.context[0];
    assert_eq!(purpose.entry, purpose_entry);
    assert_eq!(purpose.reference, committed);
    assert_eq!(
        purpose.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(purpose.origin, adoption.origin);
    let adopted = &record.context[1];
    assert_eq!(
        adopted.entry, instruction.entry,
        "the repository persists the caller-minted instruction identity"
    );
    assert_eq!(
        adopted.reference, committed,
        "the instruction belongs to the forward's revision"
    );
    assert_eq!(adopted.item, TaskContextItem::AdoptedInstruction);
    assert_eq!(adopted.origin, instruction.origin);
    assert_eq!(
        adopted.acquired_at.to_rfc3339(),
        instruction.acquired_at.to_rfc3339()
    );

    let rows = task_context_rows(&store, created.task);
    assert_eq!(
        rows.len(),
        3,
        "the AU2 purpose entry plus the forward's purpose and instruction entries"
    );
    assert_eq!(rows[1].0, 2);
    assert_eq!(rows[1].1, Some(2));
    assert_eq!(rows[1].2, crate::codec::encode_id(purpose_entry.as_raw()));
    assert_eq!(rows[1].3, "adopted_purpose");
    assert_eq!(rows[2].0, 2);
    assert_eq!(
        rows[2].1, None,
        "an instruction entry carries no purpose payload"
    );
    assert_eq!(
        rows[2].2,
        crate::codec::encode_id(instruction.entry.as_raw())
    );
    assert_eq!(rows[2].3, "adopted_instruction");
    assert_eq!(rows[2].4, "owner_conversation");
    assert_eq!(
        rows[2].5,
        crate::codec::encode_id(instruction.origin.source)
    );
    assert_eq!(rows[2].6, instruction.acquired_at.to_rfc3339());
    assert_eq!(
        task_revision_rows(&store, created.task).len(),
        2,
        "the old revision snapshot is retained"
    );
}

// --- Task: context item-kind read rules ---

// --- Delegation: AU3 creation / reload ---

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

/// A readable credential registry with no refs, pinned at one revision.
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

/// Scrubs `text` through the credential-owned boundary under `revision`; the
/// returned premise is the only way a test can name a Task result body.
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

/// Scrubs `text` under the store's current credential-set revision.
async fn scrubbed_result(store: &Store, text: &str) -> TaskResultScrubPremise {
    let revision = store
        .current_set_revision()
        .await
        .expect("the fixture credential-set revision reads");
    scrubbed_result_at(revision, text).await
}

/// Records one result through the production arrival boundary and returns the
/// recorded result. Fixtures scrub at the current revision, so a stale
/// refusal here would be a fixture error.
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

mod action;
mod agent;
mod cancel;
mod client_delivery;
mod completion;
mod delayed_arrival;
mod deletion_request;
mod erasure;
mod erasure_owners;
mod participant_erasure;
mod presence;
mod preservation;
mod report_reads;
mod result_reevaluation;
mod resume;
mod source_reconciliation;
mod targeted_deletion;
mod task_failure;
mod task_result;
mod undelivered;
mod usage;
mod usage_cap;
mod usage_query;
