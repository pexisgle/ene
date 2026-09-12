use crate::Store;
use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, PresentationMark, ReportStatus,
    ReportStatusTransition, RoundIntentMark, UndeliveredRepository,
};
use ene_credential::{
    CredentialApprovalRepository, CredentialIntentRepository as _, CredentialRef,
    CredentialRefRepository, CredentialSetRepository, CredentialSetRevision, DeviceId,
    DevicePairingRepository, DevicePairingStatus, MemoryCredentialStore, RegistrationApply,
    RegistrationFingerprint, RegistrationState,
};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _,
    InferenceTechnicalError, InferenceTicketId, TaskAgentAttemptPremise, UsageFact,
    UsageRepository, UsageSource,
};
use ene_learning::{
    ChangeKind, ExperienceSourceKind, Importance, LearningRepository, LearningScope,
    LearningTechnicalError, MemoryChange, MemoryChangeCommit, MemoryChangeOutcome, MemoryId,
    MemoryRevision, MemoryTarget, SourceRangeRef, SummaryId, SummaryRecord, TemporalMeaning,
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
    TaskPurposeAdoptionPremise, TaskPurposeRef, TaskRef, TaskRepository, TaskRevision,
    TaskTechnicalError, WorkspaceAssocId, WorkspaceAssociationPremise, WorkspaceFolderRef,
    WorkspaceNeedRef,
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
        expected_owner_message: None,
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
    let loaded = store.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1, "one item must read back");
    assert_eq!(timeline[0].text, "hello history");
    assert_eq!(timeline[0].lang, "en");
    assert_eq!(timeline[0].presence_generation, generation);
    let bounded = store
        .load_timeline(companion, Some(fixture_clock()), None, 10)
        .await;
    let kept = bounded.unwrap();
    assert_eq!(kept.len(), 1, "item at the bound must be kept");
    let capped = store.load_timeline(companion, None, None, 0).await;
    let none = capped.unwrap();
    assert!(none.is_empty(), "zero limit must return nothing");
}

/// One raw batch insert bypasses the per-append transaction so a large
/// fixture stays fast. Rows carry the same canonical `at_utc` the production
/// append writes. `start_date` is a `YYYY-MM-DD` prefix; each row is one
/// second later starting at midnight (counts stay within one day). Returns
/// `(message, round, round_wire)` per row.
fn insert_history_batch(
    store: &Store,
    companion: CompanionId,
    count: usize,
    start_date: &str,
    round: Option<RawId>,
) -> Vec<(RawId, RawId, String)> {
    assert!(count < 86_400, "the fixture stays within one day");
    let companion_text = crate::codec::encode_id(companion.as_raw());
    let mut guard = store.conn.lock().expect("store lock");
    let tx = guard
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .expect("batch transaction must open");
    let mut inserted = Vec::with_capacity(count);
    // Every row of one round carries that round's single stored projection,
    // exactly like the production append path.
    let round_wire = RawId::new().as_uuid().to_string();
    for index in 0..count {
        let second = index as u32;
        let at = WallClockWithTz::parse_rfc3339(&format!(
            "{start_date}T{:02}:{:02}:{:02}Z",
            second / 3600,
            (second / 60) % 60,
            second % 60
        ))
        .expect("fixture timestamp must parse");
        let message = RawId::new();
        let round = round.unwrap_or_default();
        tx.execute(
            "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, at_utc, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, NULL, NULL, ?9, NULL, NULL, NULL, NULL)",
            params![
                crate::codec::encode_id(message),
                companion_text,
                crate::codec::encode_id(round),
                "owner",
                format!("row {index}"),
                "en",
                at.to_rfc3339(),
                at.to_rfc3339_utc(),
                round_wire,
            ],
        )
        .expect("batch row must insert");
        inserted.push((message, round, round_wire.clone()));
    }
    tx.commit().expect("batch must commit");
    inserted
}

/// `at_utc` keeps query comparison exact across offsets; the display `at`
/// keeps its creation offset.
#[tokio::test]
async fn history_since_compares_instants_across_offsets() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    // Inserted first but earlier in time: a lexical comparison of the raw
    // offset renderings would order these two the other way around.
    let mut early = history_command(companion, generation, "early");
    early.at = WallClockWithTz::parse_rfc3339("2026-09-12T10:00:00+09:00")
        .expect("fixture timestamp must parse");
    let mut late = history_command(companion, generation, "late");
    late.at = WallClockWithTz::parse_rfc3339("2026-09-12T00:30:00-05:00")
        .expect("fixture timestamp must parse");
    assert!(store.append_message(early).await.is_ok());
    assert!(store.append_message(late).await.is_ok());

    let bound = WallClockWithTz::parse_rfc3339("2026-09-12T02:00:00Z")
        .expect("fixture timestamp must parse");
    let loaded = store.load_timeline(companion, Some(bound), None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(
        timeline.len(),
        1,
        "the offset comparison must select the later instant only"
    );
    assert_eq!(timeline[0].text, "late");
    assert_eq!(
        timeline[0].at.to_rfc3339(),
        "2026-09-12T00:30:00-05:00",
        "the display rendering keeps the creation offset"
    );
}

/// The limit bounds SQLite's scan and the Rust decode, not only the returned
/// vector: a malformed row beyond the limit never reaches the decoder.
#[tokio::test]
async fn history_limit_bounds_decoding_not_only_the_returned_vector() {
    let store = open_memory().await.unwrap();
    let (companion, _generation) = running_companion(&store).await.unwrap();
    insert_history_batch(&store, companion, 20_000, "2026-01-01", None);
    // A poisoned row after every well-formed one: any implementation that
    // decodes the whole table before truncating fails here.
    {
        let guard = store.conn.lock().expect("store lock");
        guard
            .execute(
                "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, at_utc, presence_generation) VALUES (?1, ?2, ?3, 'owner', 'poison', 'en', 'not-a-timestamp', '2027-01-01T00:00:00.000000000Z', 0)",
                params![
                    RawId::new().as_uuid().hyphenated().to_string(),
                    crate::codec::encode_id(companion.as_raw()),
                    crate::codec::encode_id(RawId::new()),
                ],
            )
            .expect("poison row must insert");
    }
    let loaded = store.load_timeline(companion, None, None, 1).await;
    let timeline = loaded.expect("limit 1 must not decode the whole table");
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].text, "row 0");
}

/// Round scope comes from the stored projection: an old round is addressable
/// even though the caller's default window would never contain it.
#[tokio::test]
async fn history_round_scope_is_independent_of_the_recent_window() {
    let store = open_memory().await.unwrap();
    let (companion, _generation) = running_companion(&store).await.unwrap();
    let old_round = RawId::new();
    let old_rows = insert_history_batch(&store, companion, 120, "2026-01-01", Some(old_round));
    insert_history_batch(&store, companion, 120, "2026-02-01", None);

    let wire = &old_rows[0].2;
    let resolved = store.round_for_stored_wire(companion, wire).await;
    assert_eq!(
        resolved,
        Ok(Some(old_round)),
        "the stored projection resolves to its domain round"
    );
    assert_eq!(
        store.round_for_stored_wire(companion, "no-such-wire").await,
        Ok(None)
    );
    let loaded = store
        .load_timeline(companion, None, Some(old_round), 500)
        .await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 120, "the whole old round comes back");
    assert!(timeline.iter().all(|item| item.text.starts_with("row ")));
    assert_eq!(
        timeline[0].round_wire.as_deref(),
        Some(wire.as_str()),
        "items stay oldest-first and carry the stored projection"
    );

    let wide = store.load_timeline(companion, None, None, 500).await;
    let all = wide.unwrap();
    assert_eq!(all.len(), 240, "the unfiltered window still sees both");
}

#[tokio::test]
async fn history_limit_zero_is_empty_and_huge_limits_saturate() {
    let store = open_memory().await.unwrap();
    let (companion, _generation) = running_companion(&store).await.unwrap();
    insert_history_batch(&store, companion, 3, "2026-01-01", None);
    let zero = store.load_timeline(companion, None, None, 0).await;
    assert_eq!(zero, Ok(Vec::new()), "zero means no items, not an error");
    let huge = store.load_timeline(companion, None, None, u64::MAX).await;
    assert_eq!(
        huge.map(|items| items.len()),
        Ok(3),
        "a limit beyond i64 saturates instead of failing"
    );
}

/// Rewinds a store to the pre-v13 shape and proves the reopen backfills the
/// canonical timestamps and the offset display rendering survives.
#[tokio::test]
async fn migration_backfills_canonical_timestamps() {
    let dir = tempfile::tempdir().expect("tempdir must exist");
    let path = dir.path().join("store.db");
    let store = Store::open(&path).await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let mut command = history_command(companion, generation, "legacy row");
    command.at = WallClockWithTz::parse_rfc3339("2026-09-12T10:00:00+09:00")
        .expect("fixture timestamp must parse");
    assert!(store.append_message(command).await.is_ok());
    {
        let guard = store.conn.lock().expect("store lock");
        guard
            .execute_batch(
                "DROP INDEX IF EXISTS idx_history_message_companion_at;
                 ALTER TABLE history_message DROP COLUMN at_utc;
                 PRAGMA user_version = 12;",
            )
            .expect("the pre-v13 fixture must apply");
    }
    drop(store);

    let reopened = Store::open(&path).await.expect("the reopen must migrate");
    let loaded = reopened.load_timeline(companion, None, None, 10).await;
    let timeline = loaded.unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(
        timeline[0].at.to_rfc3339(),
        "2026-09-12T10:00:00+09:00",
        "the display rendering keeps its offset"
    );
    let bound = WallClockWithTz::parse_rfc3339("2026-09-12T02:00:00+09:00")
        .expect("fixture timestamp must parse");
    let filtered = reopened
        .load_timeline(companion, Some(bound), None, 10)
        .await;
    assert_eq!(
        filtered.map(|items| items.len()),
        Ok(1),
        "the backfilled projection serves since filters"
    );
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
        .append_reply_with_undelivered(stale_reply, true)
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
        .append_reply_with_undelivered(current_reply, true)
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
    let loaded = store.load_timeline(companion, None, None, 10).await;
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
PRAGMA user_version = 2;",
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
    let loaded = store.load_timeline(companion, None, None, 10).await;
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
    let version = guard.query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0));
    assert!(
        matches!(version, Ok(18)),
        "migration must record version 17"
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
    let missing = store.find_device_by_wire("no-such-wire").await;
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
    let found = store.find_device_by_wire(&device.wire).await;
    assert!(
        matches!(found, Ok(Some(ref stored)) if *stored == device),
        "approved device must be findable by wire"
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
PRAGMA user_version = 4;",
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

/// Reads the file's `user_version` without running migrations.
fn read_schema_version(path: &std::path::Path) -> Option<i64> {
    let conn = rusqlite::Connection::open(path).ok()?;
    conn.query_row("PRAGMA user_version", (), |row| row.get(0))
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

/// Every main-schema table name except SQLite's own internals, sorted.
/// Comparing a migrated file against a fresh one proves the rebuild left no
/// staging table behind; the fresh list is never hand-maintained.
fn schema_table_names(path: &std::path::Path) -> Vec<String> {
    let Ok(conn) = rusqlite::Connection::open(path) else {
        return Vec::new();
    };
    let Ok(mut query) = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ) else {
        return Vec::new();
    };
    query
        .query_map((), |row| row.get(0))
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
PRAGMA user_version = 4;",
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
        Some(18),
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
    let found = second.find_device_by_wire(&device.wire).await;
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
    let version = guard.query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0));
    assert!(
        matches!(version, Ok(18)),
        "reopened database must record schema version 17"
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
        ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcome,
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
        task_agent: None,
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
async fn migration_v4_reopen_keeps_credential_approval_rows() {
    let dir = tempfile::tempdir();
    let dir = dir.unwrap();
    let path = dir.path().join("store.db");
    let opened = Store::open(&path).await;
    let first = opened.unwrap();
    let pending_requested = first
        .request_registration_with_intent(
            String::from("acme"),
            String::from("pending"),
            registration_fingerprint("reg-v4-pending", "acme", "pending"),
        )
        .await;
    assert_eq!(
        pending_requested,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "pending request must record"
    );
    approve_pair(&first, "acme", "usable", "sk-usable", "reg-v4-usable").await;
    drop(first);
    let reopened = Store::open(&path).await;
    let second = reopened.unwrap();
    let listed = CredentialApprovalRepository::list_pending(&second).await;
    let items = listed.unwrap();
    assert_eq!(items.len(), 1, "pending row must survive reopen");
    assert_eq!(items[0].provider.as_str(), "acme");
    assert_eq!(items[0].label.as_str(), "pending");
    let refs = second.list_refs().await.unwrap();
    assert!(
        refs.contains(&CredentialRef::new("acme", "usable").expect("valid test fixture")),
        "usable ref must survive reopen"
    );
    assert!(
        !refs.contains(&CredentialRef::new("acme", "pending").expect("valid test fixture")),
        "pending-only pair must stay unapproved after reopen"
    );
    let rerequest = second
        .request_registration_with_intent(
            String::from("acme"),
            String::from("pending"),
            registration_fingerprint("reg-v4-pending-2", "acme", "pending"),
        )
        .await;
    assert_eq!(
        rerequest,
        Ok(RegistrationApply::Decided(
            RegistrationState::HeldByOperation
        )),
        "pending state must survive reopen"
    );
    let reapprove = second
        .approve_credential_with_sweep("acme", "usable", "sk-usable")
        .expect("re-approval must commit");
    assert!(reapprove, "usable state must survive reopen");
    let guard = match second.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let version = guard.query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0));
    assert!(
        matches!(version, Ok(18)),
        "reopened database must record schema version 17"
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

    let memories = store
        .list_current_memories(companion, None, 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1, "the committed memory must be current");
    let current = &memories[0];
    assert_eq!(current.content, "owner likes jasmine tea");
    assert_eq!(current.scope, LearningScope::companion(companion));
    assert_eq!(current.importance, Importance::clamped(4));
    assert_eq!(current.temporal, TemporalMeaning::Enduring);

    let revisions = store
        .list_memory_revisions(memory, None, 100)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].change, ChangeKind::Initial);
    assert_eq!(revisions[0].summary, Some(evidence.id));
    assert_eq!(revisions[0].content, "owner likes jasmine tea");

    let stored_evidence = store
        .load_summaries(&[evidence.id])
        .await
        .unwrap()
        .into_iter()
        .next();
    let Some(stored_evidence) = stored_evidence else {
        panic!("the evidence summary must be stored");
    };
    assert_eq!(stored_evidence.content, "owner likes jasmine tea");
    assert_eq!(stored_evidence.source.kind, ExperienceSourceKind::Dialogue);
    assert_eq!(stored_evidence.scope, LearningScope::companion(companion));
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

/// A v11 database migrates through the later chains into the current
/// version in one open:
/// history rows gain the canonical UTC projection and pre-index memories
/// gain token rows, so neither the History window nor lexical recall goes
/// dark.
#[tokio::test]
async fn migration_v11_applies_v12_through_v17() {
    let dir = tempfile::tempdir().expect("a temp dir must open");
    let path = dir.path().join("app.db");
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let message = RawId::new();
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, presence_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        crate::codec::encode_id(message),
                        crate::codec::encode_id(companion),
                        crate::codec::encode_id(RawId::new()),
                        "owner",
                        "legacy row",
                        "en",
                        "2026-09-08T12:00:00+09:00",
                        0_i64,
                    ],
                )
                .expect("the legacy history row must insert");
        }
        let outcome = store
            .commit_memory_change(commit(
                None,
                learning_change(
                    companion,
                    MemoryTarget::New { id: memory },
                    "The owner likes jasmine tea.",
                    ChangeKind::Initial,
                    false,
                ),
            ))
            .await;
        assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
        // Rewind to the real v11 shape: no UTC projection, no token index.
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(
                    "DROP INDEX IF EXISTS idx_history_message_companion_at;
                     ALTER TABLE history_message DROP COLUMN at_utc;
                     DROP TABLE IF EXISTS learning_memory_term;
                     DROP INDEX IF EXISTS idx_learning_memory_term_memory;
                     DROP INDEX IF EXISTS idx_learning_memory_recall_newest;
                     DROP INDEX IF EXISTS idx_learning_memory_recall_importance;
                     PRAGMA user_version = 11;",
                )
                .expect("the version-11 rewind must apply");
        }
    }
    let store = Store::open(&path).await.expect("migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v11 database must converge on v17"
    );
    let projection = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .query_row(
                "SELECT at_utc FROM history_message WHERE message_id = ?1",
                rusqlite::params![crate::codec::encode_id(message)],
                |row| row.get::<_, String>(0),
            )
            .expect("the backfilled projection must read")
    };
    assert_eq!(
        projection, "2026-09-08T03:00:00.000000000Z",
        "v13 backfills the canonical UTC rendering, keeping the offset display"
    );
    let candidates = store
        .recall_candidates(companion, &[String::from("jasmine")], 3)
        .await
        .unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.content.contains("jasmine tea")),
        "v14 backfills the token index for the pre-index memory"
    );
}

/// A v13 database with the History projection applied migrates through v14
/// with that projection intact and the token schema added.
#[tokio::test]
async fn migration_v13_preserves_at_utc_and_adds_the_token_index() {
    let dir = tempfile::tempdir().expect("a temp dir must open");
    let path = dir.path().join("app.db");
    let companion = RawId::new();
    let memory = MemoryId::generate();
    let message = RawId::new();
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, at_utc, presence_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        crate::codec::encode_id(message),
                        crate::codec::encode_id(companion),
                        crate::codec::encode_id(RawId::new()),
                        "owner",
                        "projected row",
                        "en",
                        "2026-09-08T12:00:00+09:00",
                        "2026-09-08T03:00:00.000000000Z",
                        0_i64,
                    ],
                )
                .expect("the projected history row must insert");
        }
        let outcome = store
            .commit_memory_change(commit(
                None,
                learning_change(
                    companion,
                    MemoryTarget::New { id: memory },
                    "The owner likes jasmine tea.",
                    ChangeKind::Initial,
                    false,
                ),
            ))
            .await;
        assert!(matches!(outcome, Ok(MemoryChangeOutcome::Committed { .. })));
        // Rewind to the v13 shape the History change left behind: the UTC
        // projection stays applied, only the token index is missing.
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(
                    "DROP TABLE IF EXISTS learning_memory_term;
                     DROP INDEX IF EXISTS idx_learning_memory_term_memory;
                     DROP INDEX IF EXISTS idx_learning_memory_recall_newest;
                     DROP INDEX IF EXISTS idx_learning_memory_recall_importance;
                     PRAGMA user_version = 13;",
                )
                .expect("the version-13 rewind must apply");
        }
    }
    let store = Store::open(&path).await.expect("migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v13 database must converge on v17"
    );
    let (projection, columns) = {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let projection: String = guard
            .query_row(
                "SELECT at_utc FROM history_message WHERE message_id = ?1",
                rusqlite::params![crate::codec::encode_id(message)],
                |row| row.get(0),
            )
            .expect("the v13 projection must survive");
        let columns: Vec<String> = guard
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
            .expect("the index catalog must read")
            .query_map((), |row| row.get(0))
            .expect("index rows must read")
            .collect::<Result<Vec<_>, _>>()
            .expect("index names must decode");
        (projection, columns)
    };
    assert_eq!(
        projection, "2026-09-08T03:00:00.000000000Z",
        "v14 must not disturb the v13 projection"
    );
    for index in [
        "idx_learning_memory_term_memory",
        "idx_learning_memory_recall_newest",
        "idx_learning_memory_recall_importance",
    ] {
        assert!(
            columns.iter().any(|name| name == index),
            "v14 must create {index}, got {columns:?}"
        );
    }
    let candidates = store
        .recall_candidates(companion, &[String::from("jasmine")], 3)
        .await
        .unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.content.contains("jasmine tea")),
        "the v13 memory must be lexically reachable after v14"
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
                base: String::from("consent-none"),
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
    // The rolled-back registration left no pending row, so the pair
    // registers again before the retry can approve it.
    let retried = store
        .request_registration_with_intent(
            String::from("acme"),
            String::from("main"),
            RegistrationFingerprint {
                intent_id: String::from("sweep-atomic-2"),
                kind: String::from("register"),
                target: String::from("credential:acme:main"),
                base: String::from("consent-none"),
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
async fn learning_migration_adds_tables_to_a_v8_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        // A realistic v8 database carries the v7 attempt table the v10
        // migration alters; only the v9 learning group is still missing.
        conn.execute_batch(
            "PRAGMA user_version = 8;
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
    let opened = store.list_current_memories(RawId::new(), None, 10).await;
    assert_eq!(opened, Ok(Vec::new()), "migrated schema answers reads");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
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

/// A v11 database gains the owner-recency index on reopen and converges on
/// the current version, so upgraded stores enforce reply-adoption recency
/// from the index.
#[tokio::test]
async fn migration_v11_adds_the_owner_recency_index() {
    let dir = tempfile::tempdir().expect("a temp dir must open");
    let path = dir.path().join("store.db");
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch(
                "DROP INDEX IF EXISTS idx_history_message_companion_role;
                 DROP INDEX IF EXISTS idx_history_message_companion_at;
                 ALTER TABLE history_message DROP COLUMN at_utc;
                 PRAGMA user_version = 11;",
            )
            .expect("the version-11 rewind must apply");
    }
    let _store = Store::open(&path).await.expect("migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v11 database must converge on v17"
    );
    let conn = rusqlite::Connection::open(&path).expect("the migrated store must open");
    let index: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_history_message_companion_role'",
            (),
            |row| row.get(0),
        )
        .optional()
        .expect("the index catalog must read");
    assert!(index.is_some(), "v12 must create the companion-role index");
}

#[tokio::test]
async fn migration_v10_moves_stage2_consent_to_dialogue_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 9;
             CREATE TABLE consent_record (id TEXT PRIMARY KEY, rev INTEGER NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, credential_id TEXT NOT NULL);
             INSERT INTO consent_record VALUES ('consent-1', 1, 'openai', 'gpt-x', 'openai:main');
             CREATE TABLE inference_attempt (
               ticket TEXT PRIMARY KEY,
               consent_id TEXT NOT NULL,
               consent_rev INTEGER NOT NULL,
               provider TEXT NOT NULL,
               model TEXT NOT NULL,
               started_at TEXT NOT NULL
             );
             -- A real v9 database also carries the v9 learning group the v12
             -- token backfill reads; mirrors MIGRATION_V9.
             CREATE TABLE learning_summary (
               summary_id TEXT PRIMARY KEY,
               companion_id TEXT NOT NULL,
               content TEXT NOT NULL,
               source_kind TEXT NOT NULL,
               source_start TEXT NOT NULL,
               source_end TEXT NOT NULL,
               formed_at TEXT NOT NULL
             );
             CREATE TABLE learning_memory (
               memory_id TEXT PRIMARY KEY,
               companion_id TEXT NOT NULL,
               revision INTEGER NOT NULL,
               content TEXT NOT NULL,
               importance INTEGER NOT NULL,
               temporal TEXT NOT NULL,
               recall_suppressed INTEGER NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE INDEX idx_learning_memory_companion ON learning_memory (companion_id);
             CREATE TABLE learning_memory_revision (
               memory_id TEXT NOT NULL,
               revision INTEGER NOT NULL,
               companion_id TEXT NOT NULL,
               content TEXT NOT NULL,
               importance INTEGER NOT NULL,
               temporal TEXT NOT NULL,
               recall_suppressed INTEGER NOT NULL,
               change_kind TEXT NOT NULL,
               summary_id TEXT,
               at TEXT NOT NULL,
               PRIMARY KEY (memory_id, revision)
             );",
        )
        .unwrap();
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(read_schema_version(&path), Some(18));
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
    approve_pair(&approver, "acme", "main", "sk-new", "reg-race-history").await;
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
    let timeline = writer
        .load_timeline(companion, None, None, 10)
        .await
        .unwrap();
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
    approve_pair(&approver, "acme", "main", "sk-new", "reg-race-learning").await;
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
    assert!(
        writer
            .list_current_memories(companion, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(writer.load_summaries(&[evidence.id]).await, Ok(Vec::new()));
    assert!(
        writer
            .list_memory_revisions(memory, None, 100)
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
    let seeded = save_consent(
        &sender,
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
    let premise = sender.current_set_revision().await.unwrap();
    approve_pair(&approver, "openai", "main", "sk-new", "reg-race-send").await;
    let claim = sender
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: premise,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
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
    approve_pair(&approver, "acme", "main", "sk-a", "reg-reapprove-history").await;
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
    let timeline = writer
        .load_timeline(companion, None, None, 10)
        .await
        .unwrap();
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
    approve_pair(&approver, "acme", "main", "sk-a", "reg-reapprove-learning").await;
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
    assert!(
        writer
            .list_current_memories(companion, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(writer.load_summaries(&[evidence.id]).await, Ok(Vec::new()));
    assert!(
        writer
            .list_memory_revisions(memory, None, 100)
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
    let seeded = save_consent(
        &sender,
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
    approve_pair(&approver, "openai", "main", "sk-a", "reg-reapprove-send").await;
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
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: premise,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
        })
        .await;
    assert_eq!(stale, Ok(AttemptBeginOutcome::Stale));

    let fresh = sender
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: updated,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
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

#[tokio::test]
async fn task_creation_commits_the_au2_unit_and_loads() {
    let store = open_memory().await.unwrap();
    let workspace = task_workspace("/srv/workspace/ene", Some("/srv/workspace/ene/out"));
    let premise = task_premise(Some(workspace.clone()));
    let created = store
        .create_task(premise.clone())
        .await
        .expect("creation must commit");
    assert_eq!(created.task, premise.task);
    assert_eq!(created.revision, TaskRevision::initial());

    let record = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the created task must load");
    assert_eq!(record.task.reference, created);
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: premise.task,
            adopted_revision: TaskRevision::initial(),
        }
    );
    assert_eq!(record.task.assignee, premise.assignee);
    assert_eq!(record.revision.reference, created);
    assert_eq!(record.revision.purpose, record.task.purpose);
    assert_eq!(record.revision.purpose_text, premise.purpose);
    assert_eq!(record.revision.assignee, premise.assignee);
    assert_eq!(
        record.context.len(),
        1,
        "AU2 records exactly the adopted purpose entry"
    );
    let entry = &record.context[0];
    assert_eq!(entry.entry, premise.entry);
    assert_eq!(entry.reference, created);
    assert_eq!(
        entry.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(entry.origin, premise.origin);
    assert_eq!(
        entry.acquired_at.to_rfc3339(),
        premise.acquired_at.to_rfc3339(),
        "the stored acquisition time keeps its creation rendering"
    );
    let association = record
        .workspace
        .expect("the confirmed association must load");
    assert_eq!(association.assoc, workspace.assoc);
    assert_eq!(association.task, premise.task);
    assert_eq!(association.folder, workspace.need.folder);
    assert_eq!(association.save_target, workspace.need.save_target);

    assert_eq!(
        store.load_task(TaskId::generate()).await,
        Ok(None),
        "a missing task is never fabricated"
    );
}

#[tokio::test]
async fn task_creation_without_workspace_omits_the_association() {
    let store = open_memory().await.unwrap();
    let premise = task_premise(None);
    let created = store.create_task(premise).await.unwrap();
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.workspace, None);
    assert_eq!(
        task_table_count(&store, "workspace_assoc"),
        0,
        "no confirmed association writes no row"
    );
}

#[tokio::test]
async fn task_au2_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let workspace = task_workspace("/srv/workspace/ene", None);
    let premise = task_premise(Some(workspace.clone()));
    let before = {
        let store = Store::open(&path).await.unwrap();
        let created = store.create_task(premise.clone()).await.unwrap();
        let record = store.load_task(created.task).await.unwrap().unwrap();
        drop(store);
        record
    };

    let reopened = Store::open(&path).await.unwrap();
    let after = reopened
        .load_task(premise.task)
        .await
        .unwrap()
        .expect("the task must survive reopen");
    assert_eq!(after, before, "the committed AU2 unit survives reopen");
    assert_eq!(
        after.task.reference,
        TaskRef {
            task: premise.task,
            revision: TaskRevision::initial(),
        },
        "the created revision survives reopen"
    );
    assert_eq!(after.revision.purpose_text, premise.purpose);
    assert_eq!(
        after.context.len(),
        1,
        "the adopted purpose entry must survive reopen"
    );
    assert_eq!(after.context[0].origin, premise.origin);
    let association = after
        .workspace
        .expect("the association must survive reopen");
    assert_eq!(association.folder, workspace.need.folder);
    assert_eq!(reopened.load_task(TaskId::generate()).await, Ok(None));
}

#[tokio::test]
async fn task_creation_is_atomic_across_every_au2_insert() {
    let store = open_memory().await.unwrap();
    for table in [
        "task",
        "task_revision",
        "task_context_entry",
        "workspace_assoc",
    ] {
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(&format!(
                    "CREATE TRIGGER au2_abort BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'injected fault'); END;"
                ))
                .expect("the fault trigger must install");
        }
        let premise = task_premise(Some(task_workspace("/srv/workspace/ene", None)));
        let failed = store.create_task(premise.clone()).await;
        assert!(
            matches!(failed, Err(TaskTechnicalError::StorageUnavailable { .. })),
            "a fault on {table} must surface a storage failure, got {failed:?}"
        );
        assert_eq!(
            store.load_task(premise.task).await,
            Ok(None),
            "a failed creation is not visible as a Task"
        );
        for probe in [
            "task",
            "task_revision",
            "task_context_entry",
            "workspace_assoc",
        ] {
            assert_eq!(
                task_table_count(&store, probe),
                0,
                "a fault on {table} must leave no {probe} row"
            );
        }
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch("DROP TRIGGER au2_abort;")
                .expect("the fault trigger must drop");
        }
    }

    let premise = task_premise(Some(task_workspace("/srv/workspace/ene", None)));
    let created = store
        .create_task(premise.clone())
        .await
        .expect("creation succeeds after the faults clear");
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.context.len(), 1);
    assert_eq!(record.task.purpose.task, premise.task);
}

#[tokio::test]
async fn task_load_rejects_a_partial_au2_unit() {
    let store = open_memory().await.unwrap();

    // A current row without its revision snapshot is corruption, not an empty Task.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "DELETE FROM task_revision WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw())],
            )
            .expect("the snapshot probe must delete");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "a current row without its revision snapshot is a technical error"
    );

    // A current revision without its adopted context entry is corruption too.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "DELETE FROM task_context_entry WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw())],
            )
            .expect("the context probe must delete");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "a current revision without its context entry is a technical error"
    );
}

#[tokio::test]
async fn task_load_rejects_an_inconsistent_au2_unit() {
    let store = open_memory().await.unwrap();
    let mismatch = i64::try_from(TaskRevision::initial().as_u64() + 1)
        .expect("the probe revision fits an integer");

    // D1 current purpose and the D2 snapshot adopted revision must agree.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_revision SET purpose_adopted_revision = ?2 WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw()), mismatch],
            )
            .expect("the purpose probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "a current/snapshot purpose mismatch is a technical error"
    );

    // The adopted-purpose entry must record the current purpose.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET purpose_adopted_revision = ?2 WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw()), mismatch],
            )
            .expect("the context probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an adopted-purpose/current purpose mismatch is a technical error"
    );

    // D1 current assignee and the D2 snapshot assignee must agree.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_revision SET assignee = ?2 WHERE task_id = ?1",
                params![
                    crate::codec::encode_id(premise.task.as_raw()),
                    crate::codec::encode_id(RawId::new()),
                ],
            )
            .expect("the assignee probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "a current/snapshot assignee mismatch is a technical error"
    );

    // An unknown stored origin kind is an unreadable row.
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET origin_kind = 'unknown' WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw())],
            )
            .expect("the origin probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an unknown stored origin kind is a technical error"
    );
}

#[tokio::test]
async fn task_origin_kinds_round_trip() {
    let store = open_memory().await.unwrap();
    for kind in [
        TaskContextOriginKind::OwnerConversation,
        TaskContextOriginKind::Spontaneous,
        TaskContextOriginKind::ScheduleOccurrence,
    ] {
        let mut premise = task_premise(None);
        premise.origin.kind = kind;
        let created = store.create_task(premise).await.unwrap();
        let record = store.load_task(created.task).await.unwrap().unwrap();
        assert_eq!(
            record.context[0].origin.kind, kind,
            "the stored origin kind round-trips"
        );
    }
}

#[tokio::test]
async fn task_migration_adds_tables_to_a_v14_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    {
        let store = Store::open(&path).await.unwrap();
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch(
                "DROP TABLE IF EXISTS workspace_assoc;
                 DROP TABLE IF EXISTS task_context_entry;
                 DROP TABLE IF EXISTS task_revision;
                 DROP TABLE IF EXISTS task;
                 PRAGMA user_version = 14;",
            )
            .expect("the version-14 rewind must apply");
    }

    let reopened = Store::open(&path).await.expect("migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v14 database must converge on v17"
    );
    assert!(!table_columns(&path, "task").is_empty(), "task is created");
    assert!(
        !table_columns(&path, "task_revision").is_empty(),
        "task_revision is created"
    );
    assert!(
        !table_columns(&path, "task_context_entry").is_empty(),
        "task_context_entry is created"
    );
    assert!(
        !table_columns(&path, "workspace_assoc").is_empty(),
        "workspace_assoc is created"
    );
    let conn = rusqlite::Connection::open(&path).expect("the migrated store must open");
    for index in ["idx_task_context_entry_task", "idx_workspace_assoc_task"] {
        let found: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?1",
                rusqlite::params![index],
                |row| row.get(0),
            )
            .optional()
            .expect("the index catalog must read");
        assert!(found.is_some(), "{index} is created");
    }
    drop(conn);
    let premise = task_premise(None);
    let created = reopened
        .create_task(premise.clone())
        .await
        .expect("an upgraded database creates");
    assert_eq!(
        reopened
            .load_task(created.task)
            .await
            .unwrap()
            .map(|record| record.task.reference),
        Some(created),
        "an upgraded database answers create and load"
    );
}

// --- Task: V16 item-kind migration ---

/// A realistic v15 database: the Task group is written through the production
/// path, then `task_context_entry` is rewound to its pre-V16 shape (no item
/// kind, non-null purpose payload) and `user_version` is lowered to 15. The
/// unit holds a purpose entry carried from revision 1 to revision 2.
struct V15TaskSeed {
    task: TaskId,
    assignee: AssigneeRef,
    creation_entry: TaskContextEntryId,
    carried_entry: TaskContextEntryId,
    origin: TaskContextOrigin,
    acquired_at: WallClockWithTz,
}

async fn seed_v15_task_database(path: &std::path::Path) -> V15TaskSeed {
    let creation = task_premise(Some(task_workspace(
        "/srv/workspace/v16",
        Some("/srv/workspace/v16/out"),
    )));
    let carried_entry = TaskContextEntryId::generate();
    {
        let store = Store::open(path).await.expect("the seed store must open");
        let created = store
            .create_task(creation.clone())
            .await
            .expect("the seed creation must commit");
        let outcome = store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose: None,
                adopted_purpose_entry: carried_entry,
                adopted_instruction: None,
            })
            .await
            .expect("the seed forward must commit");
        assert!(
            matches!(outcome, TaskCommitOutcome::CommittedAs(_)),
            "the seed forward must commit, got {outcome:?}"
        );
    }
    {
        let conn = rusqlite::Connection::open(path).expect("the rewind must open");
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_task_context_entry_task;
             CREATE TABLE task_context_entry_v15 (
               entry_id TEXT PRIMARY KEY,
               task_id TEXT NOT NULL,
               revision INTEGER NOT NULL,
               purpose_adopted_revision INTEGER NOT NULL,
               origin_kind TEXT NOT NULL,
               origin_source TEXT NOT NULL,
               acquired_at TEXT NOT NULL
             );
             INSERT INTO task_context_entry_v15 (entry_id, task_id, revision, purpose_adopted_revision, origin_kind, origin_source, acquired_at)
               SELECT entry_id, task_id, revision, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry;
             DROP TABLE task_context_entry;
             ALTER TABLE task_context_entry_v15 RENAME TO task_context_entry;
             CREATE INDEX idx_task_context_entry_task ON task_context_entry (task_id, revision);
             PRAGMA user_version = 15;",
        )
        .expect("the version-15 rewind must apply");
    }
    V15TaskSeed {
        task: creation.task,
        assignee: creation.assignee,
        creation_entry: creation.entry,
        carried_entry,
        origin: creation.origin,
        acquired_at: creation.acquired_at,
    }
}

/// Every raw `task_context_entry` row in its v15 column order, by rowid:
/// `(entry, task, revision, adopted, origin, source, at)`.
type V15ContextRow = (String, String, i64, i64, String, String, String);

fn v15_context_rows(path: &std::path::Path) -> Vec<V15ContextRow> {
    let Ok(conn) = rusqlite::Connection::open(path) else {
        return Vec::new();
    };
    let Ok(mut query) = conn.prepare(
        "SELECT entry_id, task_id, revision, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry ORDER BY rowid",
    ) else {
        return Vec::new();
    };
    query
        .query_map((), |row| {
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
        .map(|rows| rows.filter_map(std::result::Result::ok).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn task_migration_v15_context_rows_backfill_as_adopted_purpose() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let seed = seed_v15_task_database(&path).await;
    assert_eq!(
        read_schema_version(&path),
        Some(15),
        "the seed must sit at the v15 boundary"
    );

    let reopened = Store::open(&path)
        .await
        .expect("the V16 migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v15 database must converge on v17"
    );

    // A fresh store in its own directory builds the schema and every
    // migration from zero, so its table list is the current permanent
    // shape. Comparing against it proves the V16 rebuild left no staging
    // table behind without a hand-maintained list to edit per migration.
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh_path = fresh_dir.path().join("store.db");
    let _fresh = Store::open(&fresh_path)
        .await
        .expect("a fresh store must open");
    assert_eq!(
        schema_table_names(&path),
        schema_table_names(&fresh_path),
        "the rebuild must leave no staging table behind"
    );

    // The carried purpose row (adopted revision 1 recorded at revision 2)
    // keeps its own payload, provenance, and acquisition time.
    let rows = task_context_rows(&reopened, seed.task);
    assert_eq!(rows.len(), 2, "both seeded purpose rows stay");
    assert_eq!(
        rows[0],
        (
            1,
            Some(1),
            crate::codec::encode_id(seed.creation_entry.as_raw()),
            String::from("adopted_purpose"),
            String::from("owner_conversation"),
            crate::codec::encode_id(seed.origin.source),
            seed.acquired_at.to_rfc3339(),
        ),
        "the revision-1 purpose row is backfilled unchanged"
    );
    assert_eq!(
        rows[1],
        (
            2,
            Some(1),
            crate::codec::encode_id(seed.carried_entry.as_raw()),
            String::from("adopted_purpose"),
            String::from("owner_conversation"),
            crate::codec::encode_id(seed.origin.source),
            seed.acquired_at.to_rfc3339(),
        ),
        "the carried purpose row is backfilled as adopted purpose"
    );

    let record = reopened
        .load_task(seed.task)
        .await
        .unwrap()
        .expect("the migrated unit must load");
    assert_eq!(record.task.reference.revision, TaskRevision::from_u64(2));
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: seed.task,
            adopted_revision: TaskRevision::initial(),
        }
    );
    assert_eq!(record.revision.assignee, seed.assignee);
    assert_eq!(
        record.context.len(),
        1,
        "only the current revision's purpose entry is current context"
    );
    let entry = &record.context[0];
    assert_eq!(entry.entry, seed.carried_entry);
    assert_eq!(entry.reference, record.task.reference);
    assert_eq!(
        entry.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(entry.origin, seed.origin);
    assert_eq!(
        entry.acquired_at.to_rfc3339(),
        seed.acquired_at.to_rfc3339()
    );
    let workspace = record
        .workspace
        .expect("the upgraded unit keeps its association");
    assert_eq!(workspace.task, seed.task);
    assert_eq!(workspace.folder.path, "/srv/workspace/v16");
    assert_eq!(
        workspace
            .save_target
            .expect("the save target survives")
            .path,
        "/srv/workspace/v16/out"
    );
}

#[tokio::test]
async fn migration_v16_fault_rolls_back_and_reopen_converges() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let seed = seed_v15_task_database(&path).await;
    let before = v15_context_rows(&path);
    assert_eq!(before.len(), 2, "the seed carries two purpose rows");

    // Hold a SHARED read transaction so V16's statements execute but its
    // COMMIT cannot take the EXCLUSIVE lock. The migration is one
    // transaction, so the failure must roll back every DDL and DML
    // statement and leave the v15 shape and rows exactly as they were.
    // This faults the migration independently of the rebuild's staging
    // table name, which is not part of the contract.
    //
    // The fault relies on the store's connection carrying rusqlite's
    // implicit ~5 s busy timeout: COMMIT retries against the held SHARED
    // lock, then fails with SQLITE_BUSY when the timeout expires. The
    // outcome is identical with the timeout disabled (COMMIT fails with
    // SQLITE_BUSY immediately), and the lock mechanics are deterministic
    // on Linux and Windows, so the assertions never depend on the retry
    // duration, only on the failure and rollback.
    let blocker = rusqlite::Connection::open(&path).expect("the blocker must open");
    blocker
        .execute_batch("BEGIN")
        .expect("the SHARED transaction must open");
    let held: i64 = blocker
        .query_row("SELECT COUNT(*) FROM task_context_entry", (), |row| {
            row.get(0)
        })
        .expect("the read must take the SHARED lock");
    assert_eq!(held, 2);

    let failed = Store::open(&path).await;
    assert!(failed.is_err(), "a faulted V16 migration must fail open");
    assert_eq!(
        read_schema_version(&path),
        Some(15),
        "a failed migration must not advance the version"
    );
    assert!(
        !table_columns(&path, "task_context_entry").contains(&String::from("item_kind")),
        "the failed rebuild must roll back its DDL"
    );
    assert_eq!(
        v15_context_rows(&path),
        before,
        "the failed rebuild must keep every v15 row"
    );

    blocker
        .execute_batch("ROLLBACK")
        .expect("the read transaction must close");
    drop(blocker);

    let reopened = Store::open(&path)
        .await
        .expect("open must recover after the fault clears");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "the retried migration must converge"
    );
    assert!(
        table_columns(&path, "task_context_entry").contains(&String::from("item_kind")),
        "the retried rebuild must apply the kind discriminator"
    );
    let rows = task_context_rows(&reopened, seed.task);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|row| row.3 == "adopted_purpose"),
        "the retried backfill must mark both rows as adopted purpose"
    );
    let record = reopened
        .load_task(seed.task)
        .await
        .unwrap()
        .expect("the recovered unit must load");
    assert_eq!(record.task.reference.revision, TaskRevision::from_u64(2));
    assert_eq!(record.context.len(), 1);
    assert_eq!(record.context[0].entry, seed.carried_entry);
}

// --- Task: AU4 steering / CAS forward ---

fn task_purpose_adoption(text: &str) -> TaskPurposeAdoptionPremise {
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

/// One adopted-instruction premise sourced from the given utterance record.
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

/// The D1 current row as stored: `(revision, adopted, text, assignee)`.
fn task_current_row(store: &Store, task: TaskId) -> (i64, i64, String, String) {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(
            "SELECT revision, purpose_adopted_revision, purpose_text, assignee FROM task WHERE task_id = ?1",
            params![crate::codec::encode_id(task.as_raw())],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the current row must read")
}

/// The stored D2 assignee per revision, by revision.
fn task_revision_assignees(store: &Store, task: TaskId) -> Vec<String> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut statement = guard
        .prepare("SELECT assignee FROM task_revision WHERE task_id = ?1 ORDER BY revision")
        .expect("the assignee probe must prepare");
    statement
        .query_map(params![crate::codec::encode_id(task.as_raw())], |row| {
            row.get(0)
        })
        .expect("the assignee probe must query")
        .collect::<Result<Vec<_>, _>>()
        .expect("the assignee probe must read")
}

/// Every stored revision snapshot: `(revision, adopted, text)` by revision.
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

/// One stored context entry:
/// `(revision, adopted, entry, kind, origin, source, at)`. The adopted
/// revision is `None` for entries whose kind carries no purpose payload.
type ContextRow = (i64, Option<i64>, String, String, String, String, String);

/// Every stored context entry by row order.
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

#[tokio::test]
async fn task_steering_with_a_new_purpose_adopts_it_at_the_new_revision() {
    let store = open_memory().await.unwrap();
    let workspace = task_workspace("/srv/workspace/au4", Some("/srv/workspace/au4/out"));
    let creation = task_premise(Some(workspace.clone()));
    let created = store.create_task(creation.clone()).await.unwrap();
    let adoption = task_purpose_adoption("revised AU4 purpose");
    let adopted_entry = TaskContextEntryId::generate();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(adoption.clone()),
            adopted_purpose_entry: adopted_entry,
            adopted_instruction: None,
        })
        .await
        .expect("the steering commit must run");
    let TaskCommitOutcome::CommittedAs(committed) = outcome else {
        panic!("expected CommittedAs, got {outcome:?}");
    };
    assert_eq!(committed.task, created.task);
    assert_eq!(committed.revision, TaskRevision::from_u64(2));

    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.reference, committed, "D1 advanced by one");
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: created.task,
            adopted_revision: committed.revision,
        },
        "a purpose change adopts at the new revision"
    );
    assert_eq!(record.revision.reference, committed);
    assert_eq!(record.revision.purpose, record.task.purpose);
    assert_eq!(record.revision.purpose_text, adoption.purpose);
    assert_eq!(
        record.revision.assignee, creation.assignee,
        "steering carries the assignee unchanged"
    );
    assert_eq!(
        record.context.len(),
        1,
        "the new revision records exactly its adopted purpose entry"
    );
    let entry = &record.context[0];
    assert_eq!(
        entry.entry, adopted_entry,
        "the repository persists the caller-minted entry identity"
    );
    assert_eq!(entry.reference, committed);
    assert_eq!(
        entry.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(entry.origin, adoption.origin);
    assert_eq!(
        entry.acquired_at.to_rfc3339(),
        adoption.acquired_at.to_rfc3339()
    );
    let association = record
        .workspace
        .as_ref()
        .expect("steering must keep the confirmed workspace association");
    assert_eq!(association.assoc, workspace.assoc);
    assert_eq!(association.task, created.task);
    assert_eq!(association.folder, workspace.need.folder);
    assert_eq!(association.save_target, workspace.need.save_target);

    let current = task_current_row(&store, created.task);
    assert_eq!(current.0, 2);
    assert_eq!(current.1, 2);
    assert_eq!(current.2, adoption.purpose.text);
    assert_eq!(
        current.3,
        crate::codec::encode_id(creation.assignee.companion),
        "the D1 assignee projection is untouched"
    );
    assert_eq!(
        task_revision_rows(&store, created.task),
        vec![
            (1, 1, creation.purpose.text.clone()),
            (2, 2, adoption.purpose.text.clone()),
        ],
        "the old snapshot is retained and the new one adopts the new purpose"
    );
    let contexts = task_context_rows(&store, created.task);
    assert_eq!(contexts.len(), 2, "both revisions keep their context entry");
    assert_eq!(contexts[0].0, 1);
    assert_eq!(
        contexts[0].2,
        crate::codec::encode_id(creation.entry.as_raw()),
        "the old entry keeps its row identity"
    );
    assert_eq!(contexts[1].0, 2);
    assert_eq!(
        contexts[1].2,
        crate::codec::encode_id(adopted_entry.as_raw())
    );
}

#[tokio::test]
async fn task_purpose_preserving_forward_carries_the_adopted_purpose_entry() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    let carried_entry = TaskContextEntryId::generate();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: None,
            adopted_purpose_entry: carried_entry,
            adopted_instruction: None,
        })
        .await
        .expect("the steering commit must run");
    let TaskCommitOutcome::CommittedAs(committed) = outcome else {
        panic!("expected CommittedAs, got {outcome:?}");
    };
    assert_eq!(committed.revision, TaskRevision::from_u64(2));

    // The load invariant is the contract here: a carried purpose entry whose
    // adopted revision predates the entry's revision must stay valid.
    let record = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the carried-forward revision must load");
    assert_eq!(record.task.reference, committed);
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: created.task,
            adopted_revision: TaskRevision::initial(),
        },
        "the adopted identity does not move on a purpose-preserving forward"
    );
    assert_eq!(record.revision.reference, committed);
    assert_eq!(record.revision.purpose, record.task.purpose);
    assert_eq!(
        record.revision.purpose_text, creation.purpose,
        "the in-force text is carried, not re-adopted"
    );
    assert_eq!(record.revision.assignee, creation.assignee);
    assert_eq!(record.context.len(), 1);
    let entry = &record.context[0];
    assert_eq!(
        entry.entry, carried_entry,
        "the repository persists the caller-minted carried entry identity"
    );
    assert_eq!(
        entry.reference, committed,
        "the entry belongs to revision 2"
    );
    assert_eq!(
        entry.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(
        entry.origin, creation.origin,
        "provenance is carried, not re-derived"
    );
    assert_eq!(
        entry.acquired_at.to_rfc3339(),
        creation.acquired_at.to_rfc3339()
    );

    let current = task_current_row(&store, created.task);
    assert_eq!(current.0, 2);
    assert_eq!(current.1, 1, "the D1 adopted revision stays at 1");
    assert_eq!(current.2, creation.purpose.text);
    assert_eq!(
        task_revision_rows(&store, created.task),
        vec![
            (1, 1, creation.purpose.text.clone()),
            (2, 1, creation.purpose.text.clone()),
        ],
        "the new snapshot keeps the old adopted revision"
    );
    let contexts = task_context_rows(&store, created.task);
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].1, Some(1));
    assert_eq!(
        contexts[0].2,
        crate::codec::encode_id(creation.entry.as_raw()),
        "the old entry keeps its row identity"
    );
    assert_eq!(contexts[1].0, 2);
    assert_eq!(
        contexts[1].1,
        Some(1),
        "the carried entry keeps the old adopted identity"
    );
    assert_eq!(
        contexts[1].2,
        crate::codec::encode_id(carried_entry.as_raw())
    );
    assert_eq!(contexts[1].4, contexts[0].4, "the origin kind is carried");
    assert_eq!(contexts[1].5, contexts[0].5, "the origin source is carried");
    assert_eq!(
        contexts[1].6, contexts[0].6,
        "the acquisition time is carried verbatim"
    );
}

#[tokio::test]
async fn task_steering_stale_expected_changes_nothing() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let winner = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("winner")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert!(matches!(winner, TaskCommitOutcome::CommittedAs(_)));

    let before_current = task_current_row(&store, created.task);
    let before_revisions = task_revision_rows(&store, created.task);
    let before_contexts = task_context_rows(&store, created.task);
    // The stale loser carries an adopted instruction: a rejected forward must
    // not leave that entry behind either.
    let stale = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("loser")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(task_instruction_adoption(RawId::new())),
        })
        .await
        .unwrap();
    assert_eq!(
        stale,
        TaskCommitOutcome::StaleExpected {
            current: TaskRef {
                task: created.task,
                revision: TaskRevision::from_u64(2),
            },
        },
        "a stale expected revision reports the winner's revision"
    );
    assert_eq!(task_current_row(&store, created.task), before_current);
    assert_eq!(task_revision_rows(&store, created.task), before_revisions);
    assert_eq!(task_context_rows(&store, created.task), before_contexts);
    assert_eq!(
        task_revision_rows(&store, created.task).len(),
        2,
        "no third revision is created"
    );
    assert!(
        !task_context_rows(&store, created.task)
            .iter()
            .any(|row| row.3 == "adopted_instruction"),
        "the stale loser must not leave an adopted-instruction row"
    );
}

#[tokio::test]
async fn concurrent_task_steering_commits_exactly_one_revision() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let left_instruction = task_instruction_adoption(RawId::new());
    let right_instruction = task_instruction_adoption(RawId::new());
    let left = TaskCommitPremise {
        expected: created,
        new_purpose: Some(task_purpose_adoption("concurrent left")),
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction: Some(left_instruction.clone()),
    };
    let right = TaskCommitPremise {
        expected: created,
        new_purpose: Some(task_purpose_adoption("concurrent right")),
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction: Some(right_instruction.clone()),
    };
    let (left, right) = tokio::join!(store.forward_steering(left), store.forward_steering(right));
    let left = left.unwrap();
    let right = right.unwrap();
    let winners = [&left, &right]
        .into_iter()
        .filter(|outcome| matches!(outcome, TaskCommitOutcome::CommittedAs(_)))
        .count();
    let losers = [&left, &right]
        .into_iter()
        .filter(|outcome| {
            matches!(
                outcome,
                TaskCommitOutcome::StaleExpected { current }
                    if current.revision == TaskRevision::from_u64(2)
            )
        })
        .count();
    assert_eq!(
        (winners, losers),
        (1, 1),
        "exactly one winner and one stale loser: {left:?} / {right:?}"
    );
    assert_eq!(
        task_current_row(&store, created.task).0,
        2,
        "the revision advances exactly once"
    );
    assert_eq!(
        task_revision_rows(&store, created.task).len(),
        2,
        "no duplicate revision row exists"
    );
    // Exactly the winner's instruction lands; the stale loser's entry id is
    // nowhere in the durable context.
    let (winner_instruction, loser_instruction) = match (&left, &right) {
        (TaskCommitOutcome::CommittedAs(_), TaskCommitOutcome::StaleExpected { .. }) => {
            (&left_instruction, &right_instruction)
        }
        (TaskCommitOutcome::StaleExpected { .. }, TaskCommitOutcome::CommittedAs(_)) => {
            (&right_instruction, &left_instruction)
        }
        _ => panic!("exactly one winner expected: {left:?} / {right:?}"),
    };
    let contexts = task_context_rows(&store, created.task);
    assert_eq!(
        contexts.len(),
        3,
        "one purpose entry plus exactly one instruction entry"
    );
    assert_eq!(
        contexts[2].3, "adopted_instruction",
        "the winner's instruction is the third row"
    );
    assert_eq!(
        contexts[2].2,
        crate::codec::encode_id(winner_instruction.entry.as_raw()),
        "the winner's caller-minted instruction entry is the one recorded"
    );
    assert!(
        contexts
            .iter()
            .all(|row| row.2 != crate::codec::encode_id(loser_instruction.entry.as_raw())),
        "the stale loser's instruction entry must not exist"
    );
    assert_eq!(
        contexts[2].6,
        winner_instruction.acquired_at.to_rfc3339(),
        "the winner's acquisition time is stored"
    );
    let record = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the winning revision loads");
    assert_eq!(record.context.len(), 2);
    assert_eq!(
        record.context[1].item,
        TaskContextItem::AdoptedInstruction,
        "the loaded context carries the winner's instruction"
    );
    assert_eq!(record.context[1].entry, winner_instruction.entry);
    assert_eq!(record.context[1].origin, winner_instruction.origin);
}

#[tokio::test]
async fn task_steering_faults_roll_back_every_write() {
    let store = open_memory().await.unwrap();
    for (trigger, statement) in [
        (
            "au4_abort_revision",
            "CREATE TRIGGER au4_abort_revision BEFORE INSERT ON task_revision BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        ),
        (
            "au4_abort_purpose_entry",
            "CREATE TRIGGER au4_abort_purpose_entry BEFORE INSERT ON task_context_entry WHEN NEW.item_kind = 'adopted_purpose' BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        ),
        (
            "au4_abort_instruction_entry",
            "CREATE TRIGGER au4_abort_instruction_entry BEFORE INSERT ON task_context_entry WHEN NEW.item_kind = 'adopted_instruction' BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        ),
        (
            "au4_abort_current",
            "CREATE TRIGGER au4_abort_current BEFORE UPDATE ON task BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
        ),
    ] {
        let creation = task_premise(None);
        let created = store.create_task(creation.clone()).await.unwrap();
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(statement)
                .expect("the fault trigger must install");
        }
        // The forward adopts both a new purpose and an instruction, so each
        // insert position is exercised by its own trigger case. The premise
        // is retried unchanged after the fault clears: the failed forward
        // wrote nothing, so the CAS premise is still current.
        let premise = TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("faulted")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(task_instruction_adoption(RawId::new())),
        };
        let failed = store.forward_steering(premise.clone()).await;
        assert!(
            matches!(failed, Err(TaskTechnicalError::StorageUnavailable { .. })),
            "a fault on {trigger} must surface a storage failure, got {failed:?}"
        );
        let current = task_current_row(&store, created.task);
        assert_eq!(current.0, 1, "D1 stays at the expected revision");
        assert_eq!(current.1, 1);
        assert_eq!(current.2, creation.purpose.text);
        assert_eq!(
            current.3,
            crate::codec::encode_id(creation.assignee.companion)
        );
        assert_eq!(task_revision_rows(&store, created.task).len(), 1);
        let after_fault = task_context_rows(&store, created.task);
        assert_eq!(after_fault.len(), 1, "only the AU2 purpose entry remains");
        assert!(
            !after_fault.iter().any(|row| row.3 == "adopted_instruction"),
            "a fault on {trigger} must leave no instruction row"
        );
        let record = store
            .load_task(created.task)
            .await
            .unwrap()
            .expect("the pre-steering state stays loadable");
        assert_eq!(record.task.reference.revision, TaskRevision::initial());
        assert_eq!(record.revision.purpose_text, creation.purpose);
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(&format!("DROP TRIGGER {trigger};"))
                .expect("the fault trigger must drop");
        }
        let committed = store
            .forward_steering(premise)
            .await
            .expect("the same premise succeeds after the fault clears");
        let TaskCommitOutcome::CommittedAs(committed) = committed else {
            panic!("expected CommittedAs, got {committed:?}");
        };
        assert_eq!(committed.revision, TaskRevision::from_u64(2));
        let record = store.load_task(created.task).await.unwrap().unwrap();
        assert_eq!(
            record.context.len(),
            2,
            "the retried premise records the purpose and instruction entries"
        );
        assert_eq!(record.context[1].item, TaskContextItem::AdoptedInstruction);
    }
}

#[tokio::test]
async fn task_steering_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let (task, before) = {
        let store = Store::open(&path).await.unwrap();
        let created = store.create_task(task_premise(None)).await.unwrap();
        let outcome = store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose: Some(task_purpose_adoption("persisted steering")),
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: None,
            })
            .await
            .unwrap();
        assert!(matches!(outcome, TaskCommitOutcome::CommittedAs(_)));
        let record = store.load_task(created.task).await.unwrap().unwrap();
        (created.task, record)
    };

    let reopened = Store::open(&path).await.unwrap();
    let after = reopened
        .load_task(task)
        .await
        .unwrap()
        .expect("the steered revision survives reopen");
    assert_eq!(after, before, "the steered state survives reopen");
    assert_eq!(after.task.reference.revision, TaskRevision::from_u64(2));
    assert_eq!(
        after.revision.purpose.adopted_revision,
        TaskRevision::from_u64(2)
    );
    assert_eq!(
        task_revision_rows(&reopened, task).len(),
        2,
        "old revision history survives reopen"
    );
    assert_eq!(task_context_rows(&reopened, task).len(), 2);
}

#[tokio::test]
async fn task_steering_reports_revision_exhaustion_without_writing() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    let exhausted = i64::MAX;
    let exhausted_entry = TaskContextEntryId::generate();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Move the current unit to the last representable revision as a
        // consistent unit, so the test isolates the durable bound.
        guard
            .execute(
                "UPDATE task SET revision = ?2 WHERE task_id = ?1",
                params![crate::codec::encode_id(created.task.as_raw()), exhausted],
            )
            .expect("the exhaustion probe must update");
        guard
            .execute(
                "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) SELECT task_id, ?2, purpose_adopted_revision, purpose_text, assignee FROM task_revision WHERE task_id = ?1 AND revision = 1",
                params![crate::codec::encode_id(created.task.as_raw()), exhausted],
            )
            .expect("the exhaustion snapshot must insert");
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at) SELECT ?3, task_id, ?2, purpose_adopted_revision, 'adopted_purpose', origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = 1",
                params![
                    crate::codec::encode_id(created.task.as_raw()),
                    exhausted,
                    crate::codec::encode_id(exhausted_entry.as_raw()),
                ],
            )
            .expect("the exhaustion context must insert");
    }
    let before_current = task_current_row(&store, created.task);
    let before_revisions = task_revision_rows(&store, created.task);
    let before_contexts = task_context_rows(&store, created.task);
    assert_eq!(before_revisions.len(), 2);
    assert_eq!(before_contexts.len(), 2);
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: TaskRef {
                task: created.task,
                revision: TaskRevision::from_u64(u64::try_from(exhausted).unwrap()),
            },
            new_purpose: Some(task_purpose_adoption("exhausted")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        TaskCommitOutcome::RevisionExhausted { task: created.task },
        "the first unrepresentable successor is reported, not saturated"
    );
    assert_eq!(task_current_row(&store, created.task), before_current);
    assert_eq!(task_revision_rows(&store, created.task), before_revisions);
    assert_eq!(task_context_rows(&store, created.task), before_contexts);
}

#[tokio::test]
async fn task_steering_reports_a_missing_task_as_a_domain_outcome() {
    let store = open_memory().await.unwrap();
    let missing = TaskId::generate();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: TaskRef {
                task: missing,
                revision: TaskRevision::initial(),
            },
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await;
    assert_eq!(
        outcome,
        Ok(TaskCommitOutcome::MissingTask { task: missing }),
        "a missing Task is a domain outcome, not a storage failure"
    );
}

#[tokio::test]
async fn task_steering_refuses_an_inconsistent_current_unit() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_revision SET assignee = ?2 WHERE task_id = ?1",
                params![
                    crate::codec::encode_id(created.task.as_raw()),
                    crate::codec::encode_id(RawId::new()),
                ],
            )
            .expect("the inconsistency probe must update");
    }
    let before_current = task_current_row(&store, created.task);
    let before_revisions = task_revision_rows(&store, created.task);
    let before_assignees = task_revision_assignees(&store, created.task);
    let before_contexts = task_context_rows(&store, created.task);
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("must not normalize")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await;
    assert!(
        matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "a forward must not normalize a unit the reads reject, got {outcome:?}"
    );
    assert_eq!(task_current_row(&store, created.task), before_current);
    assert_eq!(task_revision_rows(&store, created.task), before_revisions);
    assert_eq!(
        task_revision_assignees(&store, created.task),
        before_assignees
    );
    assert_eq!(task_context_rows(&store, created.task), before_contexts);
}

#[tokio::test]
async fn task_purpose_preserving_forward_fails_closed_on_an_unreadable_predecessor() {
    for probe in [
        "DELETE FROM task_context_entry WHERE task_id = ?1",
        "UPDATE task_context_entry SET origin_kind = 'unknown' WHERE task_id = ?1",
        "UPDATE task_context_entry SET acquired_at = 'not-a-time' WHERE task_id = ?1",
    ] {
        let store = open_memory().await.unwrap();
        let creation = task_premise(None);
        let created = store.create_task(creation.clone()).await.unwrap();
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    probe,
                    params![crate::codec::encode_id(created.task.as_raw())],
                )
                .expect("the predecessor probe must apply");
        }
        let before_current = task_current_row(&store, created.task);
        let before_revisions = task_revision_rows(&store, created.task);
        let before_contexts = task_context_rows(&store, created.task);
        let outcome = store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose: None,
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: None,
            })
            .await;
        assert!(
            matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
            "a carry-forward must fail closed on {probe}, got {outcome:?}"
        );
        assert_eq!(task_current_row(&store, created.task), before_current);
        assert_eq!(task_revision_rows(&store, created.task), before_revisions);
        assert_eq!(task_context_rows(&store, created.task), before_contexts);
    }
}

/// Runs one fail-closed probe in both forward branches: seed a revision-1
/// unit, apply `corrupt` to its creation purpose entry, and require the
/// forward to reject the unit without writing anything.
async fn assert_forward_rejects_corrupted_purpose(label: &str, corrupt: impl Fn(&Store, TaskId)) {
    for adopt in [false, true] {
        let store = open_memory().await.unwrap();
        let created = store.create_task(task_premise(None)).await.unwrap();
        corrupt(&store, created.task);
        let before_current = task_current_row(&store, created.task);
        let before_revisions = task_revision_rows(&store, created.task);
        let before_contexts = task_context_rows(&store, created.task);
        let new_purpose = if adopt {
            Some(task_purpose_adoption("probe purpose"))
        } else {
            None
        };
        let outcome = store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose,
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: None,
            })
            .await;
        assert!(
            matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
            "a forward must fail closed on {label} with new_purpose={adopt}, got {outcome:?}"
        );
        assert_eq!(task_current_row(&store, created.task), before_current);
        assert_eq!(task_revision_rows(&store, created.task), before_revisions);
        assert_eq!(task_context_rows(&store, created.task), before_contexts);
    }
}

#[tokio::test]
async fn task_forward_rejects_a_duplicate_current_purpose_entry() {
    assert_forward_rejects_corrupted_purpose("a duplicate current purpose entry", |store, task| {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at) SELECT ?1, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?2 AND item_kind = 'adopted_purpose'",
                params![
                    crate::codec::encode_id(TaskContextEntryId::generate().as_raw()),
                    crate::codec::encode_id(task.as_raw()),
                ],
            )
            .expect("the duplicate-purpose probe must insert");
    })
    .await;
}

#[tokio::test]
async fn task_forward_rejects_a_missing_current_purpose_entry() {
    assert_forward_rejects_corrupted_purpose("a missing current purpose entry", |store, task| {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "DELETE FROM task_context_entry WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
                params![crate::codec::encode_id(task.as_raw())],
            )
            .expect("the missing-purpose probe must delete");
    })
    .await;
}

#[tokio::test]
async fn task_forward_rejects_a_mismatched_current_purpose_entry() {
    assert_forward_rejects_corrupted_purpose(
        "a current purpose entry that disagrees with the pointer",
        |store, task| {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    "UPDATE task_context_entry SET purpose_adopted_revision = 99 WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
                    params![crate::codec::encode_id(task.as_raw())],
                )
                .expect("the mismatched-purpose probe must update");
        },
    )
    .await;
}

#[tokio::test]
async fn task_forward_rejects_a_payload_free_current_purpose_entry() {
    assert_forward_rejects_corrupted_purpose("a current purpose entry without its payload", |store, task| {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET purpose_adopted_revision = NULL WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
                params![crate::codec::encode_id(task.as_raw())],
            )
            .expect("the payload-free-purpose probe must update");
    })
    .await;
}

#[tokio::test]
async fn task_purpose_preserving_forward_after_a_change_carries_the_in_force_entry() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let adoption = task_purpose_adoption("adopted at revision 2");
    let adopted_entry = TaskContextEntryId::generate();
    let changed = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(adoption.clone()),
            adopted_purpose_entry: adopted_entry,
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(second) = changed else {
        panic!("expected CommittedAs, got {changed:?}");
    };
    let carried_entry = TaskContextEntryId::generate();
    let carried = store
        .forward_steering(TaskCommitPremise {
            expected: second,
            new_purpose: None,
            adopted_purpose_entry: carried_entry,
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(third) = carried else {
        panic!("expected CommittedAs, got {carried:?}");
    };
    assert_eq!(third.revision, TaskRevision::from_u64(3));

    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: created.task,
            adopted_revision: second.revision,
        },
        "the in-force adoption is the revision-2 purpose, not the initial one"
    );
    assert_eq!(record.revision.purpose, record.task.purpose);
    assert_eq!(record.revision.purpose_text, adoption.purpose);
    let entry = &record.context[0];
    assert_eq!(
        entry.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(
        entry.origin, adoption.origin,
        "provenance is carried from the in-force adoption entry"
    );
    assert_eq!(
        entry.acquired_at.to_rfc3339(),
        adoption.acquired_at.to_rfc3339()
    );
    assert_eq!(
        entry.entry, carried_entry,
        "the repository persists the caller-minted carried identity"
    );

    let revisions = task_revision_rows(&store, created.task);
    assert_eq!(revisions.len(), 3);
    assert_eq!(revisions[2], (3, 2, adoption.purpose.text));
    let contexts = task_context_rows(&store, created.task);
    assert_eq!(contexts.len(), 3);
    assert_eq!(contexts[2].0, 3);
    assert_eq!(contexts[2].1, Some(2));
    assert_eq!(
        contexts[2].2,
        crate::codec::encode_id(carried_entry.as_raw())
    );
    assert_eq!(
        contexts[2].5,
        crate::codec::encode_id(adoption.origin.source),
        "the revision-2 adoption source is carried, not revision 1's"
    );
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

#[tokio::test]
async fn task_instruction_only_forward_carries_the_purpose_entry_and_keeps_prior_instructions() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();

    // Revision 2 adopts a new purpose and an instruction.
    let adoption = task_purpose_adoption("in force at revision 2");
    let purpose_entry = TaskContextEntryId::generate();
    let first_instruction = task_instruction_adoption(RawId::new());
    let second = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(adoption.clone()),
            adopted_purpose_entry: purpose_entry,
            adopted_instruction: Some(first_instruction.clone()),
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(second) = second else {
        panic!("expected CommittedAs, got {second:?}");
    };

    // Revision 3 adopts a second instruction only, sourced from the same
    // utterance record as the first. The purpose entry is carried from the
    // in-force revision-2 purpose entry, not re-adopted, and the store must
    // return both instruction entries rather than deduplicating by
    // provenance.
    let carried_entry = TaskContextEntryId::generate();
    let second_instruction = task_instruction_adoption(first_instruction.origin.source);
    let third = store
        .forward_steering(TaskCommitPremise {
            expected: second,
            new_purpose: None,
            adopted_purpose_entry: carried_entry,
            adopted_instruction: Some(second_instruction.clone()),
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(third) = third else {
        panic!("expected CommittedAs, got {third:?}");
    };

    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.reference, third);
    assert_eq!(
        record.task.purpose,
        TaskPurposeRef {
            task: created.task,
            adopted_revision: second.revision,
        },
        "the carried purpose keeps its revision-2 adoption identity"
    );
    assert_eq!(record.revision.purpose_text, adoption.purpose);
    assert_eq!(
        record.context.len(),
        3,
        "the current purpose entry plus both instruction entries"
    );
    assert_eq!(record.context[0].entry, carried_entry);
    assert_eq!(record.context[0].reference, third);
    assert_eq!(
        record.context[0].item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(record.context[0].origin, adoption.origin);
    assert_eq!(record.context[1].entry, first_instruction.entry);
    assert_eq!(record.context[1].item, TaskContextItem::AdoptedInstruction);
    assert_eq!(
        record.context[1].reference, second,
        "the earlier instruction keeps its own adoption reference"
    );
    assert_eq!(record.context[2].entry, second_instruction.entry);
    assert_eq!(record.context[2].item, TaskContextItem::AdoptedInstruction);
    assert_eq!(record.context[2].reference, third);
    assert_eq!(record.context[2].origin, second_instruction.origin);
    assert_eq!(
        record.context[1].origin.source, record.context[2].origin.source,
        "a shared source must not collapse the two instruction entries"
    );

    let rows = task_context_rows(&store, created.task);
    assert_eq!(
        rows.len(),
        5,
        "every revision and adopted entry is retained"
    );
    assert_eq!(rows[4].0, 3);
    assert_eq!(rows[4].1, None);
    assert_eq!(
        rows[4].2,
        crate::codec::encode_id(second_instruction.entry.as_raw())
    );
    assert_eq!(rows[4].3, "adopted_instruction");
}

#[tokio::test]
async fn task_instruction_context_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let (task, before) = {
        let store = Store::open(&path).await.unwrap();
        let created = store.create_task(task_premise(None)).await.unwrap();
        let adoption = task_purpose_adoption("reopened purpose");
        let first_instruction = task_instruction_adoption(RawId::new());
        let second = store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose: Some(adoption),
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: Some(first_instruction),
            })
            .await
            .unwrap();
        let TaskCommitOutcome::CommittedAs(second) = second else {
            panic!("expected CommittedAs, got {second:?}");
        };
        let second_instruction = task_instruction_adoption(RawId::new());
        let third = store
            .forward_steering(TaskCommitPremise {
                expected: second,
                new_purpose: None,
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: Some(second_instruction),
            })
            .await
            .unwrap();
        assert!(
            matches!(third, TaskCommitOutcome::CommittedAs(_)),
            "the second instruction must commit, got {third:?}"
        );
        let record = store.load_task(created.task).await.unwrap().unwrap();
        (created.task, record)
    };

    let reopened = Store::open(&path).await.unwrap();
    let after = reopened
        .load_task(task)
        .await
        .unwrap()
        .expect("the instruction context survives reopen");
    assert_eq!(after, before, "every loaded field survives reopen");
    assert_eq!(after.task.reference.revision, TaskRevision::from_u64(3));
    assert_eq!(after.context.len(), 3);
    assert_eq!(
        after.context[0].item,
        TaskContextItem::AdoptedPurpose(after.task.purpose)
    );
    assert_eq!(after.context[1].item, TaskContextItem::AdoptedInstruction);
    assert_eq!(after.context[2].item, TaskContextItem::AdoptedInstruction);
    assert_eq!(
        task_revision_rows(&reopened, task).len(),
        3,
        "older revision snapshots survive reopen"
    );
    assert_eq!(
        task_context_rows(&reopened, task).len(),
        5,
        "old purpose and instruction rows stay on disk"
    );
}

#[tokio::test]
async fn task_purpose_carry_ignores_instruction_entries() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();

    // Revision 2 carries the purpose and adopts an instruction whose
    // provenance differs from the purpose's.
    let carried_in_force = TaskContextEntryId::generate();
    let instruction = task_instruction_adoption(RawId::new());
    assert_ne!(
        instruction.origin.source, creation.origin.source,
        "the probe needs distinct provenance"
    );
    let second = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: None,
            adopted_purpose_entry: carried_in_force,
            adopted_instruction: Some(instruction.clone()),
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(second) = second else {
        panic!("expected CommittedAs, got {second:?}");
    };

    // Physically move the revision-2 purpose row behind the instruction row,
    // so a carry-forward that forgets the kind discriminator cannot pass by
    // rowid luck.
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "DELETE FROM task_context_entry WHERE entry_id = ?1",
                params![crate::codec::encode_id(carried_in_force.as_raw())],
            )
            .expect("the purpose row must delete");
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, 2, 1, 'adopted_purpose', 'owner_conversation', ?3, ?4)",
                params![
                    crate::codec::encode_id(carried_in_force.as_raw()),
                    crate::codec::encode_id(created.task.as_raw()),
                    crate::codec::encode_id(creation.origin.source),
                    creation.acquired_at.to_rfc3339(),
                ],
            )
            .expect("the purpose row must reinsert");
    }

    let carried = TaskContextEntryId::generate();
    let third = store
        .forward_steering(TaskCommitPremise {
            expected: second,
            new_purpose: None,
            adopted_purpose_entry: carried,
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(third) = third else {
        panic!("expected CommittedAs, got {third:?}");
    };
    assert_eq!(third.revision, TaskRevision::from_u64(3));

    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.context.len(), 2);
    let purpose = &record.context[0];
    assert_eq!(purpose.entry, carried);
    assert_eq!(
        purpose.item,
        TaskContextItem::AdoptedPurpose(record.task.purpose)
    );
    assert_eq!(
        purpose.origin, creation.origin,
        "the carry copies the purpose row's provenance, not the instruction's"
    );
    assert_eq!(
        purpose.acquired_at.to_rfc3339(),
        creation.acquired_at.to_rfc3339(),
        "the carry copies the purpose row's acquisition time"
    );
    assert_eq!(
        record.context[1].item,
        TaskContextItem::AdoptedInstruction,
        "the instruction row is still part of the current context"
    );
    assert_eq!(record.context[1].entry, instruction.entry);
    assert_eq!(record.context[1].origin, instruction.origin);

    let rows = task_context_rows(&store, created.task);
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows[1].3, "adopted_instruction",
        "the physically first revision-2 row is the instruction"
    );
    assert_eq!(rows[1].1, None);
    assert_eq!(rows[2].3, "adopted_purpose");
    assert_eq!(rows[2].1, Some(1));
    assert_eq!(rows[3].0, 3);
    assert_eq!(rows[3].3, "adopted_purpose");
    assert_eq!(
        rows[3].5,
        crate::codec::encode_id(creation.origin.source),
        "the revision-3 purpose row carries revision 1's source"
    );
}

// --- Task: context item-kind read rules ---

#[tokio::test]
async fn task_load_rejects_an_unknown_context_item_kind() {
    let store = open_memory().await.unwrap();
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET item_kind = 'unknown' WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw())],
            )
            .expect("the kind probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an unknown stored item kind is a technical error, never skipped"
    );
}

#[tokio::test]
async fn task_load_rejects_context_kind_payload_mismatches() {
    // A purpose entry without its purpose identity payload.
    let store = open_memory().await.unwrap();
    let premise = task_premise(None);
    let _ = store.create_task(premise.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET purpose_adopted_revision = NULL WHERE task_id = ?1",
                params![crate::codec::encode_id(premise.task.as_raw())],
            )
            .expect("the payload probe must update");
    }
    assert!(
        matches!(
            store.load_task(premise.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an adopted-purpose entry without a payload is a technical error"
    );

    // An instruction entry that carries a purpose payload.
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("with instruction")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(task_instruction_adoption(RawId::new())),
        })
        .await
        .unwrap();
    assert!(
        matches!(outcome, TaskCommitOutcome::CommittedAs(_)),
        "the probe forward must commit, got {outcome:?}"
    );
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task_context_entry SET purpose_adopted_revision = 2 WHERE item_kind = 'adopted_instruction' AND task_id = ?1",
                params![crate::codec::encode_id(created.task.as_raw())],
            )
            .expect("the payload probe must update");
    }
    assert!(
        matches!(
            store.load_task(created.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an adopted-instruction entry with a purpose payload is a technical error"
    );
}

#[tokio::test]
async fn task_load_rejects_missing_or_duplicate_current_purpose_entries() {
    // The current revision's purpose entry is missing.
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("missing probe")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(task_instruction_adoption(RawId::new())),
        })
        .await
        .unwrap();
    assert!(matches!(outcome, TaskCommitOutcome::CommittedAs(_)));
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "DELETE FROM task_context_entry WHERE task_id = ?1 AND revision = 2 AND item_kind = 'adopted_purpose'",
                params![crate::codec::encode_id(created.task.as_raw())],
            )
            .expect("the missing-purpose probe must delete");
    }
    assert!(
        matches!(
            store.load_task(created.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an instruction row must not stand in for the missing purpose entry"
    );

    // The current revision carries two purpose entries.
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("duplicate probe")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert!(matches!(outcome, TaskCommitOutcome::CommittedAs(_)));
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, 2, 2, 'adopted_purpose', 'owner_conversation', ?3, ?4)",
                params![
                    crate::codec::encode_id(TaskContextEntryId::generate().as_raw()),
                    crate::codec::encode_id(created.task.as_raw()),
                    crate::codec::encode_id(RawId::new()),
                    fixture_clock().to_rfc3339(),
                ],
            )
            .expect("the duplicate-purpose probe must insert");
    }
    assert!(
        matches!(
            store.load_task(created.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "two current purpose entries are a technical error"
    );
}

#[tokio::test]
async fn task_load_rejects_instruction_entries_beyond_the_current_revision() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("current probe")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(task_instruction_adoption(RawId::new())),
        })
        .await
        .unwrap();
    assert!(matches!(outcome, TaskCommitOutcome::CommittedAs(_)));
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, item_kind, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, 3, NULL, 'adopted_instruction', 'owner_conversation', ?3, ?4)",
                params![
                    crate::codec::encode_id(TaskContextEntryId::generate().as_raw()),
                    crate::codec::encode_id(created.task.as_raw()),
                    crate::codec::encode_id(RawId::new()),
                    fixture_clock().to_rfc3339(),
                ],
            )
            .expect("the future-instruction probe must insert");
    }
    assert!(
        matches!(
            store.load_task(created.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "an instruction entry beyond the current revision is a technical error"
    );
}

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

/// One stored `delegation` row in column order:
/// `(delegation_id, task_id, task_revision, delegator, agent, scope_assoc,
/// scope_folder, scope_save_target)`.
type DelegationRow = (
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn delegation_row(store: &Store, delegation: DelegationId) -> Option<DelegationRow> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(
            "SELECT delegation_id, task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target FROM delegation WHERE delegation_id = ?1",
            params![crate::codec::encode_id(delegation.as_raw())],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .expect("the delegation probe must read")
}

#[tokio::test]
async fn delegation_creation_commits_with_and_without_a_workspace_scope() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();

    // A delegation that uses no workspace freezes an empty scope: all three
    // scope columns stay NULL.
    let bare = DelegationId::generate();
    let bare_agent = TaskAgentEphemeralId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            bare,
            created,
            bare_agent,
            delegation_scope(None),
        ))
        .await
        .unwrap();
    let DelegationOutcome::Delegated(reference) = outcome else {
        panic!("expected Delegated, got {outcome:?}");
    };
    assert_eq!(reference.delegation, bare);
    assert_eq!(
        reference.task, created,
        "the relied-on revision travels in the reference"
    );
    assert_eq!(
        reference.delegator, creation.assignee,
        "the delegator is copied from the current Task row"
    );
    assert_eq!(reference.agent, bare_agent);
    assert_eq!(reference.scope, delegation_scope(None));
    assert_eq!(
        delegation_row(&store, bare),
        Some((
            crate::codec::encode_id(bare.as_raw()),
            crate::codec::encode_id(created.task.as_raw()),
            1,
            crate::codec::encode_id(creation.assignee.companion),
            crate::codec::encode_id(bare_agent.as_raw()),
            None,
            None,
            None,
        )),
        "a bare scope persists as three NULL columns"
    );
    assert_eq!(store.load_delegation(bare).await.unwrap(), Some(reference));

    // A delegation that relied on a confirmed association freezes the
    // association's projection into the scope columns.
    let assoc = WorkspaceAssocId::generate();
    let scope = delegation_scope(Some(delegated_workspace(
        assoc,
        "/srv/workspace/au3",
        Some("/srv/workspace/au3/out"),
    )));
    let scoped = DelegationId::generate();
    let scoped_agent = TaskAgentEphemeralId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            scoped,
            created,
            scoped_agent,
            scope.clone(),
        ))
        .await
        .unwrap();
    let DelegationOutcome::Delegated(reference) = outcome else {
        panic!("expected Delegated, got {outcome:?}");
    };
    assert_eq!(reference.delegation, scoped);
    assert_eq!(reference.task, created);
    assert_eq!(reference.delegator, creation.assignee);
    assert_eq!(reference.agent, scoped_agent);
    assert_eq!(reference.scope, scope);
    assert_eq!(
        delegation_row(&store, scoped),
        Some((
            crate::codec::encode_id(scoped.as_raw()),
            crate::codec::encode_id(created.task.as_raw()),
            1,
            crate::codec::encode_id(creation.assignee.companion),
            crate::codec::encode_id(scoped_agent.as_raw()),
            Some(crate::codec::encode_id(assoc.as_raw())),
            Some(String::from("/srv/workspace/au3")),
            Some(String::from("/srv/workspace/au3/out")),
        )),
        "the frozen boundary persists its association, folder, and save target"
    );
    assert_eq!(
        store.load_delegation(scoped).await.unwrap(),
        Some(reference)
    );
    assert_eq!(
        task_table_count(&store, "delegation"),
        2,
        "each delegation keeps its own row"
    );
}

#[tokio::test]
async fn delegation_correspondence_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let (created, expected, before, scope) = {
        let store = Store::open(&path).await.unwrap();
        let creation = task_premise(Some(task_workspace(
            "/srv/workspace/au3",
            Some("/srv/workspace/au3/out"),
        )));
        let created = store.create_task(creation).await.unwrap();
        let delegation = DelegationId::generate();
        let scope = delegation_scope(Some(delegated_workspace(
            WorkspaceAssocId::generate(),
            "/srv/workspace/au3",
            Some("/srv/workspace/au3/out"),
        )));
        let outcome = store
            .create_delegation(delegation_premise(
                delegation,
                created,
                TaskAgentEphemeralId::generate(),
                scope.clone(),
            ))
            .await
            .unwrap();
        let DelegationOutcome::Delegated(expected) = outcome else {
            panic!("expected Delegated, got {outcome:?}");
        };
        assert_eq!(
            store.load_delegation(delegation).await.unwrap(),
            Some(expected.clone())
        );
        let before = store.load_task(created.task).await.unwrap().unwrap();
        (created, expected, before, scope)
    };

    let reopened = Store::open(&path).await.unwrap();
    let after = reopened
        .load_delegation(expected.delegation)
        .await
        .unwrap()
        .expect("the delegation correspondence must survive reopen");
    assert_eq!(
        after, expected,
        "the correlation survives reopen verbatim; it proves no liveness"
    );
    assert_eq!(after.scope, scope, "the frozen scope survives reopen");
    assert_eq!(
        reopened.load_task(created.task).await.unwrap(),
        Some(before),
        "reopen restores the task unit unchanged"
    );
    assert_eq!(
        task_table_count(&reopened, "delegation"),
        1,
        "reopen fabricates no extra delegation"
    );
    assert_eq!(
        task_table_count(&reopened, "task"),
        1,
        "reopen fabricates no extra task"
    );
    assert_eq!(
        task_table_count(&reopened, "task_revision"),
        1,
        "reopen fabricates no extra revision"
    );
    assert_eq!(
        reopened.load_delegation(DelegationId::generate()).await,
        Ok(None),
        "an absent correspondence is None, never fabricated"
    );
}

#[tokio::test]
async fn delegation_creation_is_stale_after_a_steering_forward() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation).await.unwrap();
    let second = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("moved before the delegation")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(second) = second else {
        panic!("expected CommittedAs, got {second:?}");
    };
    assert_eq!(second.revision, TaskRevision::from_u64(2));

    let before_current = task_current_row(&store, created.task);
    let before_revisions = task_revision_rows(&store, created.task);
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await;
    assert_eq!(
        outcome,
        Ok(DelegationOutcome::StaleTaskRevision { current: second }),
        "a moved revision is reported with the current one"
    );
    assert_eq!(
        task_table_count(&store, "delegation"),
        0,
        "a stale delegation writes zero rows"
    );
    assert_eq!(store.load_delegation(delegation).await, Ok(None));
    assert_eq!(
        task_current_row(&store, created.task),
        before_current,
        "the failed delegation attempt never advances the task"
    );
    assert_eq!(task_revision_rows(&store, created.task), before_revisions);
}

#[tokio::test]
async fn delegation_creation_races_a_steering_forward_without_torn_state() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation).await.unwrap();
    let delegation = DelegationId::generate();
    let premise = delegation_premise(
        delegation,
        created,
        TaskAgentEphemeralId::generate(),
        delegation_scope(None),
    );
    let steering = TaskCommitPremise {
        expected: created,
        new_purpose: Some(task_purpose_adoption("racing steering")),
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction: None,
    };
    let (steering, outcome) = tokio::join!(
        store.forward_steering(steering),
        store.create_delegation(premise)
    );

    // Delegation creation never advances the task revision, so the steering
    // CAS always commits: the only interleavings are delegation-then-steering
    // (the delegation stays bound to revision 1) and steering-then-delegation
    // (the delegation answers stale for revision 2).
    let steering = steering.unwrap();
    let TaskCommitOutcome::CommittedAs(advanced) = steering else {
        panic!("steering must commit at revision 2, got {steering:?}");
    };
    assert_eq!(advanced.revision, TaskRevision::from_u64(2));
    match outcome {
        Ok(DelegationOutcome::Delegated(reference)) => {
            assert_eq!(
                reference.task, created,
                "the committed delegation froze the revision it compared"
            );
            assert_eq!(
                delegation_row(&store, reference.delegation).map(|row| row.2),
                Some(1),
                "the durable row keeps the compared revision"
            );
            assert_eq!(
                task_table_count(&store, "delegation"),
                1,
                "exactly the winner's row is durable"
            );
        }
        Ok(DelegationOutcome::StaleTaskRevision { current }) => {
            assert_eq!(
                current, advanced,
                "the stale loser reports the advanced revision"
            );
            assert_eq!(
                task_table_count(&store, "delegation"),
                0,
                "the stale path leaves no partial delegation row"
            );
            assert_eq!(store.load_delegation(delegation).await, Ok(None));
        }
        other => panic!("unexpected delegation outcome: {other:?}"),
    }
    assert_eq!(
        task_current_row(&store, created.task).0,
        2,
        "the steering forward advanced exactly once"
    );
    assert_eq!(
        task_revision_rows(&store, created.task).len(),
        2,
        "no duplicate revision row exists"
    );
}

#[tokio::test]
async fn delegation_creation_reports_a_missing_task_as_a_domain_outcome() {
    let store = open_memory().await.unwrap();
    let missing = TaskId::generate();
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            TaskRef {
                task: missing,
                revision: TaskRevision::initial(),
            },
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await;
    assert_eq!(
        outcome,
        Ok(DelegationOutcome::MissingTask { task: missing }),
        "a missing Task is a domain outcome, not a storage failure"
    );
    assert_eq!(
        task_table_count(&store, "delegation"),
        0,
        "a missing task writes zero delegation rows"
    );
    assert_eq!(store.load_delegation(delegation).await, Ok(None));
}

#[tokio::test]
async fn delegation_multiple_rows_are_allowed_at_one_revision() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    let first = DelegationId::generate();
    let second = DelegationId::generate();
    assert_ne!(first, second, "re-delegation mints a new identity");
    let first_agent = TaskAgentEphemeralId::generate();
    let second_agent = TaskAgentEphemeralId::generate();
    let first_outcome = store
        .create_delegation(delegation_premise(
            first,
            created,
            first_agent,
            delegation_scope(None),
        ))
        .await
        .unwrap();
    let second_outcome = store
        .create_delegation(delegation_premise(
            second,
            created,
            second_agent,
            delegation_scope(None),
        ))
        .await
        .unwrap();
    let DelegationOutcome::Delegated(first_ref) = first_outcome else {
        panic!("expected Delegated, got {first_outcome:?}");
    };
    let DelegationOutcome::Delegated(second_ref) = second_outcome else {
        panic!("expected Delegated, got {second_outcome:?}");
    };
    assert_eq!(first_ref.delegation, first);
    assert_eq!(second_ref.delegation, second);
    assert_eq!(first_ref.task, created);
    assert_eq!(second_ref.task, created);
    assert_eq!(first_ref.agent, first_agent);
    assert_eq!(second_ref.agent, second_agent);
    assert_eq!(
        first_ref.delegator, creation.assignee,
        "each row copies the Task row's assignee"
    );
    assert_eq!(second_ref.delegator, creation.assignee);
    assert_eq!(
        task_table_count(&store, "delegation"),
        2,
        "parallel delegations of one revision are not collapsed"
    );
    assert_eq!(
        delegation_row(&store, first).map(|row| row.0),
        Some(crate::codec::encode_id(first.as_raw()))
    );
    assert_eq!(
        delegation_row(&store, second).map(|row| row.0),
        Some(crate::codec::encode_id(second.as_raw()))
    );
    assert_eq!(
        task_current_row(&store, created.task).0,
        1,
        "creating delegations never advances the task revision"
    );
}

/// Seeds one Task, applies `corrupt`, and requires `create_delegation` to fail
/// closed without writing a delegation row.
async fn assert_create_delegation_rejects(label: &str, corrupt: impl Fn(&Store, TaskId)) {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    corrupt(&store, created.task);
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await;
    assert!(
        matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "create_delegation must reject {label}, got {outcome:?}"
    );
    assert_eq!(
        task_table_count(&store, "delegation"),
        0,
        "a rejected {label} writes zero delegation rows"
    );
    assert_eq!(store.load_delegation(delegation).await, Ok(None));
}

#[tokio::test]
async fn delegation_creation_fails_closed_on_an_incoherent_task_unit() {
    for (label, statement) in [
        (
            "a missing revision snapshot",
            "DELETE FROM task_revision WHERE task_id = ?1",
        ),
        (
            "a disagreeing revision assignee",
            "UPDATE task_revision SET assignee = '11111111-1111-1111-1111-111111111111' WHERE task_id = ?1",
        ),
        (
            "a malformed revision assignee",
            "UPDATE task_revision SET assignee = 'not-an-id' WHERE task_id = ?1",
        ),
    ] {
        assert_create_delegation_rejects(label, |store, task| {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(statement, params![crate::codec::encode_id(task.as_raw())])
                .expect("the incoherent-unit probe must run");
        })
        .await;
    }
}

#[tokio::test]
async fn delegation_creation_accepts_equivalent_assignee_text_forms() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation.clone()).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        let stored: String = guard
            .query_row(
                "SELECT assignee FROM task_revision WHERE task_id = ?1",
                params![crate::codec::encode_id(created.task.as_raw())],
                |row| row.get(0),
            )
            .expect("the seeded revision row must read");
        let simple = stored.replace('-', "");
        guard
            .execute(
                "UPDATE task_revision SET assignee = ?2 WHERE task_id = ?1",
                params![crate::codec::encode_id(created.task.as_raw()), simple],
            )
            .expect("the equivalent text form must update");
    }
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    let DelegationOutcome::Delegated(reference) = outcome else {
        panic!("expected Delegated, got {outcome:?}");
    };
    assert_eq!(
        reference.delegator, creation.assignee,
        "both text forms decode to the same assignee"
    );
    assert_eq!(
        delegation_row(&store, delegation).map(|row| row.3),
        Some(crate::codec::encode_id(creation.assignee.companion)),
        "the copied delegator keeps the canonical stored text"
    );
}

#[tokio::test]
async fn delegation_creation_at_a_later_revision_binds_that_revision() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let moved = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("delegated after the move")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(moved) = moved else {
        panic!("expected CommittedAs, got {moved:?}");
    };
    assert_eq!(moved.revision, TaskRevision::from_u64(2));

    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            moved,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(
                WorkspaceAssocId::generate(),
                "/srv/workspace/moved",
                None,
            ))),
        ))
        .await
        .unwrap();
    let DelegationOutcome::Delegated(reference) = outcome else {
        panic!("expected Delegated, got {outcome:?}");
    };
    assert_eq!(
        reference.task, moved,
        "the delegation binds the relied-on revision, not the initial one"
    );
    assert_eq!(
        delegation_row(&store, delegation).map(|row| row.2),
        Some(2),
        "the durable row stores the relied-on revision"
    );
    assert_eq!(
        store.load_delegation(delegation).await,
        Ok(Some(reference)),
        "the correspondence round-trips at a later revision, folder-only scope included"
    );
}

/// Seeds one Task and one scoped delegation, applies `corrupt`, and requires
/// `load_delegation` to fail closed without deleting or rewriting the row.
async fn assert_load_delegation_rejects(label: &str, corrupt: impl Fn(&Store, DelegationId)) {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(
                WorkspaceAssocId::generate(),
                "/srv/workspace/corrupt",
                Some("/srv/workspace/corrupt/out"),
            ))),
        ))
        .await
        .unwrap();
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the corruption probe needs a committed row, got {outcome:?}"
    );
    corrupt(&store, delegation);
    let loaded = store.load_delegation(delegation).await;
    assert!(
        matches!(loaded, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "load_delegation must reject {label}, got {loaded:?}"
    );
    assert!(
        delegation_row(&store, delegation).is_some(),
        "a rejected read must not delete the {label} row"
    );
}

#[tokio::test]
async fn load_delegation_rejects_malformed_stored_values() {
    for (label, statement) in [
        (
            "a malformed task id",
            "UPDATE delegation SET task_id = 'not-an-id' WHERE delegation_id = ?1",
        ),
        (
            "a negative task revision",
            "UPDATE delegation SET task_revision = -1 WHERE delegation_id = ?1",
        ),
        (
            "a malformed delegator",
            "UPDATE delegation SET delegator = 'not-an-id' WHERE delegation_id = ?1",
        ),
        (
            "a malformed agent",
            "UPDATE delegation SET agent = 'not-an-id' WHERE delegation_id = ?1",
        ),
    ] {
        assert_load_delegation_rejects(label, |store, delegation| {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    statement,
                    params![crate::codec::encode_id(delegation.as_raw())],
                )
                .expect("the malformed-identity probe must update");
        })
        .await;
    }
}

#[tokio::test]
async fn load_delegation_rejects_inconsistent_scope_columns() {
    for (label, statement) in [
        (
            "assoc NULL with a folder",
            "UPDATE delegation SET scope_assoc = NULL, scope_folder = '/srv/orphan' WHERE delegation_id = ?1",
        ),
        (
            "assoc NULL with a save target",
            "UPDATE delegation SET scope_assoc = NULL, scope_save_target = '/srv/orphan/out' WHERE delegation_id = ?1",
        ),
        (
            "assoc NULL with a save target but no folder",
            "UPDATE delegation SET scope_assoc = NULL, scope_folder = NULL, scope_save_target = '/srv/orphan/out' WHERE delegation_id = ?1",
        ),
        (
            "assoc without a folder",
            "UPDATE delegation SET scope_folder = NULL WHERE delegation_id = ?1",
        ),
    ] {
        assert_load_delegation_rejects(label, |store, delegation| {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute(
                    statement,
                    params![crate::codec::encode_id(delegation.as_raw())],
                )
                .expect("the scope probe must update");
        })
        .await;
    }
}

#[tokio::test]
async fn delegation_creation_faults_roll_back_every_write() {
    let store = open_memory().await.unwrap();
    let creation = task_premise(None);
    let created = store.create_task(creation).await.unwrap();
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch(
                "CREATE TRIGGER au3_abort BEFORE INSERT ON delegation BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
            )
            .expect("the fault trigger must install");
    }
    let before_current = task_current_row(&store, created.task);
    let before_revisions = task_revision_rows(&store, created.task);
    let before_contexts = task_context_rows(&store, created.task);
    let delegation = DelegationId::generate();
    let premise = delegation_premise(
        delegation,
        created,
        TaskAgentEphemeralId::generate(),
        delegation_scope(Some(delegated_workspace(
            WorkspaceAssocId::generate(),
            "/srv/workspace/faulted",
            None,
        ))),
    );
    let failed = store.create_delegation(premise.clone()).await;
    assert!(
        matches!(failed, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "a fault on the delegation insert must surface a storage failure, got {failed:?}"
    );
    assert_eq!(
        task_table_count(&store, "delegation"),
        0,
        "a failed delegation leaves no row"
    );
    assert_eq!(store.load_delegation(delegation).await, Ok(None));
    assert_eq!(
        task_current_row(&store, created.task),
        before_current,
        "the task rows are untouched by the fault"
    );
    assert_eq!(task_revision_rows(&store, created.task), before_revisions);
    assert_eq!(task_context_rows(&store, created.task), before_contexts);
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch("DROP TRIGGER au3_abort;")
            .expect("the fault trigger must drop");
    }
    let committed = store
        .create_delegation(premise)
        .await
        .expect("the same premise succeeds after the fault clears");
    assert!(
        matches!(committed, DelegationOutcome::Delegated(_)),
        "the retried premise commits, got {committed:?}"
    );
    assert_eq!(task_table_count(&store, "delegation"), 1);
}

// --- Delegation: V17 migration ---

#[tokio::test]
async fn delegation_migration_adds_the_table_to_a_v16_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let (creation, created, dropped) = {
        let store = Store::open(&path).await.expect("a fresh store must open");
        assert_eq!(
            read_schema_version(&path),
            Some(18),
            "a fresh database converges on v17"
        );
        assert!(
            !table_columns(&path, "delegation").is_empty(),
            "a fresh database builds the delegation table"
        );
        let creation = task_premise(Some(task_workspace(
            "/srv/workspace/au3-migration",
            Some("/srv/workspace/au3-migration/out"),
        )));
        let created = store
            .create_task(creation.clone())
            .await
            .expect("the seed task must commit");
        let dropped = DelegationId::generate();
        let outcome = store
            .create_delegation(delegation_premise(
                dropped,
                created,
                TaskAgentEphemeralId::generate(),
                delegation_scope(None),
            ))
            .await
            .expect("the seed delegation must commit");
        assert!(
            matches!(outcome, DelegationOutcome::Delegated(_)),
            "the seed delegation must commit, got {outcome:?}"
        );
        (creation, created, dropped)
    };

    {
        let conn = rusqlite::Connection::open(&path).expect("the rewind must open");
        conn.execute_batch(
            "DROP TABLE delegation;
             PRAGMA user_version = 16;",
        )
        .expect("the version-16 rewind must apply");
    }
    assert_eq!(
        read_schema_version(&path),
        Some(16),
        "the seed must sit at the v16 boundary"
    );
    assert!(
        table_columns(&path, "delegation").is_empty(),
        "the rewind drops the delegation table"
    );

    let reopened = Store::open(&path)
        .await
        .expect("the V17 migration must succeed");
    assert_eq!(
        read_schema_version(&path),
        Some(18),
        "a v16 database converges on v17"
    );
    let columns = table_columns(&path, "delegation");
    for column in [
        "delegation_id",
        "task_id",
        "task_revision",
        "delegator",
        "agent",
        "scope_assoc",
        "scope_folder",
        "scope_save_target",
    ] {
        assert!(
            columns.contains(&String::from(column)),
            "the migrated delegation table has {column}"
        );
    }

    // The rewind dropped only the delegation table: the pre-existing task
    // unit still loads, and the dropped correspondence never resurrects.
    let record = reopened
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the pre-migration task must survive");
    assert_eq!(record.task.reference, created);
    assert_eq!(record.revision.purpose_text, creation.purpose);
    assert_eq!(
        reopened.load_delegation(dropped).await,
        Ok(None),
        "the rewound correspondence is gone, not fabricated"
    );

    // The re-created table answers create and load, including a scope copied
    // from the association that survived the migration.
    let association = record
        .workspace
        .as_ref()
        .expect("the confirmed association survives");
    let workspace = delegated_workspace(
        association.assoc,
        &association.folder.path,
        association
            .save_target
            .as_ref()
            .map(|target| target.path.as_str()),
    );
    let fresh = DelegationId::generate();
    let outcome = reopened
        .create_delegation(delegation_premise(
            fresh,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(workspace.clone())),
        ))
        .await
        .expect("creation must work after the migration");
    let DelegationOutcome::Delegated(reference) = outcome else {
        panic!("expected Delegated, got {outcome:?}");
    };
    assert_eq!(reference.task, created);
    assert_eq!(reference.delegator, creation.assignee);
    assert_eq!(reference.scope, delegation_scope(Some(workspace)));
    assert_eq!(
        reopened.load_delegation(fresh).await.unwrap(),
        Some(reference)
    );
}

mod agent;
