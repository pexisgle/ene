use crate::Store;
use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, PresentationMark, ReportStatus,
    ReportStatusTransition, RoundIntentMark, UndeliveredRef, UndeliveredRepository,
};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository, CredentialSetRepository,
    CredentialSetRevision, DeviceId, DevicePairingRepository, DevicePairingStatus,
    MemoryCredentialStore,
};
use ene_inference::{InferenceTicketId, UsageFact, UsageRepository, UsageSource};
use ene_learning::{
    ChangeKind, ExperienceSourceKind, Importance, LearningRepository, LearningScope,
    LearningTechnicalError, MemoryChange, MemoryChangeCommit, MemoryChangeOutcome, MemoryId,
    MemoryRevision, MemoryTarget, SourceRangeRef, SummaryId, SummaryRecord, TemporalMeaning,
};
use ene_permission::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
};
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceCheckRef,
    PresenceGeneration, PresenceRepository, PresenceState, ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::params;

fn fixture_clock() -> WallClockWithTz {
    if let Ok(at) = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00") {
        at
    } else {
        WallClockWithTz::now()
    }
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
        command_id: None,
        round_wire: Some(RawId::new().as_uuid().to_string()),
        round_intent: None,
        incarnation: Some((1, 2)),
        local_id: None,
    }
}

/// A keyed command always names its canonical round intent, so its replay
/// stays decidable from durable state.
fn history_command_with_ids(
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
    command_id: Option<CommandId>,
    local_id: Option<&str>,
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
        command_id,
        round_wire: Some(RawId::new().as_uuid().to_string()),
        round_intent: command_id.map(|_| RoundIntentMark::Auto),
        incarnation: Some((1, 2)),
        local_id: local_id.map(String::from),
    }
}

fn history_row_count(store: &Store, companion: CompanionId) -> Option<i64> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let counted: Result<i64, _> = guard.query_row(
        "SELECT COUNT(*) FROM history_message WHERE companion_id = ?1",
        params![crate::codec::encode_id(companion.as_raw())],
        |row| row.get(0),
    );
    assert!(counted.is_ok(), "row count must succeed");
    counted.ok()
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
async fn append_message_commits_and_timeline_reads_back() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let appended = store
        .append_message(history_command(companion, generation, "hello history"))
        .await;
    let outcome = appended.unwrap();
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "happy path must commit"
    );
    let loaded = store.load_timeline(companion, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].text, "hello history");
    assert_eq!(timeline[0].lang, "en");
    assert_eq!(timeline[0].presence_generation, generation);
    let bounded = store
        .load_timeline(companion, Some(fixture_clock()), 10)
        .await;
    let kept = bounded.unwrap();
    assert_eq!(kept.len(), 1, "item at the bound must be kept");
    let capped = store.load_timeline(companion, None, 0).await;
    let none = capped.unwrap();
    assert!(none.is_empty(), "zero limit must return nothing");
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
    let loaded = store.load_timeline(companion, None, 10).await;
    let timeline = loaded.unwrap();
    assert!(timeline.is_empty(), "stale append must store nothing");
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
    let loaded = store.load_timeline(companion, None, 10).await;
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
        .append_reply_with_undelivered(history_command(companion, generation, "reply body"), true)
        .await;
    let (outcome, registered) = appended.unwrap();
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "reply must commit"
    );
    let entry = registered.unwrap();
    assert_eq!(entry.status, ReportStatus::Pending);
    let pending = UndeliveredRepository::list_pending(&store, companion).await;
    let items = pending.unwrap();
    assert_eq!(items.len(), 1, "one entry must be pending");
    let marked = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::Pending,
            PresentationMark {
                round: entry.round,
                presented: true,
            },
        )
        .await;
    assert_eq!(
        marked,
        Ok(ReportStatusTransition::PendingToPresented),
        "pending to presented must compare-and-mark"
    );
    let pending_after = UndeliveredRepository::list_pending(&store, companion).await;
    let drained = pending_after.unwrap();
    assert!(drained.is_empty(), "presented entry must leave pending");
    let stale = store
        .compare_and_mark_reported(
            entry.id,
            ReportStatus::Pending,
            PresentationMark {
                round: entry.round,
                presented: true,
            },
        )
        .await;
    assert_eq!(
        stale,
        Ok(ReportStatusTransition::StaleSource),
        "repeat mark on a moved row must be stale"
    );
    let missing = UndeliveredRef {
        id: RawId::new(),
        companion,
        source_message: RawId::new(),
        status: ReportStatus::Pending,
        round: RawId::new(),
        presence_generation: generation,
    };
    let absent = store.register_if_parent_durable(missing).await;
    assert!(
        matches!(absent, Ok(false)),
        "register without a durable parent must decline"
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
    assert!(
        matches!(decision, MoveDecision::RejectedAsStalePresence { .. }),
        "generation mismatch must reject as stale"
    );
    if let MoveDecision::RejectedAsStalePresence { current } = decision {
        assert_eq!(current.generation, generation);
        assert_eq!(current.state, PresenceState::NoActive);
    } else {
        return;
    }
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

#[tokio::test]
async fn consent_compare_and_save_commit_and_stale_matrix() {
    let store = open_memory().await.unwrap();
    let empty = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(empty, Ok(None)), "fresh store holds no consent");
    let first = consent_record("consent-1", 3);
    let committed = store.compare_and_save(None, first.clone()).await;
    assert!(
        matches!(
            committed,
            Ok(ConsentCommitOutcome::Committed { ref record }) if *record == first
        ),
        "empty store with no expectation must commit"
    );
    let loaded = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(loaded, Ok(Some(ref current)) if *current == first));
    let intruder = consent_record("consent-9", 1);
    let unexpected = store.compare_and_save(None, intruder).await;
    assert!(
        matches!(
            unexpected,
            Ok(ConsentCommitOutcome::StaleCurrent { ref current }) if *current == Some(first.clone())
        ),
        "existing row with no expectation must be stale"
    );
    let next = consent_record("consent-1", 4);
    let recommitted = store
        .compare_and_save(
            Some((String::from("consent-1"), ConsentRevision::from_u64(3))),
            next.clone(),
        )
        .await;
    assert!(
        matches!(
            recommitted,
            Ok(ConsentCommitOutcome::Committed { ref record }) if *record == next
        ),
        "matching expectation must commit the replacement"
    );
    let replay = consent_record("consent-1", 5);
    let stale_rev = store
        .compare_and_save(
            Some((String::from("consent-1"), ConsentRevision::from_u64(3))),
            replay,
        )
        .await;
    assert!(
        matches!(
            stale_rev,
            Ok(ConsentCommitOutcome::StaleCurrent { ref current }) if *current == Some(next.clone())
        ),
        "revision mismatch must be stale"
    );
    let fork = consent_record("consent-2", 4);
    let stale_id = store
        .compare_and_save(
            Some((String::from("consent-2"), ConsentRevision::from_u64(4))),
            fork,
        )
        .await;
    assert!(
        matches!(
            stale_id,
            Ok(ConsentCommitOutcome::StaleCurrent { ref current }) if *current == Some(next.clone())
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
async fn consent_compare_and_save_expected_but_empty_is_stale() {
    let store = open_memory().await.unwrap();
    let record = consent_record("consent-1", 1);
    let outcome = store
        .compare_and_save(
            Some((String::from("consent-1"), ConsentRevision::from_u64(1))),
            record,
        )
        .await;
    assert!(
        matches!(
            outcome,
            Ok(ConsentCommitOutcome::StaleCurrent { current: None })
        ),
        "an expectation against an empty store must be stale"
    );
    let empty = store.load_current(CapabilityKind::Dialogue).await;
    assert!(matches!(empty, Ok(None)), "stale save must store nothing");
}

#[tokio::test]
async fn credential_save_load_and_list() {
    let store = open_memory().await.unwrap();
    let missing = store.load_ref("acme", "main").await;
    assert!(matches!(missing, Ok(None)), "fresh store holds no refs");
    let cred = CredentialRef::new("acme", "main").expect("valid test fixture");
    let saved = store.save_ref(cred.clone()).await;
    assert!(saved.is_ok(), "ref save must succeed");
    let loaded = store.load_ref("acme", "main").await;
    let found = loaded.unwrap().unwrap();
    assert_eq!(found, cred, "ref must round-trip");
    let second = CredentialRef::new("acme", "backup").expect("valid test fixture");
    let saved_second = store.save_ref(second.clone()).await;
    assert!(saved_second.is_ok(), "second ref save must succeed");
    let listed = store.list_refs().await;
    let refs = listed.unwrap();
    assert_eq!(refs.len(), 2, "both refs must list");
    assert!(refs.contains(&cred), "first ref must list");
    assert!(refs.contains(&second), "second ref must list");
}

#[tokio::test]
async fn usage_insert_preserves_null_tokens() {
    let store = open_memory().await.unwrap();
    let ticket = RawId::new();
    let fact = UsageFact {
        ticket: InferenceTicketId(ticket),
        provider: String::from("acme"),
        model: String::from("dialogue-1"),
        input_tokens: None,
        output_tokens: None,
        source: UsageSource::Unknown,
    };
    let recorded = store.record_usage(fact).await;
    assert!(recorded.is_ok(), "usage record must succeed");
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let checked = guard.query_row(
            "SELECT input_tokens, output_tokens, source FROM usage_fact WHERE ticket = ?1",
            params![crate::codec::encode_id(ticket)],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        let (input, output, source) = checked.unwrap();
        assert_eq!(input, None, "unknown input stays NULL, never zero");
        assert_eq!(output, None, "unknown output stays NULL, never zero");
        assert_eq!(source.as_str(), "unknown");
    }
    let counted = UsageFact {
        ticket: InferenceTicketId(RawId::new()),
        provider: String::from("acme"),
        model: String::from("dialogue-1"),
        input_tokens: Some(4),
        output_tokens: Some(2),
        source: UsageSource::Reported,
    };
    let recorded_counted = store.record_usage(counted).await;
    assert!(recorded_counted.is_ok(), "counted usage must succeed");
    let duplicate = UsageFact {
        ticket: InferenceTicketId(ticket),
        provider: String::from("acme"),
        model: String::from("dialogue-1"),
        input_tokens: Some(1),
        output_tokens: Some(1),
        source: UsageSource::Reported,
    };
    let repeated = store.record_usage(duplicate).await;
    assert!(
        repeated.is_err(),
        "duplicate ticket must fail without panicking"
    );
}

#[tokio::test]
async fn local_id_lookup_is_correspondence_metadata_not_replay_key_replacing_local_id_uniqueness() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let absent = store.lookup_local_id(companion, "send-1").await;
    assert!(
        matches!(absent, Ok(None)),
        "unknown local id must find nothing"
    );
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
    let found = store.lookup_local_id(companion, "send-1").await;
    let item = found.unwrap().unwrap();
    assert_eq!(item.local_id.as_deref(), Some("send-1"));
    assert_eq!(item.command_id, None);
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "both local id repeats must persist");
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
        .append_reply_with_undelivered(base.clone(), true)
        .await;
    let (first_outcome, first_registered) = first.unwrap();
    let HistoryAppendOutcome::CommittedAs { message: first_id } = first_outcome else {
        return;
    };
    assert!(
        first_registered.is_some(),
        "first commit registers undelivered"
    );
    let retry = store
        .append_reply_with_undelivered(base.clone(), true)
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
    let pending = UndeliveredRepository::list_pending(&store, companion).await;
    let items = pending.unwrap();
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
    let by_local = store.lookup_local_id(companion, "send-9").await;
    let same = by_local.unwrap().unwrap();
    assert_eq!(same.id, message);
    assert_eq!(same.command_id, Some(command));
    let loaded = store.load_timeline(companion, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].id, message);
    assert_eq!(timeline[0].command_id, Some(command));
    assert_eq!(timeline[0].local_id.as_deref(), Some("send-9"));
}

#[tokio::test]
async fn migration_v3_keeps_pre_command_rows_readable() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let companion = CompanionId::from_raw(RawId::new());
    let companion_text = crate::codec::encode_id(companion.as_raw());
    let message_id = RawId::new();
    let message_text = crate::codec::encode_id(message_id);
    let round_id = RawId::new();
    let round_text = crate::codec::encode_id(round_id);
    {
        let conn = rusqlite::Connection::open(&path);
        let conn = conn.unwrap();
        let shaped = conn.execute_batch(
                "CREATE TABLE companion (companion_id TEXT PRIMARY KEY, lifecycle TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE history_message (message_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, round_id TEXT NOT NULL, role TEXT NOT NULL, body TEXT NOT NULL, lang TEXT NOT NULL, at TEXT NOT NULL, presence_generation INTEGER NOT NULL, local_id TEXT NULL);
CREATE TABLE undelivered (undelivered_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, source_message TEXT NOT NULL, status TEXT NOT NULL, round_id TEXT NOT NULL, presence_generation INTEGER NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE presence_attribution (companion_id TEXT PRIMARY KEY, state TEXT NOT NULL, active_client TEXT, generation INTEGER NOT NULL);
CREATE TABLE presence_transition_log (transition_seq INTEGER PRIMARY KEY AUTOINCREMENT, companion_id TEXT NOT NULL, old_state TEXT NOT NULL, new_state TEXT NOT NULL, old_gen INTEGER NOT NULL, new_gen INTEGER NOT NULL, reason TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE consent_record (id TEXT PRIMARY KEY, rev INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, credential_id TEXT NOT NULL);
CREATE TABLE credential_ref (id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL, UNIQUE (provider, label));
CREATE TABLE usage_fact (ticket TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER, source TEXT NOT NULL);
CREATE TABLE pairing_pending (descriptor TEXT PRIMARY KEY, requested_at TEXT NOT NULL);
CREATE TABLE paired_device (device_id TEXT PRIMARY KEY, descriptor TEXT NOT NULL, paired_at TEXT NOT NULL);
CREATE INDEX idx_history_message_companion ON history_message (companion_id);
CREATE INDEX idx_history_message_round ON history_message (round_id);
CREATE INDEX idx_undelivered_companion_status ON undelivered (companion_id, status);
CREATE UNIQUE INDEX idx_history_message_companion_local ON history_message (companion_id, local_id);
CREATE INDEX idx_paired_device_descriptor ON paired_device (descriptor);
CREATE TABLE _schema_version (version INTEGER NOT NULL);
INSERT INTO _schema_version (version) VALUES (2);",
            );
        assert!(shaped.is_ok(), "v2 shape must apply");
        let seeded_companion = conn.execute(
            "INSERT INTO companion (companion_id, lifecycle, created_at) VALUES (?1, ?2, ?3)",
            params![
                companion_text,
                crate::codec::encode_lifecycle(CompanionLifecycle::Running),
                fixture_clock().to_rfc3339()
            ],
        );
        assert!(seeded_companion.is_ok(), "v2 companion must seed");
        let seeded_attribution = conn.execute(
                "INSERT INTO presence_attribution (companion_id, state, active_client, generation) VALUES (?1, ?2, ?3, ?4)",
                params![companion_text, "no_active", Option::<String>::None, 0_i64],
            );
        assert!(seeded_attribution.is_ok(), "v2 attribution must seed");
        let seeded_history = conn.execute(
                "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, presence_generation, local_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    message_text,
                    companion_text,
                    round_text,
                    "owner",
                    "legacy body",
                    "en",
                    fixture_clock().to_rfc3339(),
                    0_i64,
                    "legacy-1"
                ],
            );
        assert!(seeded_history.is_ok(), "v2 history row must seed");
    }
    let opened = Store::open(&path).await;
    let store = opened.unwrap();
    let loaded = store.load_timeline(companion, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "legacy row must survive migration");
    assert_eq!(timeline[0].id, message_id);
    assert_eq!(timeline[0].text, "legacy body");
    assert_eq!(timeline[0].local_id.as_deref(), Some("legacy-1"));
    assert_eq!(timeline[0].command_id, None);
    assert_eq!(
        timeline[0].round_wire, None,
        "pre-opaque rows carry no wire projection"
    );
    assert_eq!(
        timeline[0].incarnation, None,
        "pre-opaque rows carry no incarnation"
    );
    let by_local = store.lookup_local_id(companion, "legacy-1").await;
    let legacy = by_local.unwrap().unwrap();
    assert_eq!(legacy.id, message_id);
    let missing = store
        .lookup_command(companion, &CommandId(RawId::new()))
        .await;
    assert!(
        matches!(missing, Ok(None)),
        "legacy row carries no command key"
    );
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let version = guard.query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
        row.get::<_, i64>(0)
    });
    assert!(
        matches!(version, Ok(11)),
        "migration must record version 11"
    );
    let new_index: Result<String, _> = guard.query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_history_message_companion_command'",
            (),
            |row| row.get(0),
        );
    assert!(
        new_index.is_ok(),
        "command replay index must exist after migration"
    );
    let old_index: Result<String, _> = guard.query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_history_message_companion_local'",
            (),
            |row| row.get(0),
        );
    assert!(
        old_index.is_err(),
        "retired local id index must be gone after migration"
    );
}

#[tokio::test]
async fn device_request_approve_find_and_list() {
    let store = open_memory().await.unwrap();
    let missing = store.find_device(&DeviceId(RawId::new())).await;
    assert!(matches!(missing, Ok(None)), "fresh store pairs nothing");
    let listed_empty = DevicePairingRepository::list_pending(&store).await;
    assert!(
        matches!(listed_empty, Ok(ref items) if items.is_empty()),
        "fresh store pends nothing"
    );
    let first = store.request_pairing(String::from("phone")).await;
    assert!(
        matches!(first, Ok(DevicePairingStatus::Pending { .. })),
        "first request must pend"
    );
    let DevicePairingStatus::Pending {
        pending: first_pending,
    } = first.unwrap()
    else {
        panic!("unexpected variant");
    };
    assert_eq!(first_pending.descriptor.as_str(), "phone");
    let second = store.request_pairing(String::from("phone")).await;
    assert!(
        matches!(second, Ok(DevicePairingStatus::Pending { .. })),
        "repeat request must stay pending"
    );
    let DevicePairingStatus::Pending {
        pending: second_pending,
    } = second.unwrap()
    else {
        panic!("unexpected variant");
    };
    assert_eq!(
        second_pending.requested_at, first_pending.requested_at,
        "repeat request must not refresh the stored time"
    );
    let tablet = store.request_pairing(String::from("tablet")).await;
    assert!(
        matches!(tablet, Ok(DevicePairingStatus::Pending { .. })),
        "second descriptor must pend"
    );
    let listed = DevicePairingRepository::list_pending(&store).await;
    let items = listed.unwrap();
    assert_eq!(items.len(), 2, "both descriptors must list as pending");
    let unknown = DevicePairingRepository::approve_pending(&store, "unknown").await;
    assert!(
        matches!(unknown, Ok(None)),
        "approving an unknown descriptor must yield none"
    );
    let approved = DevicePairingRepository::approve_pending(&store, "phone").await;
    let (device, secret) = approved.unwrap().unwrap();
    assert_eq!(device.descriptor.as_str(), "phone");
    assert!(
        secret.len() == 36 && secret.chars().filter(|c| *c == '-').count() == 4,
        "approval must mint a UUID-text one-time secret"
    );
    let pending_after = DevicePairingRepository::list_pending(&store).await;
    let remaining = pending_after.unwrap();
    assert_eq!(remaining.len(), 1, "approval must drain one entry");
    assert_eq!(remaining[0].descriptor.as_str(), "tablet");
    let found = store.find_device(&device.id).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == device),
        "approved device must be findable by id"
    );
    let again = store.request_pairing(String::from("phone")).await;
    assert!(
        matches!(
            again,
            Ok(DevicePairingStatus::Paired { device: ref existing }) if *existing == device
        ),
        "re-request after pairing must return the stored record"
    );
    let reapproved = DevicePairingRepository::approve_pending(&store, "phone").await;
    let (same, rotated) = reapproved.unwrap().unwrap();
    assert!(
        same == device,
        "re-approval must return the existing record"
    );
    assert!(
        rotated.len() == 36 && rotated != secret,
        "re-approval must rotate to a fresh secret"
    );
}

#[tokio::test]
async fn device_wire_is_opaque_and_resolvable() {
    let store = open_memory().await.unwrap();
    let requested = store.request_pairing(String::from("phone")).await;
    assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
    let approved = DevicePairingRepository::approve_pending(&store, "phone").await;
    let (device, _) = approved.unwrap().unwrap();
    assert_ne!(
        device.wire,
        crate::codec::encode_id(device.id.0),
        "wire projection must not render the domain identity"
    );
    let by_wire = store.find_device_by_wire(&device.wire).await;
    assert!(
        matches!(by_wire, Ok(Some(ref stored)) if *stored == device),
        "wire lookup must resolve the approved record"
    );
    let unknown = store.find_device_by_wire("no-such-wire").await;
    assert!(matches!(unknown, Ok(None)), "unknown wire must miss");
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
    let loaded = store.load_timeline(companion, None, 10).await;
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
    let by_local_none = store.lookup_local_id(companion, "missing").await;
    assert!(
        matches!(by_local_none, Ok(None)),
        "unrelated correspondence lookup must miss"
    );
}

#[tokio::test]
async fn migration_v4_backfills_legacy_device_wire() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let device_id = RawId::new();
    let device_text = crate::codec::encode_id(device_id);
    {
        let conn = rusqlite::Connection::open(&path);
        let conn = conn.unwrap();
        let shaped = conn.execute_batch(
                "CREATE TABLE companion (companion_id TEXT PRIMARY KEY, lifecycle TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE history_message (message_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, round_id TEXT NOT NULL, role TEXT NOT NULL, body TEXT NOT NULL, lang TEXT NOT NULL, at TEXT NOT NULL, presence_generation INTEGER NOT NULL, local_id TEXT NULL, command_id TEXT NULL);
CREATE TABLE undelivered (undelivered_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, source_message TEXT NOT NULL, status TEXT NOT NULL, round_id TEXT NOT NULL, presence_generation INTEGER NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE presence_attribution (companion_id TEXT PRIMARY KEY, state TEXT NOT NULL, active_client TEXT, generation INTEGER NOT NULL);
CREATE TABLE presence_transition_log (transition_seq INTEGER PRIMARY KEY AUTOINCREMENT, companion_id TEXT NOT NULL, old_state TEXT NOT NULL, new_state TEXT NOT NULL, old_gen INTEGER NOT NULL, new_gen INTEGER NOT NULL, reason TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE consent_record (id TEXT PRIMARY KEY, rev INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, credential_id TEXT NOT NULL);
CREATE TABLE credential_ref (id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL, UNIQUE (provider, label));
CREATE TABLE usage_fact (ticket TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER, source TEXT NOT NULL);
CREATE TABLE pairing_pending (descriptor TEXT PRIMARY KEY, requested_at TEXT NOT NULL);
CREATE TABLE paired_device (device_id TEXT PRIMARY KEY, descriptor TEXT NOT NULL, paired_at TEXT NOT NULL);
CREATE TABLE credential_pending (provider TEXT NOT NULL, label TEXT NOT NULL, requested_at TEXT NOT NULL, PRIMARY KEY (provider, label));
CREATE INDEX idx_history_message_companion ON history_message (companion_id);
CREATE INDEX idx_history_message_round ON history_message (round_id);
CREATE INDEX idx_undelivered_companion_status ON undelivered (companion_id, status);
CREATE UNIQUE INDEX idx_history_message_companion_command ON history_message (companion_id, command_id);
CREATE INDEX idx_paired_device_descriptor ON paired_device (descriptor);
CREATE TABLE _schema_version (version INTEGER NOT NULL);
INSERT INTO _schema_version (version) VALUES (4);",
            );
        assert!(shaped.is_ok(), "v4 shape must apply");
        let seeded = conn.execute(
            "INSERT INTO paired_device (device_id, descriptor, paired_at) VALUES (?1, ?2, ?3)",
            params![device_text, "legacy-phone", fixture_clock().to_rfc3339()],
        );
        assert!(seeded.is_ok(), "v4 paired row must seed");
    }
    let opened = Store::open(&path).await;
    let store = opened.unwrap();
    let found = store.find_device_by_wire(&device_text).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if stored.id == DeviceId(device_id)),
        "legacy device must stay resolvable through the continuity projection"
    );
    let stored = found.unwrap().unwrap();
    assert_eq!(
        stored.wire, device_text,
        "legacy backfill keeps the identity rendering so provisioned clients resolve"
    );
}

/// Reads the singleton without running migrations.
fn read_schema_version(path: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open(path).ok()?;
    conn.query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
        row.get(0)
    })
    .ok()
}

fn table_columns(path: &std::path::Path, table: &str) -> Vec<String> {
    let Ok(conn) = rusqlite::Connection::open(path) else {
        return Vec::new();
    };
    let Ok(mut query) = conn.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return Vec::new();
    };
    query
        .query_map([table], |row| row.get(0))
        .map(|rows| {
            rows.filter_map(std::result::Result::ok)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn migration_failure_rolls_back_and_reopen_recovers() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    {
        let conn = rusqlite::Connection::open(&path);
        let conn = conn.unwrap();
        let shaped = conn.execute_batch(
            "CREATE TABLE companion (companion_id TEXT PRIMARY KEY, lifecycle TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE history_message (message_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, round_id TEXT NOT NULL, role TEXT NOT NULL, body TEXT NOT NULL, lang TEXT NOT NULL, at TEXT NOT NULL, presence_generation INTEGER NOT NULL, local_id TEXT NULL, command_id TEXT NULL);
CREATE TABLE undelivered (undelivered_id TEXT PRIMARY KEY, companion_id TEXT NOT NULL, source_message TEXT NOT NULL, status TEXT NOT NULL, round_id TEXT NOT NULL, presence_generation INTEGER NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE presence_attribution (companion_id TEXT PRIMARY KEY, state TEXT NOT NULL, active_client TEXT, generation INTEGER NOT NULL);
CREATE TABLE presence_transition_log (transition_seq INTEGER PRIMARY KEY AUTOINCREMENT, companion_id TEXT NOT NULL, old_state TEXT NOT NULL, new_state TEXT NOT NULL, old_gen INTEGER NOT NULL, new_gen INTEGER NOT NULL, reason TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE consent_record (id TEXT PRIMARY KEY, rev INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, credential_id TEXT NOT NULL);
CREATE TABLE credential_ref (id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL, UNIQUE (provider, label));
CREATE TABLE usage_fact (ticket TEXT PRIMARY KEY, provider TEXT NOT NULL, model TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER, source TEXT NOT NULL);
CREATE TABLE pairing_pending (descriptor TEXT PRIMARY KEY, requested_at TEXT NOT NULL);
CREATE TABLE paired_device (device_id TEXT PRIMARY KEY, descriptor TEXT NOT NULL, paired_at TEXT NOT NULL);
CREATE TABLE credential_pending (provider TEXT NOT NULL, label TEXT NOT NULL, requested_at TEXT NOT NULL, PRIMARY KEY (provider, label));
CREATE INDEX idx_history_message_companion ON history_message (companion_id);
CREATE INDEX idx_history_message_round ON history_message (round_id);
CREATE INDEX idx_undelivered_companion_status ON undelivered (companion_id, status);
CREATE UNIQUE INDEX idx_history_message_companion_command ON history_message (companion_id, command_id);
CREATE INDEX idx_paired_device_descriptor ON paired_device (descriptor);
CREATE TABLE _schema_version (version INTEGER NOT NULL);
INSERT INTO _schema_version (version) VALUES (4);",
        );
        assert!(shaped.is_ok(), "v4 shape must apply");
        // One paired row so the backfill UPDATE below has a row to trip
        // the injected fault on: without rows the UPDATE touches nothing
        // and the fault would never fire.
        let seeded = conn.execute(
            "INSERT INTO paired_device (device_id, descriptor, paired_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                crate::codec::encode_id(RawId::new()),
                "legacy-phone",
                fixture_clock().to_rfc3339()
            ],
        );
        assert!(seeded.is_ok(), "v4 paired row must seed");
        // Fault injection: abort V5's backfill UPDATE after its four ALTERs
        // ran, simulating a crash mid-migration. The trigger is test-only
        // crash simulation, not a production hook.
        let injected = conn.execute_batch(
            "CREATE TRIGGER inject_crash BEFORE UPDATE ON paired_device BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        );
        assert!(injected.is_ok(), "fault trigger must install");
    }
    let failed = Store::open(&path).await;
    assert!(failed.is_err(), "faulted migration must fail open");
    assert_eq!(
        read_schema_version(&path),
        Some(4),
        "a failed migration must not advance the version"
    );
    assert!(
        !table_columns(&path, "history_message").contains(&String::from("round_wire")),
        "a failed migration must roll back its DDL"
    );
    assert!(
        !table_columns(&path, "history_message").contains(&String::from("round_intent")),
        "a failed migration must roll back its DDL"
    );
    {
        let conn = rusqlite::Connection::open(&path);
        let conn = conn.unwrap();
        let dropped = conn.execute_batch("DROP TRIGGER inject_crash;");
        assert!(dropped.is_ok(), "fault trigger must drop");
    }
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "open must recover after the fault clears");
    assert_eq!(
        read_schema_version(&path),
        Some(11),
        "recovered open must converge on the current version"
    );
    assert!(
        table_columns(&path, "history_message").contains(&String::from("round_wire")),
        "recovered open must apply the rolled-back DDL"
    );
    assert!(
        table_columns(&path, "history_message").contains(&String::from("round_intent")),
        "recovered open must apply the rolled-back DDL"
    );
}

#[tokio::test]
async fn migration_v3_reopen_keeps_pairing_state() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    let first = opened.unwrap();
    let requested = first.request_pairing(String::from("phone")).await;
    assert!(
        matches!(requested, Ok(DevicePairingStatus::Pending { .. })),
        "request must pend"
    );
    let approved = DevicePairingRepository::approve_pending(&first, "phone").await;
    let (device, _) = approved.unwrap().unwrap();
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let found = second.find_device(&device.id).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == device),
        "paired device must survive reopen"
    );
    let again = second.request_pairing(String::from("phone")).await;
    assert!(
        matches!(
            again,
            Ok(DevicePairingStatus::Paired { device: ref existing }) if *existing == device
        ),
        "paired state must survive reopen"
    );
    let guard = match second.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let version = guard.query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
        row.get::<_, i64>(0)
    });
    assert!(
        matches!(version, Ok(11)),
        "reopened database must record schema version 11"
    );
}

#[tokio::test]
async fn credential_approval_request_approve_usable_cycle() {
    let store = open_memory().await.unwrap();
    let unknown_approve =
        CredentialApprovalRepository::approve_pending(&store, "acme", "nope").await;
    assert!(
        matches!(unknown_approve, Ok(false)),
        "approving an unknown pair must yield false"
    );
    let unknown_flag = store.is_approved("acme", "nope").await;
    assert!(
        matches!(unknown_flag, Ok(false)),
        "unknown pair must not read as approved"
    );
    let listed_empty = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed_empty, Ok(ref items) if items.is_empty()),
        "fresh store pends no credential approvals"
    );
    let requested = store
        .request_approval(String::from("acme"), String::from("main"))
        .await;
    assert!(
        matches!(requested, Ok(true)),
        "first request must record a pending entry"
    );
    let rerequested = store
        .request_approval(String::from("acme"), String::from("main"))
        .await;
    assert!(
        matches!(rerequested, Ok(false)),
        "repeat request must not record again"
    );
    let listed = CredentialApprovalRepository::list_pending(&store).await;
    let items = listed.unwrap();
    assert_eq!(items.len(), 1, "one approval must pend");
    assert_eq!(items[0].provider.as_str(), "acme");
    assert_eq!(items[0].label.as_str(), "main");
    let flagged_pending = store.is_approved("acme", "main").await;
    assert!(
        matches!(flagged_pending, Ok(false)),
        "pending-only pair must not read as approved"
    );
    let approved = CredentialApprovalRepository::approve_pending(&store, "acme", "main").await;
    assert!(
        matches!(approved, Ok(true)),
        "approval of a pending pair must succeed"
    );
    let drained = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(drained, Ok(ref items) if items.is_empty()),
        "approval must drain the pending entry"
    );
    let flagged_before_ref = store.is_approved("acme", "main").await;
    assert!(
        matches!(flagged_before_ref, Ok(true)),
        "approval atomically records the usable ref in the same transaction"
    );
    let saved = store
        .save_ref(CredentialRef::new("acme", "main").expect("valid test fixture"))
        .await;
    assert!(saved.is_ok(), "usable ref save must succeed");
    let flagged = store.is_approved("acme", "main").await;
    assert!(
        matches!(flagged, Ok(true)),
        "pair with a stored ref must read as approved"
    );
    let reapproved = CredentialApprovalRepository::approve_pending(&store, "acme", "main").await;
    assert!(
        matches!(reapproved, Ok(true)),
        "re-approving a usable pair must stay true"
    );
    let again = store
        .request_approval(String::from("acme"), String::from("main"))
        .await;
    assert!(
        matches!(again, Ok(false)),
        "request after usable must not record"
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

#[tokio::test]
async fn complete_with_intent_decides_atomically() {
    use ene_permission::{
        ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint, IntentOutcome,
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
            String::from("consent-none"),
            true,
            fingerprint("c-0", "consent-none"),
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
    let saved = store
        .compare_and_save(
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
        matches!(saved, Ok(ConsentCommitOutcome::Committed { .. })),
        "consent must seed"
    );
    let unready = store
        .complete_with_intent(
            String::from("consent-rev-1"),
            false,
            fingerprint("c-1", "consent-rev-1"),
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
            String::from("consent-rev-1"),
            true,
            fingerprint("c-2", "consent-rev-1"),
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
            String::from("consent-none"),
            true,
            fingerprint("c-3", "consent-none"),
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
        ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint, IntentOutcome,
        IntentOutcomeRepository as _, IntentResolution, ShortcutIntentOutcome,
    };

    fn fingerprint(id: &str) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: id.to_owned(),
            kind: String::from("assign"),
            target: String::from("consent:dialogue:openai:dialogue-1:openai:main"),
            base: String::from("consent-rev-1"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        }
    }

    let store = open_memory().await.unwrap();
    let saved = store
        .compare_and_save(
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
        matches!(saved, Ok(ConsentCommitOutcome::Committed { .. })),
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
            Ok(IntentResolution::Decided(
                ShortcutIntentOutcome::Miss { .. }
            ))
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
    use ene_permission::{ConsentRecord, ConsentRepository as _, ConsentRevision};

    let store = open_memory().await.unwrap();
    let saved = store
        .compare_and_save(
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
        matches!(saved, Ok(ConsentCommitOutcome::Committed { .. })),
        "consent must seed"
    );
    let claim = |ticket: InferenceTicketId, rev: u64| InferenceAttempt {
        ticket,
        capability: CapabilityKind::Dialogue,
        expected_credential_set: CredentialSetRevision::initial(),
        expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(rev)),
        provider: String::from("openai"),
        model: String::from("dialogue-1"),
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
    let moved = store
        .compare_and_save(
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
        matches!(moved, Ok(ConsentCommitOutcome::Committed { .. })),
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
async fn credential_approval_blank_inputs_are_absent() {
    let store = open_memory().await.unwrap();
    let blank_provider = store
        .request_approval(String::new(), String::from("main"))
        .await;
    assert!(
        matches!(blank_provider, Ok(false)),
        "blank provider must record nothing"
    );
    let whitespace_provider = store
        .request_approval(String::from("   "), String::from("main"))
        .await;
    assert!(
        matches!(whitespace_provider, Ok(false)),
        "whitespace provider must record nothing"
    );
    let blank_label = store
        .request_approval(String::from("acme"), String::new())
        .await;
    assert!(
        matches!(blank_label, Ok(false)),
        "blank label must record nothing"
    );
    let whitespace_label = store
        .request_approval(String::from("acme"), String::from("  "))
        .await;
    assert!(
        matches!(whitespace_label, Ok(false)),
        "whitespace label must record nothing"
    );
    let both_blank = store.request_approval(String::new(), String::new()).await;
    assert!(
        matches!(both_blank, Ok(false)),
        "blank pair must record nothing"
    );
    let listed = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed, Ok(ref items) if items.is_empty()),
        "blank requests must leave pending empty"
    );
    let approve_blank_provider =
        CredentialApprovalRepository::approve_pending(&store, "", "main").await;
    assert!(
        matches!(approve_blank_provider, Ok(false)),
        "blank approve must yield false"
    );
    let approve_blank_label =
        CredentialApprovalRepository::approve_pending(&store, "acme", "   ").await;
    assert!(
        matches!(approve_blank_label, Ok(false)),
        "whitespace approve must yield false"
    );
    let approve_both_blank =
        CredentialApprovalRepository::approve_pending(&store, "  ", "  ").await;
    assert!(
        matches!(approve_both_blank, Ok(false)),
        "blank pair approve must yield false"
    );
    let flagged_blank = store.is_approved("", "main").await;
    assert!(
        matches!(flagged_blank, Ok(false)),
        "blank pair must never read as approved"
    );
    let flagged_blank_label = store.is_approved("acme", "").await;
    assert!(
        matches!(flagged_blank_label, Ok(false)),
        "blank label must never read as approved"
    );
    let flagged_both_blank = store.is_approved("   ", "  ").await;
    assert!(
        matches!(flagged_both_blank, Ok(false)),
        "whitespace pair must never read as approved"
    );
    let listed_after = CredentialApprovalRepository::list_pending(&store).await;
    assert!(
        matches!(listed_after, Ok(ref items) if items.is_empty()),
        "blank approves must record nothing"
    );
}

#[tokio::test]
async fn migration_v4_reopen_keeps_credential_approval_rows() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    let first = opened.unwrap();
    let pending_requested = first
        .request_approval(String::from("acme"), String::from("pending"))
        .await;
    assert!(
        matches!(pending_requested, Ok(true)),
        "pending request must record"
    );
    let usable_requested = first
        .request_approval(String::from("acme"), String::from("usable"))
        .await;
    assert!(
        matches!(usable_requested, Ok(true)),
        "usable request must record"
    );
    let usable_approved =
        CredentialApprovalRepository::approve_pending(&first, "acme", "usable").await;
    assert!(
        matches!(usable_approved, Ok(true)),
        "usable approval must succeed"
    );
    let usable_saved = first
        .save_ref(CredentialRef::new("acme", "usable").expect("valid test fixture"))
        .await;
    assert!(usable_saved.is_ok(), "usable ref save must succeed");
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let listed = CredentialApprovalRepository::list_pending(&second).await;
    let items = listed.unwrap();
    assert_eq!(items.len(), 1, "pending row must survive reopen");
    assert_eq!(items[0].provider.as_str(), "acme");
    assert_eq!(items[0].label.as_str(), "pending");
    let usable_flag = second.is_approved("acme", "usable").await;
    assert!(
        matches!(usable_flag, Ok(true)),
        "usable ref must survive reopen"
    );
    let pending_flag = second.is_approved("acme", "pending").await;
    assert!(
        matches!(pending_flag, Ok(false)),
        "pending-only pair must stay unapproved after reopen"
    );
    let rerequest = second
        .request_approval(String::from("acme"), String::from("pending"))
        .await;
    assert!(
        matches!(rerequest, Ok(false)),
        "pending state must survive reopen"
    );
    let reapprove = CredentialApprovalRepository::approve_pending(&second, "acme", "usable").await;
    assert!(
        matches!(reapprove, Ok(true)),
        "usable state must survive reopen"
    );
    let guard = match second.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let version = guard.query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
        row.get::<_, i64>(0)
    });
    assert!(
        matches!(version, Ok(11)),
        "reopened database must record schema version 11"
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
        )
        .await;
    assert!(second_append.is_ok(), "second append must succeed");
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let ensured_again = second.ensure_running_companion().await;
    let same = ensured_again.unwrap();
    assert_eq!(same, companion, "companion must survive restart");
    let loaded = second.load_timeline(companion, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 2, "both items must survive restart");
    assert_eq!(timeline[0].text, "first");
    assert_eq!(timeline[1].text, "second");
}

#[tokio::test]
async fn registration_intent_decides_held_then_already_decided() {
    use ene_credential::{
        CredentialIntentRepository as _, RegistrationApply, RegistrationFingerprint,
        RegistrationState,
    };

    fn fingerprint(id: &str) -> RegistrationFingerprint {
        RegistrationFingerprint {
            intent_id: id.to_owned(),
            kind: String::from("register"),
            target: String::from("credential:acme:main"),
            base: String::from("consent-none"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        }
    }

    let store = open_memory().await.unwrap();
    let first = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            fingerprint("reg-1"),
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
            fingerprint("reg-1"),
        )
        .await;
    assert_eq!(
        repeat,
        Ok(RegistrationApply::AlreadyDecided),
        "the same intent never decides twice"
    );
    let approved =
        ene_credential::CredentialApprovalRepository::approve_pending(&store, "acme", "main").await;
    assert!(matches!(approved, Ok(true)), "approval must apply");
    let usable = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            fingerprint("reg-2"),
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
        change,
    }
}

#[tokio::test]
async fn learning_new_memory_keeps_summary_grounds_and_current_row() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "owner likes jasmine tea");
    let outcome = store
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
    assert_eq!(
        outcome,
        Ok(MemoryChangeOutcome::Committed {
            memory,
            revision: MemoryRevision::initial(),
        })
    );

    let current = store.load_current_memory(memory).await.unwrap();
    let Some(current) = current else {
        panic!("the committed memory must be current");
    };
    assert_eq!(current.content, "owner likes jasmine tea");
    assert_eq!(current.scope, LearningScope::companion(companion));
    assert_eq!(current.importance, Importance::clamped(4));
    assert_eq!(current.temporal, TemporalMeaning::Enduring);

    let revisions = store.list_memory_revisions(memory).await.unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].change, ChangeKind::Initial);
    assert_eq!(revisions[0].summary, Some(evidence.id));
    assert_eq!(revisions[0].content, "owner likes jasmine tea");

    let stored_evidence = store.load_summary(evidence.id).await.unwrap();
    let Some(stored_evidence) = stored_evidence else {
        panic!("the evidence summary must be stored");
    };
    assert_eq!(stored_evidence.content, "owner likes jasmine tea");
    assert_eq!(stored_evidence.source.kind, ExperienceSourceKind::Dialogue);
    assert_eq!(stored_evidence.scope, LearningScope::companion(companion));
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
    assert_eq!(store.load_current_memory(second).await, Ok(None));
    let stored = store.load_summary(evidence.id).await.unwrap().unwrap();
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
        store.load_summary(evidence.id).await,
        Ok(None),
        "a rejected change must not strand its Summary"
    );
    assert_eq!(store.list_memory_revisions(memory).await.unwrap().len(), 2);
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
    let current = store.load_current_memory(memory).await.unwrap().unwrap();
    assert_eq!(current.content, "owner lives in Osaka");
    assert_eq!(current.revision, MemoryRevision::from_u64(2));

    let revisions = store.list_memory_revisions(memory).await.unwrap();
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
    let current = store.load_current_memory(memory).await.unwrap().unwrap();
    assert_eq!(
        current.content, "owner switched to the day shift",
        "the stale result must not overwrite the newer recognition"
    );
    assert_eq!(store.list_memory_revisions(memory).await.unwrap().len(), 2);
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
    let current = store.load_current_memory(memory).await.unwrap().unwrap();
    assert!(current.recall_suppressed, "recall is suppressed");
    assert_eq!(
        current.content, "owner was worried about the launch",
        "normal forgetting never deletes content"
    );
    let revisions = store.list_memory_revisions(memory).await.unwrap();
    assert_eq!(revisions.len(), 2, "revision history is kept");
    assert_eq!(revisions[0].content, "owner was worried about the launch");
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
    let current = store.load_current_memory(memory).await.unwrap().unwrap();
    assert!(!current.recall_suppressed);
    assert_eq!(store.list_memory_revisions(memory).await.unwrap().len(), 3);
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
    let current = reopened.load_current_memory(memory).await.unwrap();
    let Some(current) = current else {
        panic!("memory must survive reopen");
    };
    assert_eq!(current.content, "owner prefers morning conversations");
    let revisions = reopened.list_memory_revisions(memory).await.unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].summary, Some(evidence.id));
    let stored_evidence = reopened.load_summary(evidence.id).await.unwrap();
    assert!(
        stored_evidence.is_some(),
        "summary evidence survives reopen"
    );
}

#[tokio::test]
async fn learning_migration_adds_tables_to_a_v8_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        // A realistic v8 database carries the v7 attempt table the v10
        // migration alters; only the v9 learning group is still missing.
        conn.execute_batch(
            "CREATE TABLE _schema_version (version INTEGER NOT NULL);
             INSERT INTO _schema_version (version) VALUES (8);
             CREATE TABLE inference_attempt (
               ticket TEXT PRIMARY KEY,
               consent_id TEXT NOT NULL,
               consent_rev INTEGER NOT NULL,
               provider TEXT NOT NULL,
               model TEXT NOT NULL,
               started_at TEXT NOT NULL
             );",
        )
        .unwrap();
    }
    let store = Store::open(&path).await.unwrap();
    let opened = store.load_current_memory(MemoryId::generate()).await;
    assert_eq!(opened, Ok(None), "migrated schema answers reads");
    assert_eq!(
        read_schema_version(&path),
        Some(11),
        "migration advances the schema version"
    );
    assert!(
        !table_columns(&path, "learning_memory").is_empty(),
        "learning_memory is created"
    );
    assert!(
        !table_columns(&path, "learning_memory_revision").is_empty(),
        "learning_memory_revision is created"
    );
    assert!(
        !table_columns(&path, "learning_summary").is_empty(),
        "learning_summary is created"
    );
}

#[tokio::test]
async fn migration_v10_moves_stage2_consent_to_dialogue_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE _schema_version (version INTEGER NOT NULL);
             INSERT INTO _schema_version (version) VALUES (9);
             CREATE TABLE consent_record (id TEXT PRIMARY KEY, rev INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, credential_id TEXT NOT NULL);
             INSERT INTO consent_record VALUES ('consent-1', 1, 'openai', 'gpt-x', 'openai:main');
             CREATE TABLE inference_attempt (
               ticket TEXT PRIMARY KEY,
               consent_id TEXT NOT NULL,
               consent_rev INTEGER NOT NULL,
               provider TEXT NOT NULL,
               model TEXT NOT NULL,
               started_at TEXT NOT NULL
             );",
        )
        .unwrap();
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(read_schema_version(&path), Some(11));
    let dialogue = store
        .load_current(CapabilityKind::Dialogue)
        .await
        .unwrap()
        .expect("the Stage 2 row must survive as the dialogue assignment");
    assert_eq!(dialogue.id, "consent-1");
    assert_eq!(dialogue.capability, CapabilityKind::Dialogue);
    assert_eq!(dialogue.provider, "openai");
    assert_eq!(
        store.load_current(CapabilityKind::Learning).await,
        Ok(None),
        "an existing environment starts with learning unassigned"
    );
    assert!(
        table_columns(&path, "consent_record").contains(&String::from("capability")),
        "the rebuilt consent table is capability-scoped"
    );
    assert_eq!(
        table_columns(&path, "credential_set"),
        vec![String::from("id"), String::from("rev")],
        "the credential-set table stores non-secret revision metadata only"
    );
    assert_eq!(
        store.current_set_revision().await,
        Ok(CredentialSetRevision::initial()),
        "an existing environment starts before any registered credential"
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
    assert!(matches!(
        store
            .request_approval(String::from("openai"), String::from("main"))
            .await,
        Ok(true)
    ));

    assert!(
        store
            .approve_credential_with_sweep("openai", "main", "sk-test-only")
            .expect("the approval sweep must commit"),
        "the approval makes the pair usable"
    );

    let timeline = store.load_recent_timeline(companion, 10).await.unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].text, "the key is [credential]");
    let current = store.load_current_memory(memory).await.unwrap().unwrap();
    assert_eq!(current.content, "owner key [credential]");
    let revisions = store.list_memory_revisions(memory).await.unwrap();
    assert_eq!(revisions[0].content, "owner key [credential]");
    let stored = store.load_summary(evidence.id).await.unwrap().unwrap();
    assert_eq!(stored.content, "evidence mentions [credential]");
}

#[tokio::test]
async fn startup_sweep_fails_closed_when_a_registered_value_is_unreadable() {
    let store = open_memory().await.unwrap();
    let Some((companion, generation)) = running_companion(&store).await else {
        panic!("the running companion must resolve");
    };
    store
        .append_message(history_command(
            companion,
            generation,
            "the old key is sk-legacy",
        ))
        .await
        .unwrap();
    let readable = CredentialRef::new("openai", "main").unwrap();
    let unreadable = CredentialRef::new("openai", "other").unwrap();
    store.save_ref(readable.clone()).await.unwrap();
    store.save_ref(unreadable.clone()).await.unwrap();
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

#[tokio::test]
async fn stale_credential_set_refuses_history_append_after_approval() {
    // Two connections model two Host processes sharing one SQLite file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let writer = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let Some((companion, generation)) = running_companion(&writer).await else {
        panic!("the running companion must resolve");
    };
    // The writer scrubs its row under the current set premise.
    let premise = writer.current_set_revision().await.unwrap();
    // The approver concurrently sweeps + registers + bumps in one commit.
    assert!(matches!(
        approver
            .request_approval(String::from("acme"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-new"),
        Ok(true)
    ));
    // The writer's prepared row carries a value that just became registered;
    // the stale premise refuses it.
    let mut cmd = history_command(companion, generation, "the key is sk-new");
    cmd.expected_credential_set = Some(premise);
    let appended = writer.append_message(cmd).await;
    assert_eq!(
        appended,
        Ok(HistoryAppendOutcome::StaleCredentialSet),
        "a stale credential-set premise must refuse the raw append"
    );
    let timeline = writer.load_timeline(companion, None, 10).await.unwrap();
    assert!(
        timeline.iter().all(|item| !item.text.contains("sk-new")),
        "no raw bearer may land in History: {timeline:?}"
    );
}

#[tokio::test]
async fn stale_credential_set_refuses_memory_commit_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race-learning.db");
    let writer = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let companion = RawId::new();
    let premise = writer.current_set_revision().await.unwrap();
    assert!(matches!(
        approver
            .request_approval(String::from("acme"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-new"),
        Ok(true)
    ));
    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "evidence says sk-new");
    let outcome = writer
        .commit_memory_change(MemoryChangeCommit {
            summary: Some(evidence.clone()),
            secret_premise: Some(premise),
            change: learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner key sk-new",
                ChangeKind::Initial,
                false,
            ),
        })
        .await;
    assert_eq!(
        outcome,
        Ok(MemoryChangeOutcome::StaleCredentialSet),
        "a stale credential-set premise must refuse the Learning commit"
    );
    assert_eq!(writer.load_current_memory(memory).await, Ok(None));
    assert_eq!(writer.load_summary(evidence.id).await, Ok(None));
    assert!(
        writer
            .list_memory_revisions(memory)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_credential_set_refuses_attempt_claim_after_approval() {
    use ene_inference::{
        AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race-send.db");
    let sender = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let seeded = sender
        .compare_and_save(
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
    assert!(matches!(seeded, Ok(ConsentCommitOutcome::Committed { .. })));
    let premise = sender.current_set_revision().await.unwrap();
    assert!(matches!(
        approver
            .request_approval(String::from("openai"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("openai", "main", "sk-new"),
        Ok(true)
    ));
    let claim = sender
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            capability: CapabilityKind::Dialogue,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: premise,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
        })
        .await;
    assert_eq!(
        claim,
        Ok(AttemptBeginOutcome::Stale),
        "a stale credential-set premise must refuse the send claim"
    );
}

#[tokio::test]
async fn reapproval_with_a_new_value_refuses_a_stale_history_premise() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reapprove-history.db");
    let writer = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let Some((companion, generation)) = running_companion(&writer).await else {
        panic!("the running companion must resolve");
    };
    // The value A is usable; the in-flight premise names that generation.
    assert!(matches!(
        approver
            .request_approval(String::from("acme"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-a"),
        Ok(true)
    ));
    let premise = writer.current_set_revision().await.unwrap();
    // The Owner updates the value to B through a re-approval.
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-b"),
        Ok(true)
    ));
    let updated = writer.current_set_revision().await.unwrap();
    assert!(
        updated > premise,
        "a successful re-approval must advance the credential-set revision"
    );

    // The in-flight write prepared under A may carry B raw and is refused.
    let mut stale = history_command(companion, generation, "the new key is sk-b");
    stale.expected_credential_set = Some(premise);
    assert_eq!(
        writer.append_message(stale).await,
        Ok(HistoryAppendOutcome::StaleCredentialSet)
    );
    let timeline = writer.load_timeline(companion, None, 10).await.unwrap();
    assert!(
        timeline.iter().all(|item| !item.text.contains("sk-b")),
        "a stale premise must not re-save the updated value: {timeline:?}"
    );

    // Content prepared under the new revision commits normally.
    let mut fresh = history_command(companion, generation, "the new key is [credential]");
    fresh.expected_credential_set = Some(updated);
    assert!(matches!(
        writer.append_message(fresh).await,
        Ok(HistoryAppendOutcome::CommittedAs { .. })
    ));
}

#[tokio::test]
async fn reapproval_with_a_new_value_refuses_a_stale_memory_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reapprove-learning.db");
    let writer = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let companion = RawId::new();
    assert!(matches!(
        approver
            .request_approval(String::from("acme"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-a"),
        Ok(true)
    ));
    let premise = writer.current_set_revision().await.unwrap();
    assert!(matches!(
        approver.approve_credential_with_sweep("acme", "main", "sk-b"),
        Ok(true)
    ));
    let updated = writer.current_set_revision().await.unwrap();

    let memory = MemoryId::generate();
    let evidence = learning_summary(companion, "evidence says sk-b");
    let stale = writer
        .commit_memory_change(MemoryChangeCommit {
            summary: Some(evidence.clone()),
            secret_premise: Some(premise),
            change: learning_change(
                companion,
                MemoryTarget::New { id: memory },
                "owner key sk-b",
                ChangeKind::Initial,
                false,
            ),
        })
        .await;
    assert_eq!(stale, Ok(MemoryChangeOutcome::StaleCredentialSet));
    assert_eq!(writer.load_current_memory(memory).await, Ok(None));
    assert_eq!(writer.load_summary(evidence.id).await, Ok(None));
    assert!(
        writer
            .list_memory_revisions(memory)
            .await
            .unwrap()
            .is_empty()
    );

    let fresh_memory = MemoryId::generate();
    let fresh = writer
        .commit_memory_change(MemoryChangeCommit {
            summary: Some(learning_summary(companion, "fresh evidence")),
            secret_premise: Some(updated),
            change: learning_change(
                companion,
                MemoryTarget::New { id: fresh_memory },
                "owner key [credential]",
                ChangeKind::Initial,
                false,
            ),
        })
        .await;
    assert!(matches!(fresh, Ok(MemoryChangeOutcome::Committed { .. })));
}

#[tokio::test]
async fn reapproval_with_a_new_value_refuses_a_stale_attempt_claim() {
    use ene_inference::{
        AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reapprove-send.db");
    let sender = Store::open(&path).await.unwrap();
    let approver = Store::open(&path).await.unwrap();
    let seeded = sender
        .compare_and_save(
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
    assert!(matches!(seeded, Ok(ConsentCommitOutcome::Committed { .. })));
    assert!(matches!(
        approver
            .request_approval(String::from("openai"), String::from("main"))
            .await,
        Ok(true)
    ));
    assert!(matches!(
        approver.approve_credential_with_sweep("openai", "main", "sk-a"),
        Ok(true)
    ));
    let premise = sender.current_set_revision().await.unwrap();
    assert!(matches!(
        approver.approve_credential_with_sweep("openai", "main", "sk-b"),
        Ok(true)
    ));
    let updated = sender.current_set_revision().await.unwrap();
    assert!(updated > premise);

    let stale = sender
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            capability: CapabilityKind::Dialogue,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: premise,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
        })
        .await;
    assert_eq!(stale, Ok(AttemptBeginOutcome::Stale));

    let fresh = sender
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            capability: CapabilityKind::Dialogue,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: updated,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
        })
        .await;
    assert_eq!(fresh, Ok(AttemptBeginOutcome::Started));
}

#[tokio::test]
async fn recent_timeline_keeps_the_newest_window_in_order() {
    let store = open_memory().await.unwrap();
    let Some((companion, generation)) = running_companion(&store).await else {
        panic!("the running companion must resolve");
    };
    for text in ["one", "two", "three"] {
        let appended = store
            .append_message(history_command(companion, generation, text))
            .await;
        assert!(matches!(
            appended,
            Ok(HistoryAppendOutcome::CommittedAs { .. })
        ));
    }
    let recent = store.load_recent_timeline(companion, 2).await.unwrap();
    assert_eq!(recent.len(), 2, "the window is capped");
    assert_eq!(recent[0].text, "two", "oldest first within the window");
    assert_eq!(recent[1].text, "three");
}
