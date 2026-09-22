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
    WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
        .expect("fixture timestamp must parse")
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
        base: String::from("consent-dialogue-none"),
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

#[tokio::test]
async fn append_with_moved_consent_is_rejected() {
    use ene_companion::HistoryRepository as _;

    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let mut cmd = history_command(companion, generation, "consent body");
    cmd.expected_consent = Some((String::from("consent-1"), 999));
    let appended = store.append_message(cmd).await;
    let outcome = appended.unwrap();
    assert_eq!(
        outcome,
        HistoryAppendOutcome::StaleConsent,
        "moved consent must answer stale-consent"
    );
    let loaded = store.load_timeline(companion, None, None, 10).await;
    assert!(
        matches!(&loaded, Ok(items) if items.is_empty()),
        "stale-consent append must store nothing"
    );
}

#[tokio::test]
async fn append_while_stopped_is_held_by_lifecycle() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let stopped = guard.execute(
            "UPDATE companion SET lifecycle = ?1 WHERE companion_id = ?2",
            params![
                crate::codec::encode_lifecycle(CompanionLifecycle::Stopped),
                crate::codec::encode_id(companion.as_raw())
            ],
        );
        assert!(stopped.is_ok(), "lifecycle update must succeed");
    }
    let appended = store
        .append_message(history_command(companion, generation, "held body"))
        .await;
    let outcome = appended.unwrap();
    assert_eq!(
        outcome,
        HistoryAppendOutcome::HeldByLifecycle {
            lifecycle: CompanionLifecycle::Stopped
        },
        "stopped companion must hold appends"
    );
}

#[tokio::test]
async fn undelivered_register_mark_and_stale_mark() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let appended = store
        .append_reply_with_undelivered(
            history_command(companion, generation, "reply body"),
            true,
            None,
        )
        .await;
    let (outcome, registered) = appended.unwrap();
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "reply must commit"
    );
    let entry = registered.expect("registration must return the entry");
    assert_eq!(entry.status, ReportStatus::Pending);
    assert!(entry.round.is_some(), "conversation sources carry a round");
    assert!(entry.presence_generation.is_some());
    let round = entry.round.unwrap();

    let listed = store
        .list_unpresented(companion, None, ene_companion::UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1, "one entry must be unpresented");
    assert_eq!(listed.entries[0].id, entry.id);
    assert_eq!(listed.next, None, "a short page drains the pass");

    // Presentation start: Pending -> PresentationUnknown, still re-listed.
    let started = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::Pending,
            PresentationMark {
                round,
                presented: false,
            },
        )
        .await;
    assert_eq!(
        started,
        Ok(ReportStatusTransition::MarkedPresentationUnknown)
    );
    let relisted = store
        .list_unpresented(companion, None, ene_companion::UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert_eq!(relisted.entries.len(), 1, "Unknown must be re-listed");
    assert_eq!(
        relisted.entries[0].status,
        ReportStatus::PresentationUnknown
    );

    // A current not-presented receipt returns the row to Pending.
    let failed = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round,
                presented: false,
            },
        )
        .await;
    assert_eq!(failed, Ok(ReportStatusTransition::FailedToPending));

    // A mismatched expected status on the pending row is stale.
    let stale = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round,
                presented: true,
            },
        )
        .await;
    assert_eq!(
        stale,
        Ok(ReportStatusTransition::StaleSource),
        "a mismatched expected status must be stale"
    );

    // Confirmed presentation is absorbing.
    let presented = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::Pending,
            PresentationMark {
                round,
                presented: true,
            },
        )
        .await;
    assert_eq!(presented, Ok(ReportStatusTransition::PendingToPresented));
    let duplicate = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round,
                presented: true,
            },
        )
        .await;
    assert_eq!(
        duplicate,
        Ok(ReportStatusTransition::AlreadyPresented),
        "duplicate ACK writes nothing"
    );
    let downgrade = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round,
                presented: false,
            },
        )
        .await;
    assert_eq!(
        downgrade,
        Ok(ReportStatusTransition::AlreadyPresented),
        "presented absorbs a later not-presented receipt"
    );
    let drained = store
        .list_unpresented(companion, None, ene_companion::UNDELIVERED_PAGE_MAX)
        .await
        .unwrap();
    assert!(
        drained.entries.is_empty(),
        "presented entries leave the unpresented list"
    );

    let stale = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::Pending,
            PresentationMark {
                round,
                presented: true,
            },
        )
        .await;
    assert_eq!(
        stale,
        Ok(ReportStatusTransition::AlreadyPresented),
        "a presented row absorbs every later mark, stale premise included"
    );
}

#[tokio::test]
async fn presence_begin_mismatch_is_rejected_as_stale() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let raw = companion.as_raw();
    let check = PresenceCheckRef {
        expected_generation: PresenceGeneration::from_u64(generation.as_u64() + 1),
        expected_state: PresenceState::NoActive,
        expected_active: None,
    };
    let rejected = store
        .compare_and_begin_transition(
            raw,
            check,
            Some(ClientId::generate()),
            ThinMoveReason::InitialAttach,
        )
        .await;
    let decision = rejected.unwrap();
    let MoveDecision::RejectedAsStalePresence { current } = decision else {
        panic!("generation mismatch must reject as stale, got {decision:?}");
    };
    assert_eq!(current.generation, generation);
    assert_eq!(current.state, PresenceState::NoActive);
    let client = ClientId::generate();
    let begin = store
        .compare_and_begin_transition(
            raw,
            PresenceCheckRef {
                expected_generation: generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await;
    let MoveDecision::TransitioningToNew { generation: next } = begin.unwrap() else {
        panic!("unexpected variant");
    };
    let confirmed = store
        .confirm_transition(
            raw,
            next,
            LiveReachabilityRef {
                client,
                connection_live: true,
            },
        )
        .await;
    let ConfirmTransitionOutcome::Confirmed(fact) = confirmed.unwrap() else {
        panic!("unexpected variant");
    };
    assert_eq!(fact.state, PresenceState::Present);
    assert_eq!(fact.active_client, Some(client));
    assert_eq!(fact.generation, next);
}

#[tokio::test]
async fn confirm_by_unpinned_client_is_rejected_without_touching_state() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let raw = companion.as_raw();
    let pinned = ClientId::generate();
    let intruder = ClientId::generate();
    let begin = store
        .compare_and_begin_transition(
            raw,
            PresenceCheckRef {
                expected_generation: generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            Some(pinned),
            ThinMoveReason::InitialAttach,
        )
        .await;
    let MoveDecision::TransitioningToNew { generation: next } = begin.unwrap() else {
        panic!("unexpected variant");
    };
    let rejected = store
        .confirm_transition(
            raw,
            next,
            LiveReachabilityRef {
                client: intruder,
                connection_live: true,
            },
        )
        .await;
    assert!(
        matches!(
            rejected,
            Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { .. })
        ),
        "unpinned confirm must reject, got {rejected:?}"
    );
    let current = store.load_attribution(raw).await;
    assert!(
        matches!(&current, Ok(Some(fact)) if fact.state == PresenceState::InTransition
            && fact.generation == next
            && fact.active_client == Some(pinned)),
        "rejected confirm must leave the row untouched, got {current:?}"
    );
    let confirmed = store
        .confirm_transition(
            raw,
            next,
            LiveReachabilityRef {
                client: pinned,
                connection_live: true,
            },
        )
        .await;
    assert!(
        matches!(
            confirmed,
            Ok(ConfirmTransitionOutcome::Confirmed(ref fact))
                if fact.state == PresenceState::Present
                    && fact.active_client == Some(pinned)
        ),
        "pinned confirm must succeed, got {confirmed:?}"
    );
}

fn consent_record(id: &str, rev: u64) -> ConsentRecord {
    ConsentRecord {
        capability: CapabilityKind::Dialogue,
        id: String::from(id),
        rev: ConsentRevision::from_u64(rev),
        provider: String::from("acme"),
        model: String::from("dialogue-1"),
        credential_id: String::from("cred-1"),
    }
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
async fn consent_assign_with_intent_commit_and_stale_matrix() {
    let store = open_memory().await.unwrap();
    let empty = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(empty, Ok(None)), "fresh store holds no consent");
    let first = consent_record("consent-1", 3);
    let committed = save_consent(&store, None, first.clone()).await;
    assert!(
        matches!(
            committed,
            ConsentCommitOutcome::Committed { ref record } if *record == first
        ),
        "empty store with no expectation must commit"
    );
    let loaded = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(loaded, Ok(Some(ref current)) if *current == first));
    let intruder = consent_record("consent-9", 1);
    let unexpected = save_consent(&store, None, intruder).await;
    assert!(
        matches!(
            unexpected,
            ConsentCommitOutcome::StaleCurrent { ref current } if *current == Some(first.clone())
        ),
        "existing row with no expectation must be stale"
    );
    let next = consent_record("consent-1", 4);
    let recommitted = save_consent(
        &store,
        Some((String::from("consent-1"), ConsentRevision::from_u64(3))),
        next.clone(),
    )
    .await;
    assert!(
        matches!(
            recommitted,
            ConsentCommitOutcome::Committed { ref record } if *record == next
        ),
        "matching expectation must commit the replacement"
    );
    let replay = consent_record("consent-1", 5);
    let stale_rev = save_consent(
        &store,
        Some((String::from("consent-1"), ConsentRevision::from_u64(3))),
        replay,
    )
    .await;
    assert!(
        matches!(
            stale_rev,
            ConsentCommitOutcome::StaleCurrent { ref current } if *current == Some(next.clone())
        ),
        "revision mismatch must be stale"
    );
    let fork = consent_record("consent-2", 4);
    let stale_id = save_consent(
        &store,
        Some((String::from("consent-2"), ConsentRevision::from_u64(4))),
        fork,
    )
    .await;
    assert!(
        matches!(
            stale_id,
            ConsentCommitOutcome::StaleCurrent { ref current } if *current == Some(next.clone())
        ),
        "id mismatch must be stale"
    );
    let kept = store.load_current(CapabilityKind::Dialogue).await;
    assert!(
        matches!(kept, Ok(Some(ref current)) if *current == next),
        "stale attempts must leave the stored row untouched"
    );
}

#[tokio::test]
async fn consent_assign_with_intent_expected_but_empty_is_stale() {
    let store = open_memory().await.unwrap();
    let record = consent_record("consent-1", 1);
    let outcome = save_consent(
        &store,
        Some((String::from("consent-1"), ConsentRevision::from_u64(1))),
        record,
    )
    .await;
    assert!(
        matches!(
            outcome,
            ConsentCommitOutcome::StaleCurrent { current: None }
        ),
        "an expectation against an empty store must be stale"
    );
    let empty = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(empty, Ok(None)), "stale save must store nothing");
}

#[tokio::test]
async fn credential_register_and_list() {
    let store = open_memory().await.unwrap();
    let listed_empty = store.list_refs().await;
    assert!(
        matches!(listed_empty, Ok(ref refs) if refs.is_empty()),
        "fresh store holds no refs"
    );
    let cred = CredentialRef::new("acme", "main").expect("valid test fixture");
    approve_pair(&store, "acme", "main", "sk-main", "reg-list-1").await;
    let second = CredentialRef::new("acme", "backup").expect("valid test fixture");
    approve_pair(&store, "acme", "backup", "sk-backup", "reg-list-2").await;
    let listed = store.list_refs().await;
    let refs = listed.unwrap();
    assert_eq!(refs.len(), 2, "both refs must list");
    assert!(refs.contains(&cred), "first ref must list");
    assert!(refs.contains(&second), "second ref must list");
}

#[tokio::test]
async fn local_id_is_correspondence_metadata_not_a_replay_key() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let first = store
        .append_message(history_command_with_ids(
            companion,
            generation,
            "first body",
            None,
            Some("send-1"),
        ))
        .await;
    assert!(
        matches!(first, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "first append must commit"
    );
    let second = store
        .append_message(history_command_with_ids(
            companion,
            generation,
            "second body",
            None,
            Some("send-1"),
        ))
        .await;
    assert!(
        matches!(second, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "repeating a local id must append, not replay"
    );
    let first_outcome = first.unwrap();
    let second_outcome = second.unwrap();
    assert_ne!(
        first_outcome, second_outcome,
        "local id repeats must mint distinct messages"
    );
    let loaded = store.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 2, "both local id repeats must persist");
    assert!(
        timeline
            .iter()
            .all(|item| item.local_id.as_deref() == Some("send-1")),
        "local id must round-trip as correspondence metadata"
    );
    assert!(timeline.iter().all(|item| item.command_id.is_none()));
}

#[tokio::test]
async fn command_replay_returns_original_accept_without_duplicate_row() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let command = CommandId(RawId::new());
    let base = history_command_with_ids(
        companion,
        generation,
        "original body",
        Some(command),
        Some("send-1"),
    );
    let first = store
        .append_reply_with_undelivered(base.clone(), true, None)
        .await;
    let (first_outcome, first_registered) = first.unwrap();
    let HistoryAppendOutcome::CommittedAs { message: first_id } = first_outcome else {
        panic!("the first append must commit, got {first_outcome:?}");
    };
    assert!(
        first_registered.is_some(),
        "first commit registers undelivered"
    );
    let retry = store
        .append_reply_with_undelivered(base.clone(), true, None)
        .await;
    let (retry_outcome, retry_registered) = retry.unwrap();
    let HistoryAppendOutcome::AlreadyCommittedAs { message, round } = retry_outcome else {
        assert!(
            format!("{retry_outcome:?}").is_empty(),
            "retry must replay the original accept, got {retry_outcome:?}"
        );
        return;
    };
    assert_eq!(
        message, first_id,
        "retry must replay the original message identity"
    );
    let looked_up = store.lookup_command(companion, &command).await;
    let original = looked_up.unwrap().unwrap();
    assert_eq!(original.round, round, "replay must name the original round");
    assert!(
        retry_registered.is_none(),
        "replay must not re-register undelivered"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(1), "replay must not append a second row");
    let pending = UndeliveredRepository::list_unpresented(
        &store,
        companion,
        None,
        ene_companion::UNDELIVERED_PAGE_MAX,
    )
    .await;
    let items = pending.unwrap().entries;
    assert_eq!(items.len(), 1, "replay must not duplicate undelivered");
    let looked_up = store.lookup_command(companion, &command).await;
    let item = looked_up.unwrap().unwrap();
    assert_eq!(item.id, first_id);
    assert_eq!(item.text, "original body");
}

#[tokio::test]
async fn command_reuse_with_different_request_is_declined() -> Result<(), String> {
    let store = open_memory().await.ok_or_else(|| String::from("open"))?;
    let (companion, generation) = running_companion(&store)
        .await
        .ok_or_else(|| String::from("companion"))?;
    let command = CommandId(RawId::new());
    let base = history_command_with_ids(
        companion,
        generation,
        "original body",
        Some(command),
        Some("send-1"),
    );
    let first = store.append_message(base.clone()).await;
    assert!(
        matches!(first, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "first command append must commit"
    );
    // Every request-semantics field decides: body, language, incarnation,
    // and the canonical client round intent.
    for (label, mut conflicting) in [
        ("body", {
            let mut cmd = base.clone();
            cmd.text = String::from("other body");
            cmd
        }),
        ("lang", {
            let mut cmd = base.clone();
            cmd.lang = String::from("fr");
            cmd
        }),
        ("incarnation", {
            let mut cmd = base.clone();
            cmd.incarnation = Some((9, 9));
            cmd
        }),
        ("round intent: auto to force-new", {
            let mut cmd = base.clone();
            cmd.round_intent = Some(RoundIntentMark::New);
            cmd
        }),
        ("round intent: auto to explicit join", {
            let mut cmd = base.clone();
            cmd.round_intent = Some(RoundIntentMark::Existing(String::from("round-wire-9")));
            cmd
        }),
    ] {
        let attempt = store.append_message(conflicting.clone()).await;
        assert!(
            matches!(attempt, Ok(HistoryAppendOutcome::CommandConflict)),
            "{label} mismatch must decline without side effects"
        );
        conflicting.text = String::from("original body");
        conflicting.lang = String::from("en");
        conflicting.incarnation = base.incarnation;
        conflicting.round_intent = base.round_intent.clone();
        let replayed = store.append_message(conflicting).await;
        assert!(
            matches!(
                replayed,
                Ok(HistoryAppendOutcome::AlreadyCommittedAs { .. })
            ),
            "{label} restored request must replay the original accept"
        );
    }
    // Round identity and its wire projection are the accepted result, not
    // the request: a newer round on the same send replays instead of
    // conflicting.
    let mut drifted = base.clone();
    drifted.round = RawId::new();
    drifted.round_wire = Some(String::from("rotated-wire"));
    let replayed = store.append_message(drifted).await;
    assert!(
        matches!(
            replayed,
            Ok(HistoryAppendOutcome::AlreadyCommittedAs { .. })
        ),
        "round/wire drift on the same request must replay, got {replayed:?}"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(1), "conflicts must never append rows");
    Ok(())
}

/// A keyed command without a stored round intent proves nothing: replay
/// fails closed (declined, never guessed) whether the stored row or the
/// incoming command is the one missing the intent.
#[tokio::test]
async fn unprovable_round_intent_fails_closed() -> Result<(), String> {
    let store = open_memory().await.ok_or_else(|| String::from("open"))?;
    let (companion, generation) = running_companion(&store)
        .await
        .ok_or_else(|| String::from("companion"))?;
    let command = CommandId(RawId::new());
    let keyed = history_command_with_ids(
        companion,
        generation,
        "original body",
        Some(command),
        Some("send-1"),
    );
    let first = store.append_message(keyed.clone()).await;
    assert!(
        matches!(first, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "keyed append must commit, got {first:?}"
    );
    let mut no_intent = keyed.clone();
    no_intent.round_intent = None;
    let attempt = store.append_message(no_intent).await;
    assert!(
        matches!(attempt, Ok(HistoryAppendOutcome::CommandConflict)),
        "a keyed command without a round intent must not exact-replay, got {attempt:?}"
    );
    let legacy = CommandId(RawId::new());
    let mut unmarked = history_command_with_ids(
        companion,
        generation,
        "legacy body",
        Some(legacy),
        Some("send-2"),
    );
    unmarked.round_intent = None;
    let stored = store.append_message(unmarked.clone()).await;
    assert!(
        matches!(stored, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "the unmarked row still stores, got {stored:?}"
    );
    let mut retry = keyed;
    retry.command_id = Some(legacy);
    retry.text = String::from("legacy body");
    retry.lang = String::from("en");
    retry.incarnation = unmarked.incarnation;
    retry.round_intent = Some(RoundIntentMark::Auto);
    let attempt = store.append_message(retry).await;
    assert!(
        matches!(attempt, Ok(HistoryAppendOutcome::CommandConflict)),
        "a stored row without an intent must not exact-replay, got {attempt:?}"
    );
    Ok(())
}

#[tokio::test]
async fn different_commands_append_separately() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let first = store
        .append_message(history_command_with_ids(
            companion,
            generation,
            "first body",
            Some(CommandId(RawId::new())),
            None,
        ))
        .await;
    assert!(
        matches!(first, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "first command must commit"
    );
    let second = store
        .append_message(history_command_with_ids(
            companion,
            generation,
            "second body",
            Some(CommandId(RawId::new())),
            None,
        ))
        .await;
    assert!(
        matches!(second, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "distinct command must commit separately"
    );
    let first_outcome = first.unwrap();
    let second_outcome = second.unwrap();
    assert_ne!(
        first_outcome, second_outcome,
        "distinct commands must mint distinct messages"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "distinct commands must persist twice");
}

#[tokio::test]
async fn null_command_appends_never_collide() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let first = store
        .append_message(history_command(companion, generation, "first body"))
        .await;
    assert!(
        matches!(first, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "first NULL command must commit"
    );
    let second = store
        .append_message(history_command(companion, generation, "second body"))
        .await;
    assert!(
        matches!(second, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "second NULL command must commit without colliding"
    );
    let first_outcome = first.unwrap();
    let second_outcome = second.unwrap();
    assert_ne!(
        first_outcome, second_outcome,
        "NULL commands must mint distinct messages"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "NULL commands must persist twice");
}

#[tokio::test]
async fn lookup_command_roundtrip_returns_both_ids() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let command = CommandId(RawId::new());
    let appended = store
        .append_message(history_command_with_ids(
            companion,
            generation,
            "command body",
            Some(command),
            Some("send-9"),
        ))
        .await;
    let HistoryAppendOutcome::CommittedAs { message } = appended.unwrap() else {
        panic!("unexpected variant");
    };
    let found = store.lookup_command(companion, &command).await;
    let item = found.unwrap().unwrap();
    assert_eq!(item.id, message);
    assert_eq!(item.command_id, Some(command));
    assert_eq!(item.local_id.as_deref(), Some("send-9"));
    assert_eq!(item.text, "command body");
    let missing = store
        .lookup_command(companion, &CommandId(RawId::new()))
        .await;
    assert!(
        matches!(missing, Ok(None)),
        "unknown command must find nothing"
    );
    let loaded = store.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].id, message);
    assert_eq!(timeline[0].command_id, Some(command));
    assert_eq!(timeline[0].local_id.as_deref(), Some("send-9"));
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
async fn device_request_approve_find_and_list() {
    let store = open_memory().await.unwrap();
    assert!(matches!(
        store.find_device_by_wire("no-such-wire").await,
        Ok(None)
    ));
    let first = store
        .request_pairing(String::from("phone"), String::from("conn-1"))
        .await
        .unwrap();
    let second = store
        .request_pairing(String::from("phone"), String::from("conn-2"))
        .await
        .unwrap();
    assert_ne!(first.pending_id, second.pending_id);
    assert_eq!(
        DevicePairingRepository::list_pending(&store)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(
        store.approve_pending(&first.pending_id, "conn-9").await,
        Ok(None)
    ));
    let (device, secret) = store
        .approve_pending(&first.pending_id, "conn-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(device.descriptor, "phone");
    assert_eq!(secret.expose_secret().len(), 36);
    assert!(matches!(
        store.approve_pending(&first.pending_id, "conn-1").await,
        Ok(None)
    ));
    assert_eq!(
        DevicePairingRepository::list_pending(&store)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        matches!(store.find_device_by_wire(&device.wire).await, Ok(Some(ref stored)) if *stored == device)
    );
    let (other, _) = store
        .approve_pending(&second.pending_id, "conn-2")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(other, device);
}

#[tokio::test]
async fn pairing_abandonment_is_origin_scoped_and_one_shot() {
    let store = open_memory().await.unwrap();
    let abandoned = store
        .request_pairing(String::from("phone"), String::from("conn-a"))
        .await
        .unwrap();
    let survivor = store
        .request_pairing(String::from("tablet"), String::from("conn-b"))
        .await
        .unwrap();
    store.abandon_pending_by_origin("conn-a").await.unwrap();
    assert!(matches!(
        store.approve_pending(&abandoned.pending_id, "conn-a").await,
        Ok(None)
    ));
    let listed = DevicePairingRepository::list_pending(&store).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].pending_id, survivor.pending_id);
    let (device, _) = store
        .approve_pending(&survivor.pending_id, "conn-b")
        .await
        .unwrap()
        .unwrap();
    store.abandon_pending_by_origin("conn-b").await.unwrap();
    assert!(matches!(
        store.approve_pending(&survivor.pending_id, "conn-b").await,
        Ok(None)
    ));
    assert!(
        matches!(store.find_device_by_wire(&device.wire).await, Ok(Some(ref stored)) if *stored == device)
    );
}

#[tokio::test]
async fn startup_clear_makes_pending_unapprovable() {
    let store = open_memory().await.unwrap();
    let pending = store
        .request_pairing(String::from("phone"), String::from("conn-1"))
        .await
        .unwrap();
    store.clear_unapproved_pendings().await.unwrap();
    assert!(
        DevicePairingRepository::list_pending(&store)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store.approve_pending(&pending.pending_id, "conn-1").await,
        Ok(None)
    ));
}

#[tokio::test]
async fn device_wire_is_opaque_and_resolvable() {
    let store = open_memory().await.unwrap();
    let pending = store
        .request_pairing(String::from("phone"), String::from("conn-1"))
        .await
        .unwrap();
    let (device, _) = store
        .approve_pending(&pending.pending_id, "conn-1")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(device.wire, crate::codec::encode_id(device.id.0));
    assert!(
        matches!(store.find_device_by_wire(&device.wire).await, Ok(Some(ref stored)) if *stored == device)
    );
    assert!(matches!(
        store.find_device_by_wire("no-such-wire").await,
        Ok(None)
    ));
}

#[tokio::test]
async fn history_wire_projection_and_incarnation_roundtrip() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let mut cmd = history_command(companion, generation, "opaque body");
    cmd.round_wire = Some(String::from("round-wire-9"));
    cmd.incarnation = Some((7, 11));
    let appended = store.append_message(cmd).await;
    let HistoryAppendOutcome::CommittedAs { message } = appended.unwrap() else {
        panic!("unexpected variant");
    };
    let loaded = store.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].id, message);
    assert_eq!(
        timeline[0].round_wire.as_deref(),
        Some("round-wire-9"),
        "timeline must echo the stored wire projection"
    );
    assert_eq!(
        timeline[0].incarnation,
        Some((7, 11)),
        "timeline must echo the stored incarnation"
    );
}

/// Reads the file's `user_version` without initializing the schema.
fn read_schema_version(path: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open(path).ok()?;
    conn.query_row("PRAGMA user_version", (), |row| row.get(0))
        .ok()
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
async fn intent_outcome_roundtrips_and_refreshes() {
    use ene_permission::{
        IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository as _,
        IntentResolution,
    };

    fn record() -> IntentOutcomeRecord {
        IntentOutcomeRecord {
            fingerprint: IntentFingerprint {
                intent_id: String::from("intent-1"),
                kind: String::from("assign"),
                target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
                base: String::from("consent-dialogue-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
            outcome: IntentOutcome::StoredAsRuleView {
                revision: String::from("consent-dialogue-rev-1"),
            },
        }
    }

    let store = open_memory().await.unwrap();
    let missing = store.lookup_intent_outcome("no-such-intent").await;
    assert!(matches!(missing, Ok(None)), "unknown intent must miss");
    let recorded = store.record_intent_outcome(record()).await;
    assert!(
        matches!(recorded, Ok(IntentResolution::Decided(()))),
        "first write must decide, got {recorded:?}"
    );
    let found = store.lookup_intent_outcome("intent-1").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == record()),
        "recorded outcome must read back"
    );
    // Write-once: re-recording the same intent replays the original, even
    // with a different outcome attached.
    let mut conflicting = record();
    conflicting.outcome = IntentOutcome::HeldByOperation;
    let rerecorded = store.record_intent_outcome(conflicting).await;
    assert!(
        matches!(
            rerecorded,
            Ok(IntentResolution::Replay(ref stored)) if *stored == record()
        ),
        "re-record must replay the original, got {rerecorded:?}"
    );
    let found = store.lookup_intent_outcome("intent-1").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == record()),
        "the original row must survive, got {found:?}"
    );
    let mut other = record();
    other.fingerprint.target = String::from("consent:dialogue:openai:other:openai:main");
    other.outcome = IntentOutcome::HeldByOperation;
    let conflicted = store.record_intent_outcome(other).await;
    assert!(
        matches!(
            conflicted,
            Ok(IntentResolution::Conflict(ref stored)) if *stored == record()
        ),
        "different content must conflict, got {conflicted:?}"
    );
    let found = store.lookup_intent_outcome("intent-1").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == record()),
        "conflict must not rewrite the row, got {found:?}"
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
                base: String::from("consent-dialogue-none"),
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
    let attempt = |model: &'static str| {
        let store = &store;
        let fingerprint = IntentFingerprint {
            intent_id: String::from("race-1"),
            kind: String::from("assign"),
            target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
            base: String::from("consent-dialogue-none"),
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

#[tokio::test]
async fn complete_with_intent_decides_atomically() {
    use ene_permission::{
        ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcome,
        IntentOutcomeRepository as _, IntentResolution,
    };

    fn fingerprint(id: &str, base: &str) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: id.to_owned(),
            kind: String::from("complete"),
            target: String::from("setup:complete"),
            base: base.to_owned(),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        }
    }

    let store = open_memory().await.unwrap();
    let empty = store
        .complete_with_intent(
            String::from("consent-dialogue-none"),
            true,
            fingerprint("c-0", "consent-dialogue-none"),
        )
        .await;
    assert!(
        matches!(
            empty,
            Ok(IntentResolution::Decided(ref decided)) if decided.outcome == IntentOutcome::NeedsClarification
        ),
        "empty completion must clarify, got {empty:?}"
    );
    let found = store.lookup_intent_outcome("c-0").await;
    assert!(
        matches!(found, Ok(Some(_))),
        "the clarify decision must leave its replay row"
    );
    let saved = save_consent(
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
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "consent must seed"
    );
    let unready = store
        .complete_with_intent(
            String::from("consent-dialogue-rev-1"),
            false,
            fingerprint("c-1", "consent-dialogue-rev-1"),
        )
        .await;
    assert!(
        matches!(
            unready,
            Ok(IntentResolution::Decided(ref decided)) if decided.outcome == IntentOutcome::NeedsClarification
        ),
        "bearerless completion must clarify, got {unready:?}"
    );
    let ready = store
        .complete_with_intent(
            String::from("consent-dialogue-rev-1"),
            true,
            fingerprint("c-2", "consent-dialogue-rev-1"),
        )
        .await;
    assert!(
        matches!(
            ready,
            Ok(IntentResolution::Decided(ref decided)) if decided.outcome == IntentOutcome::AppliedAsOneTime
        ),
        "ready completion must apply, got {ready:?}"
    );
    let stale = store
        .complete_with_intent(
            String::from("consent-dialogue-none"),
            true,
            fingerprint("c-3", "consent-dialogue-none"),
        )
        .await;
    assert!(
        matches!(
            stale,
            Ok(IntentResolution::Decided(ref decided)) if decided.outcome
                == IntentOutcome::StaleBaseView {
                    current: String::from("consent-dialogue-rev-1"),
                }
        ),
        "moved base must report stale with the current mark, got {stale:?}"
    );
    let found = store.lookup_intent_outcome("c-3").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if stored.outcome == IntentOutcome::StaleBaseView {
            current: String::from("consent-dialogue-rev-1"),
        }),
        "the stale decision must leave its replay row"
    );
}

#[tokio::test]
async fn shortcut_with_intent_hits_atomically() {
    use ene_permission::{
        ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcome,
        IntentOutcomeRepository as _, IntentResolution, ShortcutIntentOutcome,
    };

    fn fingerprint(id: &str) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: id.to_owned(),
            kind: String::from("assign"),
            target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
            base: String::from("consent-dialogue-rev-1"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        }
    }

    let store = open_memory().await.unwrap();
    let saved = save_consent(
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
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "consent must seed"
    );
    let hit = store
        .shortcut_with_intent(
            CapabilityKind::Dialogue,
            String::from("openai"),
            String::from("dialogue-1"),
            String::from("openai:main"),
            fingerprint("s-1"),
        )
        .await;
    assert!(
        matches!(
            hit,
            Ok(IntentResolution::Decided(ShortcutIntentOutcome::Hit { .. }))
        ),
        "matching route must hit, got {hit:?}"
    );
    let found = store.lookup_intent_outcome("s-1").await;
    assert!(
        matches!(
            found,
            Ok(Some(ref stored))
                if stored.outcome
                    == IntentOutcome::StoredAsRuleView {
                        revision: String::from("consent-dialogue-rev-1"),
                    }
        ),
        "the hit must leave its snapshot, got {found:?}"
    );
    let miss = store
        .shortcut_with_intent(
            CapabilityKind::Dialogue,
            String::from("openai"),
            String::from("dialogue-9"),
            String::from("openai:main"),
            fingerprint("s-2"),
        )
        .await;
    assert!(
        matches!(
            miss,
            Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss))
        ),
        "differing route must miss, got {miss:?}"
    );
    let missing = store.lookup_intent_outcome("s-2").await;
    assert!(
        matches!(missing, Ok(None)),
        "a miss must leave no replay row"
    );
}

#[tokio::test]
async fn begin_claims_started_rejects_moved_and_duplicate() {
    use ene_inference::{
        AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId,
    };
    use ene_permission::{ConsentRecord, ConsentRevision};

    let store = open_memory().await.unwrap();
    let saved = save_consent(
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
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "consent must seed"
    );
    let claim = |ticket: InferenceTicketId, rev: u64| InferenceAttempt {
        ticket,
        consumer: ConsumerKind::CompanionDialogue,
        capability: CapabilityKind::Dialogue,
        purpose: PurposeKind::DialogueResponse,
        expected_credential_set: CredentialSetRevision::initial(),
        expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(rev)),
        provider: String::from("openai"),
        model: String::from("dialogue-1"),
        data_use: Vec::new(),
        task_agent: None,
        pricing: None,
        usage_estimate: None,
    };
    let ticket = InferenceTicketId(RawId::new());
    let started = store.begin_inference_attempt(claim(ticket, 1)).await;
    assert!(
        matches!(started, Ok(AttemptBeginOutcome::Started)),
        "a fresh premise must claim, got {started:?}"
    );
    let duplicate = store.begin_inference_attempt(claim(ticket, 1)).await;
    assert!(
        matches!(duplicate, Ok(AttemptBeginOutcome::Stale)),
        "re-claiming one ticket must never send twice, got {duplicate:?}"
    );
    let moved = save_consent(
        &store,
        Some((String::from("consent-1"), ConsentRevision::from_u64(1))),
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(2),
            provider: String::from("openai"),
            model: String::from("dialogue-2"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(
        matches!(moved, ConsentCommitOutcome::Committed { .. }),
        "consent must move"
    );
    let stale = store
        .begin_inference_attempt(claim(InferenceTicketId(RawId::new()), 1))
        .await;
    assert!(
        matches!(stale, Ok(AttemptBeginOutcome::Stale)),
        "a moved premise must fail stale before any byte leaves, got {stale:?}"
    );
    let current = store
        .begin_inference_attempt(claim(InferenceTicketId(RawId::new()), 2))
        .await;
    assert!(
        matches!(current, Ok(AttemptBeginOutcome::Started)),
        "the current premise must claim, got {current:?}"
    );
}

#[tokio::test]
async fn credential_registration_blank_inputs_are_absent() {
    let store = open_memory().await.unwrap();
    for (provider, label) in [
        ("", "main"),
        ("   ", "main"),
        ("acme", ""),
        ("acme", "  "),
        ("", ""),
    ] {
        let requested = store
            .request_registration_with_intent(
                provider.to_owned(),
                label.to_owned(),
                registration_fingerprint("reg-blank", provider, label),
            )
            .await;
        assert!(
            requested.is_err(),
            "blank pair {provider:?}:{label:?} must be rejected"
        );
        let approved = store
            .approve_credential_with_sweep(provider, label, "sk-blank")
            .expect("blank input is absent, not an error");
        assert!(
            !approved,
            "blank pair {provider:?}:{label:?} must never approve"
        );
    }
    let listed = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed, Ok(ref items) if items.is_empty()),
        "blank requests must leave pending empty"
    );
    let refs = store.list_refs().await;
    assert!(
        matches!(refs, Ok(ref refs) if refs.is_empty()),
        "blank requests must leave the usable refs empty"
    );
}

#[tokio::test]
async fn restart_keeps_timeline_intact() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    let first = opened.unwrap();
    let ensured = first.ensure_running_companion().await;
    let companion = ensured.unwrap();
    let attributed = first.load_attribution(companion.as_raw()).await;
    let current = attributed.unwrap().unwrap();
    let first_append = first
        .append_message(history_command(companion, current.generation, "first"))
        .await;
    assert!(
        matches!(first_append, Ok(HistoryAppendOutcome::CommittedAs { .. })),
        "first append must commit"
    );
    let second_append = first
        .append_reply_with_undelivered(
            history_command(companion, current.generation, "second"),
            false,
            None,
        )
        .await;
    assert!(second_append.is_ok(), "second append must succeed");
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let ensured_again = second.ensure_running_companion().await;
    let same = ensured_again.unwrap();
    assert_eq!(same, companion, "companion must survive restart");
    let loaded = second.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 2, "both items must survive restart");
    assert_eq!(timeline[0].text, "first");
    assert_eq!(timeline[1].text, "second");
}

#[tokio::test]
async fn registration_intent_decides_held_then_already_decided() {
    let store = open_memory().await.unwrap();
    let first = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-1", "acme", "main"),
        )
        .await;
    assert_eq!(
        first,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "a fresh registration pends Owner approval"
    );
    let repeat = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-1", "acme", "main"),
        )
        .await;
    assert_eq!(
        repeat,
        Ok(RegistrationApply::AlreadyDecided),
        "the same intent never decides twice"
    );
    let approved = store
        .approve_credential_with_sweep("acme", "main", "sk-main")
        .expect("approval must commit");
    assert!(approved, "approval must apply");
    let usable = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            registration_fingerprint("reg-2", "acme", "main"),
        )
        .await;
    assert_eq!(
        usable,
        Ok(RegistrationApply::Decided(
            RegistrationState::AppliedAsOneTime
        )),
        "an approved pair registers as usable"
    );
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

/// Candidate lookup work is index-backed, not just its output: the query
/// plan for the exact recall SQL must serve every arm from an index with no
/// table scan and no sort step, so growing unrelated rows cannot push the
/// lookup back to a full scan. `SCAN (subquery-N)` lines only drain the
/// already-capped arm results and are expected.
#[tokio::test]
async fn recall_candidate_lookup_is_index_backed_not_a_scan() {
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
    for index in 0..400 {
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

    for term_count in [0, 2] {
        let sql = crate::learning::recall_candidates_sql(term_count);
        let plan = {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut statement = guard
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("the recall SQL must explain");
            let companion_text = crate::codec::encode_id(companion);
            let steps: Vec<String> = if term_count == 0 {
                statement
                    .query_map(rusqlite::params![companion_text, 3_i64], |row| {
                        row.get::<_, String>(3)
                    })
                    .expect("the plan must read")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("plan rows must decode")
            } else {
                statement
                    .query_map(
                        rusqlite::params![companion_text, 3_i64, "jasmine", "tea"],
                        |row| row.get::<_, String>(3),
                    )
                    .expect("the plan must read")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("plan rows must decode")
            };
            steps
        };
        assert!(
            !plan.is_empty(),
            "the recall plan must have steps for {term_count} terms"
        );
        for step in &plan {
            assert!(
                !(step.starts_with("SCAN") && !step.contains("subquery"))
                    && !step.contains("TEMP B-TREE"),
                "every arm must be an index walk, got: {plan:?}"
            );
        }
        if term_count > 0 {
            assert!(
                plan.iter()
                    .any(|step| step.contains("learning_memory_term")),
                "the lexical arm must seek the token index, got: {plan:?}"
            );
        }
    }

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
        "the oldest relevant memory stays reachable behind 400 newer rows"
    );
}

/// A revision update re-derives the token rows in the same commit: the old
/// content stops matching and the current content starts, with no orphaned
/// index row surviving beside the new recognition.
#[tokio::test]
async fn recall_token_index_follows_revision_updates() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "The owner drinks something warm.",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    let Ok(MemoryChangeOutcome::Committed { revision, .. }) = outcome else {
        panic!("the seed must commit");
    };
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: revision,
                },
                "The owner likes jasmine tea now.",
                ChangeKind::Refined,
                false,
            ),
        ))
        .await;
    assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));

    let terms = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut statement = guard
            .prepare("SELECT term FROM learning_memory_term WHERE memory_id = ?1")
            .expect("token rows must be readable");
        statement
            .query_map(
                rusqlite::params![crate::codec::encode_id(memory.as_raw())],
                |row| row.get::<_, String>(0),
            )
            .expect("token rows must read")
            .collect::<Result<Vec<_>, _>>()
            .expect("token rows must decode")
    };
    assert!(
        terms.iter().any(|term| term == "jasmine"),
        "the current content must be indexed, got {terms:?}"
    );
    assert!(
        terms.iter().all(|term| term != "warm"),
        "the replaced content must leave no token behind, got {terms:?}"
    );

    let candidates = store
        .recall_candidates(companion, &[String::from("jasmine")], 3)
        .await
        .unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.content.contains("jasmine tea now")),
        "the updated memory matches on its current content"
    );
}

/// The per-Memory token refresh is an index search, not a table scan: one
/// revision update must not cost a walk over every stored token.
#[tokio::test]
async fn memory_term_delete_uses_the_memory_index() {
    let store = open_memory().await.unwrap();
    let plan = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut statement = guard
            .prepare("EXPLAIN QUERY PLAN DELETE FROM learning_memory_term WHERE memory_id = ?1")
            .expect("the refresh delete must explain");
        statement
            .query_map(rusqlite::params!["probe"], |row| row.get::<_, String>(3))
            .expect("the plan must read")
            .collect::<Result<Vec<_>, _>>()
            .expect("plan rows must decode")
    };
    assert!(!plan.is_empty(), "the delete plan must have steps");
    for step in &plan {
        assert!(
            !step.starts_with("SCAN"),
            "the per-memory refresh must seek the index, got: {plan:?}"
        );
    }
    assert!(
        plan.iter()
            .any(|step| step.contains("idx_learning_memory_term_memory")),
        "the delete must use the memory index, got: {plan:?}"
    );
}

/// The credential sweep rebuilds derived tokens from the swept canonical
/// text: a whole-string replace cannot see the fragments the tokenizer
/// stored, so without a rebuild the old secret-derived pieces would stay
/// searchable beside redacted canonical content.
#[tokio::test]
async fn credential_sweep_rebuilds_tokens_from_swept_content() {
    use ene_learning::recall_index_terms;

    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let secret = MemoryId::generate();
    let mut change = learning_change(
        companion,
        MemoryTarget::New { id: secret },
        "the passcode is sk-live-9901 today",
        ChangeKind::Initial,
        false,
    );
    change.importance = Importance::clamped(0);
    let outcome = store.commit_memory_change(commit(None, change)).await;
    assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    // Bury the secret memory under higher-importance fillers so only the
    // lexical arm can surface it: newest returns fillers, importance
    // prefers them, and the secret term decides alone.
    for index in 0..10 {
        let id = MemoryId::generate();
        let mut filler = learning_change(
            companion,
            MemoryTarget::New { id },
            &format!("filler memory {index}"),
            ChangeKind::Initial,
            false,
        );
        filler.importance = Importance::clamped(9);
        let outcome = store.commit_memory_change(commit(None, filler)).await;
        assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    }
    let before = store
        .recall_candidates(companion, &[String::from("live")], 3)
        .await
        .unwrap();
    assert!(
        before.iter().any(|memory| memory.id == secret),
        "the secret-derived token is indexed before the sweep"
    );

    approve_pair(&store, "acme", "main", "sk-live-9901", "sweep-fragments-1").await;

    let (content, terms) = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let content: String = guard
            .query_row(
                "SELECT content FROM learning_memory WHERE memory_id = ?1",
                rusqlite::params![crate::codec::encode_id(secret.as_raw())],
                |row| row.get(0),
            )
            .expect("the swept memory must read");
        let mut statement = guard
            .prepare("SELECT term FROM learning_memory_term WHERE memory_id = ?1")
            .expect("token rows must be readable");
        let terms = statement
            .query_map(
                rusqlite::params![crate::codec::encode_id(secret.as_raw())],
                |row| row.get::<_, String>(0),
            )
            .expect("token rows must read")
            .collect::<Result<Vec<_>, _>>()
            .expect("token rows must decode");
        (content, terms)
    };
    assert!(
        !content.contains("sk-live-9901"),
        "canonical content is redacted"
    );
    let mut expected = recall_index_terms(&content);
    expected.sort();
    let mut stored = terms;
    stored.sort();
    assert_eq!(
        stored, expected,
        "derived tokens equal the swept canonical text exactly"
    );
    assert!(
        stored.iter().all(|term| term != "live" && term != "9901"),
        "no secret-derived fragment survives, got {stored:?}"
    );
    let after = store
        .recall_candidates(companion, &[String::from("live")], 3)
        .await
        .unwrap();
    assert!(
        after.iter().all(|memory| memory.id != secret),
        "the swept memory is lexically unreachable by the old fragment"
    );
}

/// Sweep, token rebuild, and credential revision advance share one
/// transaction: a rebuild failure rolls the canonical redaction back too,
///
/// never a redacted-canonical/old-token split.
#[tokio::test]
async fn credential_sweep_rebuild_is_atomic_with_the_redaction() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let secret = MemoryId::generate();
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: secret },
                "the passcode is sk-live-9901 today",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    let requested = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            RegistrationFingerprint {
                intent_id: String::from("sweep-atomic-1"),
                kind: String::from("register"),
                target: String::from("credential:acme:main"),
                base: String::from("consent-dialogue-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
        )
        .await;
    assert!(
        matches!(
            requested,
            Ok(ene_credential::RegistrationApply::Decided(
                ene_credential::RegistrationState::HeldByOperation
            ))
        ),
        "the pair must pend approval"
    );
    let revision_before: i64 = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .query_row("SELECT rev FROM credential_set WHERE id = 1", (), |row| {
                row.get(0)
            })
            .expect("the revision must read")
    };
    // Fault injection on the rebuild path only: the canonical sweep runs,
    // then the token insert aborts mid-transaction.
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch(
                "CREATE TRIGGER sweep_abort BEFORE INSERT ON learning_memory_term BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
            )
            .expect("the fault trigger must install");
    }
    let failed = store.approve_credential_with_sweep("acme", "main", "sk-live-9901");
    assert!(
        failed.is_err(),
        "the faulted sweep must fail, got {failed:?}"
    );
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let content: String = guard
            .query_row(
                "SELECT content FROM learning_memory WHERE memory_id = ?1",
                rusqlite::params![crate::codec::encode_id(secret.as_raw())],
                |row| row.get(0),
            )
            .expect("the memory must read");
        assert!(
            content.contains("sk-live-9901"),
            "the rolled-back redaction leaves canonical content untouched"
        );
        let revision: i64 = guard
            .query_row("SELECT rev FROM credential_set WHERE id = 1", (), |row| {
                row.get(0)
            })
            .expect("the revision must read");
        assert_eq!(
            revision, revision_before,
            "the revision must not advance on a rolled-back sweep"
        );
        guard
            .execute_batch("DROP TRIGGER sweep_abort;")
            .expect("the fault trigger must drop");
    }
    // The pending row from the committed request survives the rolled-back
    // approval, so the retry request answers from the same pending entry
    // before the approval is repeated.
    assert_eq!(
        CredentialApprovalRepository::list_pending(&store)
            .await
            .unwrap()
            .len(),
        1,
        "the faulted approval must not remove the committed pending row"
    );
    let retried = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            RegistrationFingerprint {
                intent_id: String::from("sweep-atomic-2"),
                kind: String::from("register"),
                target: String::from("credential:acme:main"),
                base: String::from("consent-dialogue-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
        )
        .await;
    assert!(
        matches!(
            retried,
            Ok(ene_credential::RegistrationApply::Decided(
                ene_credential::RegistrationState::HeldByOperation
            ))
        ),
        "the rolled-back registration must pend again"
    );
    assert!(
        store
            .approve_credential_with_sweep("acme", "main", "sk-live-9901")
            .expect("the retry must commit"),
        "the approval makes the pair usable"
    );
}

#[tokio::test]
async fn learning_reused_summary_identity_with_a_different_payload_is_refused() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "owner likes jasmine tea");
    let committed = store
        .commit_memory_change(commit(
            Some(evidence.clone()),
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner likes jasmine tea",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(
        committed,
        Ok(MemoryChangeOutcome::Committed { .. })
    ));

    // Same identity, different payload: the evidence must not be rebound and
    // the refused change must leave no current row behind.
    let second = MemoryId::generate();
    let mut conflicting = evidence.clone();
    conflicting.content = String::from("evidence that never happened");
    let refused = store
        .commit_memory_change(commit(
            Some(conflicting),
            learning_change(
                companion,
                MemoryTarget::New { id: second },
                "a recognition grounded on the conflicting payload",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert_eq!(
        refused,
        Err(LearningTechnicalError::SummaryIdentityConflict {
            summary: evidence.id,
        })
    );
    assert!(
        store
            .list_memory_revisions(second, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "the refused change must leave no current row"
    );
    let stored = store
        .load_summaries(&[evidence.id])
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(stored.content, "owner likes jasmine tea");
}

#[tokio::test]
async fn learning_stale_change_leaves_no_orphan_summary() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let seeded = store
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
    assert!(matches!(seeded, Ok(MemoryChangeOutcome::Committed { .. })));
    let advance = store
        .commit_memory_change(commit(
            None,
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
    assert!(matches!(advance, Ok(MemoryChangeOutcome::Committed { .. })));

    // A formation judged from the now-old revision: its change and its new
    // Summary evidence both roll back, so no orphaned grounds remain.
    let evidence = learning_summary(companion, "stale evidence");
    let stale = store
        .commit_memory_change(commit(
            Some(evidence.clone()),
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "stale recognition",
                ChangeKind::Reinforced,
                false,
            ),
        ))
        .await;
    assert_eq!(
        stale,
        Ok(MemoryChangeOutcome::StaleTarget {
            memory,
            current: MemoryRevision::from_u64(2),
        })
    );
    assert_eq!(
        store.load_summaries(&[evidence.id]).await,
        Ok(Vec::new()),
        "a rejected change must not strand its Summary"
    );
    assert_eq!(
        store
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap()
            .len(),
        2
    );
}

/// The durable revision column is signed: a stored revision at the
/// representable bound is exhausted for storage, so the update reports the
/// documented domain outcome and writes no revision row.
#[tokio::test]
async fn learning_revision_at_the_signed_bound_is_exhausted_without_a_write() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let seeded = store
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
    assert!(matches!(seeded, Ok(MemoryChangeOutcome::Committed { .. })));
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE learning_memory SET revision = ?1 WHERE memory_id = ?2",
                rusqlite::params![i64::MAX, crate::codec::encode_id(memory.as_raw())],
            )
            .expect("the current row must move to the bound");
        guard
            .execute(
                "UPDATE learning_memory_revision SET revision = ?1 WHERE memory_id = ?2",
                rusqlite::params![i64::MAX, crate::codec::encode_id(memory.as_raw())],
            )
            .expect("the revision row must move to the bound");
    }
    let outcome = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::from_u64(i64::MAX as u64),
                },
                "owner lives in Osaka",
                ChangeKind::ChangedSince,
                false,
            ),
        ))
        .await;
    assert_eq!(
        outcome,
        Ok(MemoryChangeOutcome::RevisionExhausted { memory })
    );
    let revisions: i64 = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .query_row(
                "SELECT COUNT(*) FROM learning_memory_revision WHERE memory_id = ?1",
                rusqlite::params![crate::codec::encode_id(memory.as_raw())],
                |row| row.get(0),
            )
            .expect("the revision count must read")
    };
    assert_eq!(
        revisions, 1,
        "the exhausted update must write no revision row"
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
async fn learning_stale_update_is_rejected_without_overwriting() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let seeded = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner is on the night shift",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(seeded, Ok(MemoryChangeOutcome::Committed { .. })));
    let winner = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "owner switched to the day shift",
                ChangeKind::ChangedSince,
                false,
            ),
        ))
        .await;
    assert!(matches!(winner, Ok(MemoryChangeOutcome::Committed { .. })));

    let stale = store
        .commit_memory_change(commit(
            None,
            learning_change(
                companion,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "stale formation result",
                ChangeKind::Reinforced,
                false,
            ),
        ))
        .await;
    assert_eq!(
        stale,
        Ok(MemoryChangeOutcome::StaleTarget {
            memory,
            current: MemoryRevision::from_u64(2),
        })
    );
    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(
        revisions[1].content, "owner switched to the day shift",
        "the stale result must not overwrite the newer recognition"
    );
    assert_eq!(revisions.len(), 2);
}

#[tokio::test]
async fn learning_scope_and_missing_target_are_domain_outcomes() {
    let store = open_memory().await.unwrap();
    let owner = RawId::new();
    let other = RawId::new();
    let memory = MemoryId::generate();
    let seeded = store
        .commit_memory_change(commit(
            None,
            learning_change(
                owner,
                MemoryTarget::New { id: memory },
                "private to the owner companion",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(seeded, Ok(MemoryChangeOutcome::Committed { .. })));

    let crossed = store
        .commit_memory_change(commit(
            None,
            learning_change(
                other,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                "must not cross companions",
                ChangeKind::Reinforced,
                false,
            ),
        ))
        .await;
    assert_eq!(crossed, Ok(MemoryChangeOutcome::ScopeMismatch { memory }));

    let missing = store
        .commit_memory_change(commit(
            None,
            learning_change(
                owner,
                MemoryTarget::Existing {
                    id: MemoryId::generate(),
                    expected_revision: MemoryRevision::initial(),
                },
                "target does not exist",
                ChangeKind::Reinforced,
                false,
            ),
        ))
        .await;
    assert!(matches!(
        missing,
        Ok(MemoryChangeOutcome::MissingTarget { .. })
    ));
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
async fn learning_list_current_is_companion_scoped_and_newest_first() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let other = RawId::new();
    let first = MemoryId::generate();
    let second = MemoryId::generate();
    for (companion, id, text) in [
        (companion, first, "first memory"),
        (companion, second, "second memory"),
        (other, MemoryId::generate(), "other companion memory"),
    ] {
        let outcome = store
            .commit_memory_change(commit(
                None,
                learning_change(
                    companion,
                    MemoryTarget::New { id },
                    text,
                    ChangeKind::Initial,
                    false,
                ),
            ))
            .await;
        assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
    }
    let listed = store
        .list_current_memories(companion, None, 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2, "only this companion's memories");
    assert_eq!(listed[0].id, second, "newest insert first");
    assert_eq!(listed[1].id, first);
    assert!(
        listed
            .iter()
            .all(|memory| memory.scope == LearningScope::companion(companion))
    );
    // The cursor starts strictly after the named Memory and stays scoped.
    let older = store
        .list_current_memories(companion, Some(second), 10)
        .await
        .unwrap();
    assert_eq!(older.len(), 1, "the cursor pages past the named Memory");
    assert_eq!(older[0].id, first);
    let unknown = store
        .list_current_memories(companion, Some(MemoryId::generate()), 10)
        .await
        .unwrap();
    assert!(unknown.is_empty(), "an unknown cursor yields no page");
}

#[tokio::test]
async fn learning_memory_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("learning.db");
    let store = Store::open(&path).await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "a durable preference");
    let committed = store
        .commit_memory_change(commit(
            Some(evidence.clone()),
            learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner prefers morning conversations",
                ChangeKind::Initial,
                false,
            ),
        ))
        .await;
    assert!(matches!(
        committed,
        Ok(MemoryChangeOutcome::Committed { .. })
    ));
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    let memories = reopened
        .list_current_memories(companion, None, 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1, "memory must survive reopen");
    assert_eq!(memories[0].content, "owner prefers morning conversations");
    let revisions = reopened
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].summary, Some(evidence.id));
    let stored_evidence = reopened
        .load_summaries(&[evidence.id])
        .await
        .unwrap()
        .into_iter()
        .next();
    assert!(
        stored_evidence.is_some(),
        "summary evidence survives reopen"
    );
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
