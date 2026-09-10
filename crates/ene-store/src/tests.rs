#![allow(
    clippy::expect_used,
    reason = "test fixtures construct validated credential refs"
)]

use crate::Store;
use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, PresentationMark, ReportStatus,
    ReportStatusTransition, RoundIntentMark, UndeliveredRef, UndeliveredRepository,
};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository, DeviceId,
    DevicePairingRepository, DevicePairingStatus,
};
use ene_inference::{InferenceTicketId, UsageFact, UsageRepository, UsageSource};
use ene_permission::{ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision};
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
        command_id: None,
        round_wire: Some(RawId::new().as_uuid().to_string()),
        round_intent: None,
        incarnation: Some((1, 2)),
        local_id: None,
    }
}

/// Builds a history command carrying an explicit replay key and
/// correspondence metadata. A keyed command always names its canonical
/// round intent, so its replay stays decidable from durable state.
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
        command_id,
        round_wire: Some(RawId::new().as_uuid().to_string()),
        round_intent: command_id.map(|_| RoundIntentMark::Auto),
        incarnation: Some((1, 2)),
        local_id: local_id.map(String::from),
    }
}

/// Counts durable history rows for one companion.
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
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "file open must succeed");
    let Ok(first) = opened else {
        return;
    };
    let ensured = first.ensure_running_companion().await;
    assert!(ensured.is_ok(), "ensure must succeed");
    let Ok(companion) = ensured else {
        return;
    };
    let lifecycle = first.load_lifecycle(companion).await;
    assert!(
        matches!(lifecycle, Ok(Some(CompanionLifecycle::Running))),
        "seeded companion must run"
    );
    drop(first);
    let reopened = Store::open(&path).await;
    assert!(reopened.is_ok(), "reopen must succeed");
    let Ok(second) = reopened else {
        return;
    };
    let ensured_again = second.ensure_running_companion().await;
    assert!(ensured_again.is_ok(), "re-ensure must succeed");
    let Ok(same) = ensured_again else {
        return;
    };
    assert_eq!(same, companion, "seed must be idempotent");
    let ping = lock_for_test(&second);
    assert!(ping.is_ok(), "reopened handle must serve queries");
}

#[tokio::test]
async fn append_message_commits_and_timeline_reads_back() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let appended = store
        .append_message(history_command(companion, generation, "hello history"))
        .await;
    assert!(appended.is_ok(), "append must succeed");
    let Ok(outcome) = appended else {
        return;
    };
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "happy path must commit"
    );
    let loaded = store.load_timeline(companion, None, 10).await;
    assert!(loaded.is_ok(), "timeline must load");
    let Ok(timeline) = loaded else {
        return;
    };
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].text, "hello history");
    assert_eq!(timeline[0].lang, "en");
    assert_eq!(timeline[0].presence_generation, generation);
    let bounded = store
        .load_timeline(companion, Some(fixture_clock()), 10)
        .await;
    assert!(bounded.is_ok(), "since filter must succeed");
    let Ok(kept) = bounded else {
        return;
    };
    assert_eq!(kept.len(), 1, "item at the bound must be kept");
    let capped = store.load_timeline(companion, None, 0).await;
    assert!(capped.is_ok(), "zero limit must succeed");
    let Ok(none) = capped else {
        return;
    };
    assert!(none.is_empty(), "zero limit must return nothing");
}

#[tokio::test]
async fn append_with_stale_generation_is_rejected() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let stale_number = generation.as_u64() + 1;
    let appended = store
        .append_message(history_command(
            companion,
            PresenceGeneration::from_u64(stale_number),
            "stale body",
        ))
        .await;
    assert!(appended.is_ok(), "stale append must be an outcome");
    let Ok(outcome) = appended else {
        return;
    };
    assert_eq!(
        outcome,
        HistoryAppendOutcome::StaleExpected {
            current: generation
        },
        "stale expectation must carry the current generation"
    );
    let loaded = store.load_timeline(companion, None, 10).await;
    let Ok(timeline) = loaded else {
        return;
    };
    assert!(timeline.is_empty(), "stale append must store nothing");
}

#[tokio::test]
async fn append_with_moved_consent_is_rejected() {
    use ene_companion::HistoryRepository as _;

    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let mut cmd = history_command(companion, generation, "consent body");
    cmd.expected_consent = Some((String::from("consent-1"), 999));
    let appended = store.append_message(cmd).await;
    assert!(appended.is_ok(), "consent mismatch must be an outcome");
    let Ok(outcome) = appended else {
        return;
    };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    assert!(appended.is_ok(), "held append must be an outcome");
    let Ok(outcome) = appended else {
        return;
    };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let appended = store
        .append_reply_with_undelivered(history_command(companion, generation, "reply body"), true)
        .await;
    assert!(appended.is_ok(), "reply append must succeed");
    let Ok((outcome, registered)) = appended else {
        return;
    };
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "reply must commit"
    );
    let Some(entry) = registered else {
        return;
    };
    assert_eq!(entry.status, ReportStatus::Pending);
    let pending = UndeliveredRepository::list_pending(&store, companion).await;
    assert!(pending.is_ok(), "pending list must succeed");
    let Ok(items) = pending else {
        return;
    };
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
    let Ok(drained) = pending_after else {
        return;
    };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    assert!(rejected.is_ok(), "stale begin must be an outcome");
    let Ok(decision) = rejected else {
        return;
    };
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
    assert!(begin.is_ok(), "matching begin must succeed");
    let Ok(MoveDecision::TransitioningToNew { generation: next }) = begin else {
        return;
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
    assert!(confirmed.is_ok(), "confirm must succeed");
    let Ok(ConfirmTransitionOutcome::Confirmed(fact)) = confirmed else {
        return;
    };
    assert_eq!(fact.state, PresenceState::Present);
    assert_eq!(fact.active_client, Some(client));
    assert_eq!(fact.generation, next);
}

#[tokio::test]
async fn confirm_by_unpinned_client_is_rejected_without_touching_state() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    assert!(begin.is_ok(), "matching begin must succeed");
    let Ok(MoveDecision::TransitioningToNew { generation: next }) = begin else {
        return;
    };
    // A live confirm for a different client must not crown it: the row
    // stays InTransition toward the pinned target.
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
    // The pinned client still confirms normally afterwards.
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
        id: String::from(id),
        rev: ConsentRevision::from_u64(rev),
        provider: String::from("acme"),
        model: String::from("dialogue-1"),
        credential_id: String::from("cred-1"),
    }
}

#[tokio::test]
async fn consent_compare_and_save_commit_and_stale_matrix() {
    let Some(store) = open_memory().await else {
        return;
    };
    let empty = store.load_current().await;
    assert!(matches!(empty, Ok(None)), "fresh store holds no consent");
    // No row plus no expectation: insert and commit.
    let first = consent_record("consent-1", 3);
    let committed = store.compare_and_save(None, first.clone()).await;
    assert!(
        matches!(
            committed,
            Ok(ConsentCommitOutcome::Committed { ref record }) if *record == first
        ),
        "empty store with no expectation must commit"
    );
    let loaded = store.load_current().await;
    assert!(matches!(loaded, Ok(Some(ref current)) if *current == first));
    // A row plus no expectation: stale, never overwritten.
    let intruder = consent_record("consent-9", 1);
    let unexpected = store.compare_and_save(None, intruder).await;
    assert!(
        matches!(
            unexpected,
            Ok(ConsentCommitOutcome::StaleCurrent { ref current }) if *current == Some(first.clone())
        ),
        "existing row with no expectation must be stale"
    );
    // Matching id and revision: overwrite and commit.
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
    // Same id, older revision: stale, stored row untouched.
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
    // Different id, same revision: stale, stored row untouched.
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
    let kept = store.load_current().await;
    assert!(
        matches!(kept, Ok(Some(ref current)) if *current == next),
        "stale attempts must leave the stored row untouched"
    );
}

#[tokio::test]
async fn consent_compare_and_save_expected_but_empty_is_stale() {
    let Some(store) = open_memory().await else {
        return;
    };
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
    let empty = store.load_current().await;
    assert!(matches!(empty, Ok(None)), "stale save must store nothing");
}

#[tokio::test]
async fn credential_save_load_and_list() {
    let Some(store) = open_memory().await else {
        return;
    };
    let missing = store.load_ref("acme", "main").await;
    assert!(matches!(missing, Ok(None)), "fresh store holds no refs");
    let cred = CredentialRef::new("acme", "main").expect("valid test fixture");
    let saved = store.save_ref(cred.clone()).await;
    assert!(saved.is_ok(), "ref save must succeed");
    let loaded = store.load_ref("acme", "main").await;
    assert!(loaded.is_ok(), "ref load must succeed");
    let Ok(Some(found)) = loaded else {
        return;
    };
    assert_eq!(found, cred, "ref must round-trip");
    let second = CredentialRef::new("acme", "backup").expect("valid test fixture");
    let saved_second = store.save_ref(second.clone()).await;
    assert!(saved_second.is_ok(), "second ref save must succeed");
    let listed = store.list_refs().await;
    assert!(listed.is_ok(), "ref list must succeed");
    let Ok(refs) = listed else {
        return;
    };
    assert_eq!(refs.len(), 2, "both refs must list");
    assert!(refs.contains(&cred), "first ref must list");
    assert!(refs.contains(&second), "second ref must list");
}

#[tokio::test]
async fn usage_insert_preserves_null_tokens() {
    let Some(store) = open_memory().await else {
        return;
    };
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
        assert!(checked.is_ok(), "usage row must read back");
        let Ok((input, output, source)) = checked else {
            return;
        };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let absent = store.lookup_local_id(companion, "send-1").await;
    assert!(
        matches!(absent, Ok(None)),
        "unknown local id must find nothing"
    );
    // Durable replay keys on `command_id` now; `local_id` is stored
    // correspondence metadata, so repeating it appends again instead of
    // replaying the first accept.
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
    let Ok(first_outcome) = first else {
        return;
    };
    let Ok(second_outcome) = second else {
        return;
    };
    assert_ne!(
        first_outcome, second_outcome,
        "local id repeats must mint distinct messages"
    );
    let found = store.lookup_local_id(companion, "send-1").await;
    assert!(found.is_ok(), "correspondence lookup must succeed");
    let Ok(Some(item)) = found else {
        return;
    };
    assert_eq!(item.local_id.as_deref(), Some("send-1"));
    assert_eq!(item.command_id, None);
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "both local id repeats must persist");
}

#[tokio::test]
async fn command_replay_returns_original_accept_without_duplicate_row() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    assert!(first.is_ok(), "first command append must succeed");
    let Ok((first_outcome, first_registered)) = first else {
        return;
    };
    let HistoryAppendOutcome::CommittedAs { message: first_id } = first_outcome else {
        return;
    };
    assert!(
        first_registered.is_some(),
        "first commit registers undelivered"
    );
    // Transport retry reuses the command id with a fresh message id but
    // identical content: the replay must return the original accept
    // without a second row or a second undelivered registration.
    let retry = store
        .append_reply_with_undelivered(base.clone(), true)
        .await;
    assert!(retry.is_ok(), "command retry must succeed");
    let Ok((retry_outcome, retry_registered)) = retry else {
        return;
    };
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
    assert!(looked_up.is_ok(), "replayed command must stay lookable");
    let Ok(Some(original)) = looked_up else {
        return;
    };
    assert_eq!(original.round, round, "replay must name the original round");
    assert!(
        retry_registered.is_none(),
        "replay must not re-register undelivered"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(1), "replay must not append a second row");
    let pending = UndeliveredRepository::list_pending(&store, companion).await;
    assert!(pending.is_ok(), "pending list must succeed");
    let Ok(items) = pending else {
        return;
    };
    assert_eq!(items.len(), 1, "replay must not duplicate undelivered");
    let looked_up = store.lookup_command(companion, &command).await;
    assert!(looked_up.is_ok(), "command lookup must succeed");
    let Ok(Some(item)) = looked_up else {
        return;
    };
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
    // Every request-semantics field decides: body, language, sending
    // incarnation, and the canonical client round intent.
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
    // Round identity and its wire projection are the accepted result,
    // not the request: the same send re-intaked into a newer round
    // replays the stored accept instead of conflicting.
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

/// A keyed command without a stored round intent proves nothing: the
/// replay attempt fails closed (declined, never guessed) whether the
/// stored row or the incoming command is the one missing the intent.
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
    // Same key, same request fields, but the caller dropped the intent:
    // sameness cannot be proven, so the key is declined.
    let mut no_intent = keyed.clone();
    no_intent.round_intent = None;
    let attempt = store.append_message(no_intent).await;
    assert!(
        matches!(attempt, Ok(HistoryAppendOutcome::CommandConflict)),
        "a keyed command without a round intent must not exact-replay, got {attempt:?}"
    );
    // The mirrored case: a keyed row stored without an intent (a
    // pre-mark row) declines every later replay attempt too.
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    let Ok(first_outcome) = first else {
        return;
    };
    let Ok(second_outcome) = second else {
        return;
    };
    assert_ne!(
        first_outcome, second_outcome,
        "distinct commands must mint distinct messages"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "distinct commands must persist twice");
}

#[tokio::test]
async fn null_command_appends_never_collide() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    let Ok(first_outcome) = first else {
        return;
    };
    let Ok(second_outcome) = second else {
        return;
    };
    assert_ne!(
        first_outcome, second_outcome,
        "NULL commands must mint distinct messages"
    );
    let count = history_row_count(&store, companion);
    assert_eq!(count, Some(2), "NULL commands must persist twice");
}

#[tokio::test]
async fn lookup_command_roundtrip_returns_both_ids() {
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
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
    assert!(appended.is_ok(), "command append must succeed");
    let Ok(HistoryAppendOutcome::CommittedAs { message }) = appended else {
        return;
    };
    let found = store.lookup_command(companion, &command).await;
    assert!(found.is_ok(), "command lookup must succeed");
    let Ok(Some(item)) = found else {
        return;
    };
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
    assert!(by_local.is_ok(), "local lookup must succeed");
    let Ok(Some(same)) = by_local else {
        return;
    };
    assert_eq!(same.id, message);
    assert_eq!(same.command_id, Some(command));
    let loaded = store.load_timeline(companion, None, 10).await;
    assert!(loaded.is_ok(), "timeline must load");
    let Ok(timeline) = loaded else {
        return;
    };
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].id, message);
    assert_eq!(timeline[0].command_id, Some(command));
    assert_eq!(timeline[0].local_id.as_deref(), Some("send-9"));
}

#[tokio::test]
async fn migration_v3_keeps_pre_command_rows_readable() {
    let dir = tempfile::tempdir();
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let companion = CompanionId::from_raw(RawId::new());
    let companion_text = crate::codec::encode_id(companion.as_raw());
    let message_id = RawId::new();
    let message_text = crate::codec::encode_id(message_id);
    let round_id = RawId::new();
    let round_text = crate::codec::encode_id(round_id);
    {
        let conn = rusqlite::Connection::open(&path);
        assert!(conn.is_ok(), "raw v2 file must open");
        let Ok(conn) = conn else {
            return;
        };
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
    assert!(opened.is_ok(), "open must migrate v2 to v5");
    let Ok(store) = opened else {
        return;
    };
    let loaded = store.load_timeline(companion, None, 10).await;
    assert!(loaded.is_ok(), "migrated timeline must load");
    let Ok(timeline) = loaded else {
        return;
    };
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
    assert!(by_local.is_ok(), "legacy local lookup must succeed");
    let Ok(Some(legacy)) = by_local else {
        return;
    };
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
    assert!(matches!(version, Ok(8)), "migration must record version 8");
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
    let Some(store) = open_memory().await else {
        return;
    };
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
    let Ok(DevicePairingStatus::Pending {
        pending: first_pending,
    }) = first
    else {
        return;
    };
    assert_eq!(first_pending.descriptor.as_str(), "phone");
    // A second request returns the stored entry without refreshing it.
    let second = store.request_pairing(String::from("phone")).await;
    assert!(
        matches!(second, Ok(DevicePairingStatus::Pending { .. })),
        "repeat request must stay pending"
    );
    let Ok(DevicePairingStatus::Pending {
        pending: second_pending,
    }) = second
    else {
        return;
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
    let Ok(items) = listed else {
        return;
    };
    assert_eq!(items.len(), 2, "both descriptors must list as pending");
    let unknown = DevicePairingRepository::approve_pending(&store, "unknown").await;
    assert!(
        matches!(unknown, Ok(None)),
        "approving an unknown descriptor must yield none"
    );
    let approved = DevicePairingRepository::approve_pending(&store, "phone").await;
    assert!(approved.is_ok(), "approval must succeed");
    let Ok(Some((device, secret))) = approved else {
        return;
    };
    assert_eq!(device.descriptor.as_str(), "phone");
    assert!(
        secret.len() == 36 && secret.chars().filter(|c| *c == '-').count() == 4,
        "approval must mint a UUID-text one-time secret"
    );
    let pending_after = DevicePairingRepository::list_pending(&store).await;
    let Ok(remaining) = pending_after else {
        return;
    };
    assert_eq!(remaining.len(), 1, "approval must drain one entry");
    assert_eq!(remaining[0].descriptor.as_str(), "tablet");
    let found = store.find_device(&device.id).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == device),
        "approved device must be findable by id"
    );
    // Re-requesting a paired descriptor returns the stored record.
    let again = store.request_pairing(String::from("phone")).await;
    assert!(
        matches!(
            again,
            Ok(DevicePairingStatus::Paired { device: ref existing }) if *existing == device
        ),
        "re-request after pairing must return the stored record"
    );
    // Re-approving returns the same record without minting a new id,
    // but with a freshly minted secret (rotation).
    let reapproved = DevicePairingRepository::approve_pending(&store, "phone").await;
    assert!(reapproved.is_ok(), "re-approval must succeed");
    let Ok(Some((same, rotated))) = reapproved else {
        return;
    };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let requested = store.request_pairing(String::from("phone")).await;
    assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
    let approved = DevicePairingRepository::approve_pending(&store, "phone").await;
    let Ok(Some((device, _))) = approved else {
        return;
    };
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
    let Some(store) = open_memory().await else {
        return;
    };
    let Some((companion, generation)) = running_companion(&store).await else {
        return;
    };
    let mut cmd = history_command(companion, generation, "opaque body");
    cmd.round_wire = Some(String::from("round-wire-9"));
    cmd.incarnation = Some((7, 11));
    let appended = store.append_message(cmd).await;
    let Ok(HistoryAppendOutcome::CommittedAs { message }) = appended else {
        return;
    };
    let loaded = store.load_timeline(companion, None, 10).await;
    let Ok(timeline) = loaded else {
        return;
    };
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
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let device_id = RawId::new();
    let device_text = crate::codec::encode_id(device_id);
    {
        let conn = rusqlite::Connection::open(&path);
        assert!(conn.is_ok(), "raw v4 file must open");
        let Ok(conn) = conn else {
            return;
        };
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
    assert!(opened.is_ok(), "open must migrate v4 to v5");
    let Ok(store) = opened else {
        return;
    };
    let found = store.find_device_by_wire(&device_text).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if stored.id == DeviceId(device_id)),
        "legacy device must stay resolvable through the continuity projection"
    );
    let Ok(Some(stored)) = found else {
        return;
    };
    assert_eq!(
        stored.wire, device_text,
        "legacy backfill keeps the identity rendering so provisioned clients resolve"
    );
}

/// Reads the `_schema_version` singleton through a throwaway
/// connection, without running migrations.
fn read_schema_version(path: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open(path).ok()?;
    conn.query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
        row.get(0)
    })
    .ok()
}

/// Column names of one table through a throwaway connection.
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
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    {
        let conn = rusqlite::Connection::open(&path);
        assert!(conn.is_ok(), "raw v4 file must open");
        let Ok(conn) = conn else {
            return;
        };
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
        // Fault injection: abort V5's backfill UPDATE *after* its four
        // ALTERs ran, simulating a crash mid-migration. No production
        // hook is involved — the trigger is test-only crash simulation.
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
    // Clearing the fault lets the next open resume from scratch and
    // converge on the current schema.
    {
        let conn = rusqlite::Connection::open(&path);
        assert!(conn.is_ok(), "file must reopen for cleanup");
        let Ok(conn) = conn else {
            return;
        };
        let dropped = conn.execute_batch("DROP TRIGGER inject_crash;");
        assert!(dropped.is_ok(), "fault trigger must drop");
    }
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "open must recover after the fault clears");
    assert_eq!(
        read_schema_version(&path),
        Some(8),
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
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "file open must succeed");
    let Ok(first) = opened else {
        return;
    };
    let requested = first.request_pairing(String::from("phone")).await;
    assert!(
        matches!(requested, Ok(DevicePairingStatus::Pending { .. })),
        "request must pend"
    );
    let approved = DevicePairingRepository::approve_pending(&first, "phone").await;
    let Ok(Some((device, _))) = approved else {
        return;
    };
    drop(first);
    let reopened = Store::open(&path).await;
    assert!(reopened.is_ok(), "reopen after pairing must succeed");
    let Ok(second) = reopened else {
        return;
    };
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
        matches!(version, Ok(8)),
        "reopened database must record schema version 8"
    );
}

#[tokio::test]
async fn credential_approval_request_approve_usable_cycle() {
    let Some(store) = open_memory().await else {
        return;
    };
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
    assert!(listed.is_ok(), "pending list must succeed");
    let Ok(items) = listed else {
        return;
    };
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
                target: String::from("consent:openai:dialogue-1:openai:main"),
                base: String::from("consent-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
            outcome: IntentOutcome::StoredAsRuleView {
                revision: String::from("1"),
            },
        }
    }

    let Some(store) = open_memory().await else {
        return;
    };
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
    // Write-once: re-recording the same intent replays instead of
    // refreshing, even with a different outcome attached.
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
    // Same id, different content: conflict, original preserved.
    let mut other = record();
    other.fingerprint.target = String::from("consent:openai:other:openai:main");
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
                target: String::from("consent:openai:dialogue-1:openai:main"),
                base: String::from("consent-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
            outcome: IntentOutcome::StoredAsRuleView {
                revision: String::from("1"),
            },
        }
    }

    let Some(store) = open_memory().await else {
        return;
    };
    let committed = store
        .assign_with_intent(
            None,
            ConsentRecord {
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
                        current: String::from("consent-rev-1"),
                    }
        ),
        "stale assign must leave its stale snapshot, got {stale_row:?}"
    );
    // Same id, same fingerprint: replays the stored row without
    // re-running compare-and-save or touching consent.
    let replayed = store
        .assign_with_intent(
            None,
            ConsentRecord {
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
    // Same id, different content: conflicts without side effects — the
    // original row and the consent record both survive untouched.
    let conflicted = store
        .assign_with_intent(
            None,
            ConsentRecord {
                id: String::from("consent-9"),
                rev: ConsentRevision::from_u64(9),
                provider: String::from("other"),
                model: String::from("other"),
                credential_id: String::from("other"),
            },
            IntentFingerprint {
                target: String::from("consent:openai:changed:openai:main"),
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
    let timeline = store.load_current().await;
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

    let Some(store) = open_memory().await else {
        return;
    };
    // Two sends of one logical intent racing through the same store:
    // the shared-connection mutex serializes whole transactions, so
    // the loser always observes the winner's row. Exactly one decides;
    // the other replays — the answer never forks and the row is never
    // rewritten.
    let attempt = |model: &'static str| {
        let store = &store;
        let fingerprint = IntentFingerprint {
            intent_id: String::from("race-1"),
            kind: String::from("assign"),
            target: String::from("consent:openai:dialogue-1:openai:main"),
            base: String::from("consent-none"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        };
        async move {
            store
                .assign_with_intent(
                    None,
                    ConsentRecord {
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

    let Some(store) = open_memory().await else {
        return;
    };
    // Empty premise with no consent: clarify, recorded.
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
    // Fresh base but bearer absent: clarify, recorded.
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
    // Fresh base and bearer: applied, recorded.
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
    // Stale base: stale snapshot with the current mark, recorded.
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
                    current: String::from("consent-rev-1"),
                }
        ),
        "moved base must report stale with the current mark, got {stale:?}"
    );
    let found = store.lookup_intent_outcome("c-3").await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if stored.outcome == IntentOutcome::StaleBaseView {
            current: String::from("consent-rev-1"),
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
            target: String::from("consent:openai:dialogue-1:openai:main"),
            base: String::from("consent-rev-1"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        }
    }

    let Some(store) = open_memory().await else {
        return;
    };
    let saved = store
        .compare_and_save(
            None,
            ConsentRecord {
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
                        revision: String::from("1"),
                    }
        ),
        "the hit must leave its snapshot, got {found:?}"
    );
    let miss = store
        .shortcut_with_intent(
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

    let Some(store) = open_memory().await else {
        return;
    };
    let saved = store
        .compare_and_save(
            None,
            ConsentRecord {
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
        expected_consent: (String::from("consent-1"), rev),
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
    let Some(store) = open_memory().await else {
        return;
    };
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
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "file open must succeed");
    let Ok(first) = opened else {
        return;
    };
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
    assert!(reopened.is_ok(), "reopen must succeed");
    let Ok(second) = reopened else {
        return;
    };
    let listed = CredentialApprovalRepository::list_pending(&second).await;
    assert!(listed.is_ok(), "pending list must survive reopen");
    let Ok(items) = listed else {
        return;
    };
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
        matches!(version, Ok(8)),
        "reopened database must record schema version 8"
    );
}

#[tokio::test]
async fn restart_keeps_timeline_intact() {
    let dir = tempfile::tempdir();
    assert!(dir.is_ok(), "tempdir must open");
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    assert!(opened.is_ok(), "file open must succeed");
    let Ok(first) = opened else {
        return;
    };
    let ensured = first.ensure_running_companion().await;
    assert!(ensured.is_ok(), "ensure must succeed");
    let Ok(companion) = ensured else {
        return;
    };
    let attributed = first.load_attribution(companion.as_raw()).await;
    let Ok(Some(current)) = attributed else {
        return;
    };
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
    assert!(reopened.is_ok(), "reopen must succeed");
    let Ok(second) = reopened else {
        return;
    };
    let ensured_again = second.ensure_running_companion().await;
    let Ok(same) = ensured_again else {
        return;
    };
    assert_eq!(same, companion, "companion must survive restart");
    let loaded = second.load_timeline(companion, None, 10).await;
    assert!(loaded.is_ok(), "timeline must load after restart");
    let Ok(timeline) = loaded else {
        return;
    };
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

    let Some(store) = open_memory().await else {
        return;
    };
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
