//! Presence attribution, relocation hint, and startup normalization (Stage 5
//! slice B): AU7 atomic hint updates, the PR §6.4 startup table, S5-14
//! recovery-wait competition, and the S5-05 confirm-time currentness check.

use super::*;

use ene_presence::{PresenceAttribution, RelocationHint, StopCompanionOutcome};

/// One transition-log row: `(old_state, new_state, old_gen, new_gen, reason)`.
type TransitionRow = (String, String, i64, i64, String);

fn transition_rows(store: &Store, companion: CompanionId) -> Vec<TransitionRow> {
    let guard = store.conn.lock().expect("store lock");
    let mut statement = guard
        .prepare("SELECT old_state, new_state, old_gen, new_gen, reason FROM presence_transition_log WHERE companion_id = ?1 ORDER BY transition_seq")
        .expect("transition statement");
    let rows = statement
        .query_map(
            params![crate::codec::encode_id(companion.as_raw())],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("transition rows");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("transition rows decode")
}

fn execute(store: &Store, sql: &str, parameters: &[&dyn rusqlite::ToSql]) {
    let guard = store.conn.lock().expect("store lock");
    guard.execute(sql, parameters).expect("fixture write");
}

/// Installs the AU7 fault trigger: the next transition-log insert aborts, so
/// the surrounding transaction must roll every other write back with it.
fn install_transition_fault(store: &Store) {
    execute(
        store,
        "CREATE TRIGGER au7_abort BEFORE INSERT ON presence_transition_log BEGIN SELECT RAISE(ABORT, 'au7'); END",
        &[],
    );
}

fn drop_transition_fault(store: &Store) {
    execute(store, "DROP TRIGGER au7_abort", &[]);
}

/// Moves a `NoActive` companion to `Present` on `client`, returning the
/// committed generation.
async fn attach(
    store: &Store,
    companion: CompanionId,
    client: ClientId,
    generation: PresenceGeneration,
) -> PresenceGeneration {
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the attach begin must transition, got {begin:?}");
    };
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    let ConfirmTransitionOutcome::Confirmed(fact) = confirmed else {
        panic!("the live confirm must crown the pinned client, got {confirmed:?}");
    };
    assert_eq!(fact.state, PresenceState::Present);
    fact.generation
}

/// Detaches through the normal-disconnect fallback shape: begin to `None` and
/// confirm not-live, ending at `NoActive`.
async fn detach(
    store: &Store,
    companion: CompanionId,
    client: ClientId,
    generation: PresenceGeneration,
) {
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: generation,
                expected_state: PresenceState::Present,
                expected_active: Some(client),
            },
            None,
            ThinMoveReason::DisconnectObserved,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the disconnect begin must transition, got {begin:?}");
    };
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client,
                connection_live: false,
            },
        )
        .await
        .expect("confirm answers");
    let ConfirmTransitionOutcome::Confirmed(fact) = confirmed else {
        panic!("the not-live confirm must answer NoActive, got {confirmed:?}");
    };
    assert_eq!(fact.state, PresenceState::NoActive);
}

async fn load_fact(store: &Store, companion: CompanionId) -> PresenceAttribution {
    let loaded = store.load_attribution(companion.as_raw()).await;
    let Some(fact) = loaded.expect("attribution loads") else {
        panic!("attribution row must exist");
    };
    fact
}

async fn load_hint(store: &Store, companion: CompanionId) -> RelocationHint {
    let loaded = store.load_hint(companion.as_raw()).await;
    let Some(hint) = loaded.expect("hint loads") else {
        panic!("hint row must exist");
    };
    hint
}

#[tokio::test]
async fn begin_and_confirm_update_hint_and_log_in_the_attribution_transaction() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_b = ClientId::generate();

    // The seeded hint is empty history: no last client, no recovery intent.
    let seeded = load_hint(&store, companion).await;
    assert_eq!(seeded.last_client, None);
    assert_eq!(seeded.recovery_destination, None);

    let present = attach(&store, companion, client_a, generation).await;
    let after_attach = load_hint(&store, companion).await;
    assert_eq!(
        after_attach.last_client,
        Some(client_a),
        "a Present confirm records the last confirmed client"
    );
    assert_eq!(after_attach.recovery_destination, None);

    // Startup recovery seeds the recovery intent, so the faulted begin below
    // has a destination that must survive the rollback.
    let report = store.normalize_on_startup().await.unwrap();
    assert_eq!(report.normalized.len(), 1);
    let waiting = load_fact(&store, companion).await;
    assert_eq!(waiting.state, PresenceState::RecoveryWait);
    assert_eq!(waiting.generation.as_u64(), present.as_u64() + 1);
    let hint = load_hint(&store, companion).await;
    assert_eq!(hint.recovery_destination, Some(client_a));

    // AU7: attribution, hint, and transition log commit or roll back together.
    install_transition_fault(&store);
    let before = transition_rows(&store, companion).len();
    let faulted = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: waiting.generation,
                expected_state: PresenceState::RecoveryWait,
                expected_active: None,
            },
            Some(client_b),
            ThinMoveReason::InitialAttach,
        )
        .await;
    assert!(
        faulted.is_err(),
        "the faulted transition-log insert must fail the begin"
    );
    let rolled_back = load_fact(&store, companion).await;
    assert_eq!(
        rolled_back.state,
        PresenceState::RecoveryWait,
        "a failed begin leaves the attribution untouched"
    );
    assert_eq!(rolled_back.generation, waiting.generation);
    let rolled_back_hint = load_hint(&store, companion).await;
    assert_eq!(
        rolled_back_hint.recovery_destination,
        Some(client_a),
        "a failed begin rolls the hint destination back"
    );
    assert_eq!(rolled_back_hint.last_client, Some(client_a));
    assert_eq!(
        transition_rows(&store, companion).len(),
        before,
        "a failed begin appends no transition-log row"
    );

    // With the fault cleared, the begin commits and cancels the recovery
    // intent; the InTransition candidate never becomes `last_client`.
    drop_transition_fault(&store);
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: waiting.generation,
                expected_state: PresenceState::RecoveryWait,
                expected_active: None,
            },
            Some(client_b),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the cleared begin must transition, got {begin:?}");
    };
    let transitioning = load_fact(&store, companion).await;
    assert_eq!(transitioning.state, PresenceState::InTransition);
    assert_eq!(transitioning.active_client, Some(client_b));
    let transitioning_hint = load_hint(&store, companion).await;
    assert_eq!(
        transitioning_hint.last_client,
        Some(client_a),
        "the InTransition candidate never overwrites last_client"
    );
    assert_eq!(
        transitioning_hint.recovery_destination, None,
        "leaving RecoveryWait clears the recovery destination"
    );

    // A faulted confirm rolls the hint and log back with the attribution.
    install_transition_fault(&store);
    let before = transition_rows(&store, companion).len();
    let faulted = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client: client_b,
                connection_live: true,
            },
        )
        .await;
    assert!(
        faulted.is_err(),
        "the faulted confirm must fail on the transition-log insert"
    );
    let still_transitioning = load_fact(&store, companion).await;
    assert_eq!(still_transitioning.state, PresenceState::InTransition);
    assert_eq!(still_transitioning.active_client, Some(client_b));
    assert_eq!(
        load_hint(&store, companion).await.last_client,
        Some(client_a),
        "a failed confirm does not record the new client"
    );
    assert_eq!(transition_rows(&store, companion).len(), before);

    drop_transition_fault(&store);
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client: client_b,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    assert!(matches!(
        confirmed,
        ConfirmTransitionOutcome::Confirmed(ref fact)
            if fact.state == PresenceState::Present && fact.active_client == Some(client_b)
    ));
    let present_hint = load_hint(&store, companion).await;
    assert_eq!(present_hint.last_client, Some(client_b));
    assert_eq!(present_hint.recovery_destination, None);

    // A NoActive confirm keeps `last_client` as history.
    let present_b = load_fact(&store, companion).await;
    detach(&store, companion, client_b, present_b.generation).await;
    let no_active_hint = load_hint(&store, companion).await;
    assert_eq!(no_active_hint.last_client, Some(client_b));
    assert_eq!(no_active_hint.recovery_destination, None);
}

#[tokio::test]
async fn startup_normalization_moves_present_to_recovery_wait_with_a_new_generation() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client = ClientId::generate();
    let present = attach(&store, companion, client, generation).await;

    let report = store.normalize_on_startup().await.unwrap();
    assert!(report.failures.is_empty());
    assert_eq!(report.normalized.len(), 1);
    let fact = load_fact(&store, companion).await;
    assert_eq!(fact.state, PresenceState::RecoveryWait);
    assert_eq!(fact.active_client, None);
    assert_eq!(fact.generation.as_u64(), present.as_u64() + 1);
    let hint = load_hint(&store, companion).await;
    assert_eq!(
        hint.recovery_destination,
        Some(client),
        "the pre-restart Present client is the recovery destination"
    );
    assert_eq!(hint.last_client, Some(client));
    let log = transition_rows(&store, companion);
    let last = log.last().expect("normalization logs a transition");
    assert_eq!(last.0, "present");
    assert_eq!(last.1, "recovery_wait");
    assert_eq!(last.4, "restart_recovery");
}

#[tokio::test]
async fn stop_companion_clears_presence_and_keeps_history() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client = ClientId::generate();
    let present = attach(&store, companion, client, generation).await;
    let log_before = transition_rows(&store, companion).len();

    let stopped = store.stop_companion(companion.as_raw()).await.unwrap();
    let StopCompanionOutcome::Stopped(fact) = stopped else {
        panic!("a Present companion stops, got {stopped:?}");
    };
    assert_eq!(fact.state, PresenceState::Stopped);
    assert_eq!(fact.active_client, None);
    assert_eq!(
        fact.generation, present,
        "stop does not advance the generation"
    );
    let hint = load_hint(&store, companion).await;
    assert_eq!(hint.last_client, Some(client), "history survives the stop");
    assert_eq!(hint.recovery_destination, None);
    let log = transition_rows(&store, companion);
    assert_eq!(log.len(), log_before + 1);
    assert_eq!(log.last().unwrap().4, "stop");

    let again = store.stop_companion(companion.as_raw()).await.unwrap();
    assert!(
        matches!(again, StopCompanionOutcome::AlreadyStopped(_)),
        "a repeated stop is idempotent, got {again:?}"
    );
    assert_eq!(transition_rows(&store, companion).len(), log_before + 1);

    // Stop outranks move and recovery: even a caller that reads the stopped
    // row as current cannot begin a transition from it.
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: present,
                expected_state: PresenceState::Stopped,
                expected_active: None,
            },
            Some(ClientId::generate()),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    assert!(
        matches!(begin, MoveDecision::DeniedByConstraint { .. }),
        "a stopped companion is never moved, got {begin:?}"
    );

    let missing = store.stop_companion(RawId::new()).await.unwrap();
    assert_eq!(missing, StopCompanionOutcome::MissingCompanion);
}

#[tokio::test]
async fn recovery_wait_generation_cas_rejects_a_stale_summon_and_a_late_restore() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_b = ClientId::generate();
    let present = attach(&store, companion, client_a, generation).await;
    store.normalize_on_startup().await.unwrap();
    let waiting = load_fact(&store, companion).await;
    assert_eq!(waiting.state, PresenceState::RecoveryWait);

    // A summon holding the pre-restart generation is stale: the startup
    // generation advance invalidates every old premise.
    let stale = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: present,
                expected_state: PresenceState::Present,
                expected_active: Some(client_a),
            },
            Some(client_b),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    assert!(
        matches!(stale, MoveDecision::RejectedAsStalePresence { .. }),
        "a stale summon must be rejected, got {stale:?}"
    );

    // A summon on the current generation wins and cancels the recovery
    // intent; the late original client can no longer auto-restore.
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: waiting.generation,
                expected_state: PresenceState::RecoveryWait,
                expected_active: None,
            },
            Some(client_b),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the current-generation summon must transition, got {begin:?}");
    };
    assert_eq!(
        load_hint(&store, companion).await.recovery_destination,
        None,
        "a summon on the current generation cancels recovery intent"
    );
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client: client_b,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    assert!(matches!(
        confirmed,
        ConfirmTransitionOutcome::Confirmed(ref fact)
            if fact.state == PresenceState::Present && fact.active_client == Some(client_b)
    ));
    let late = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: waiting.generation,
                expected_state: PresenceState::RecoveryWait,
                expected_active: None,
            },
            Some(client_a),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    assert!(
        matches!(late, MoveDecision::RejectedAsStalePresence { .. }),
        "the late original client must not auto-restore, got {late:?}"
    );
    assert_eq!(
        load_fact(&store, companion).await.active_client,
        Some(client_b)
    );
}
