//! Presence attribution, relocation hint, and startup normalization (Stage 5
//! slice B): AU7 atomic hint updates, the PR §6.4 startup table, S5-14
//! recovery-wait competition, and the S5-05 confirm-time currentness check.

use super::*;

use ene_presence::{
    PresenceAttribution, RelocationHint, StartupNormalizationFailureReason, StopCompanionOutcome,
};

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
async fn startup_normalization_refreshes_recovery_wait_and_keeps_waiting() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client = ClientId::generate();
    let present = attach(&store, companion, client, generation).await;
    store.normalize_on_startup().await.unwrap();
    let waiting = load_fact(&store, companion).await;

    // A crash during startup leaves RecoveryWait; the next startup issues a
    // new generation and keeps the same destination instead of losing intent.
    let report = store.normalize_on_startup().await.unwrap();
    assert_eq!(report.normalized.len(), 1);
    let again = load_fact(&store, companion).await;
    assert_eq!(again.state, PresenceState::RecoveryWait);
    assert_eq!(again.generation.as_u64(), waiting.generation.as_u64() + 1);
    let hint = load_hint(&store, companion).await;
    assert_eq!(hint.recovery_destination, Some(client));

    // No timer and no self-report can steal the wait: a live confirm from a
    // different client on the waiting generation is an idempotent read-back,
    // never a Present crowning.
    let intruder = ClientId::generate();
    let confirmed = store
        .confirm_transition(
            companion.as_raw(),
            again.generation,
            LiveReachabilityRef {
                client: intruder,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    assert!(matches!(
        confirmed,
        ConfirmTransitionOutcome::Confirmed(ref fact)
            if fact.state == PresenceState::RecoveryWait && fact.active_client.is_none()
    ));
    assert_eq!(
        load_fact(&store, companion).await.state,
        PresenceState::RecoveryWait
    );
    assert_eq!(present.as_u64() + 2, again.generation.as_u64());
}

#[tokio::test]
async fn startup_normalization_never_moves_in_transition_to_its_candidate_or_last_client() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_b = ClientId::generate();
    let present = attach(&store, companion, client_a, generation).await;
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: present,
                expected_state: PresenceState::Present,
                expected_active: Some(client_a),
            },
            Some(client_b),
            ThinMoveReason::DisconnectObserved,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { .. } = begin else {
        panic!("the move must transition, got {begin:?}");
    };

    let report = store.normalize_on_startup().await.unwrap();
    assert_eq!(report.normalized.len(), 1);
    let fact = load_fact(&store, companion).await;
    assert_eq!(fact.state, PresenceState::NoActive);
    assert_eq!(fact.active_client, None);
    let hint = load_hint(&store, companion).await;
    assert_eq!(
        hint.recovery_destination, None,
        "an unconfirmed move leaves no recovery intent"
    );
    assert_eq!(
        hint.last_client,
        Some(client_a),
        "the last confirmed client stays as history"
    );
    assert_ne!(fact.active_client, Some(client_b));
}

#[tokio::test]
async fn startup_normalization_keeps_no_active_without_recovering() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client = ClientId::generate();
    let present = attach(&store, companion, client, generation).await;
    detach(&store, companion, client, present).await;
    let no_active = load_fact(&store, companion).await;
    assert_eq!(no_active.state, PresenceState::NoActive);

    let report = store.normalize_on_startup().await.unwrap();
    assert_eq!(report.normalized.len(), 1);
    let fact = load_fact(&store, companion).await;
    assert_eq!(fact.state, PresenceState::NoActive);
    assert_eq!(fact.active_client, None);
    assert_eq!(fact.generation.as_u64(), no_active.generation.as_u64() + 1);
    let hint = load_hint(&store, companion).await;
    assert_eq!(
        hint.recovery_destination, None,
        "NoActive never recovers and never sets a destination"
    );
    assert_eq!(
        hint.last_client,
        Some(client),
        "last_client survives as history"
    );
}

#[tokio::test]
async fn startup_normalization_never_recovers_stopped_or_deleted_lifecycles() {
    for lifecycle in [CompanionLifecycle::Stopped, CompanionLifecycle::Deleted] {
        let store = open_memory().await.unwrap();
        let (companion, generation) = running_companion(&store).await.unwrap();
        let client = ClientId::generate();
        attach(&store, companion, client, generation).await;
        execute(
            &store,
            "UPDATE companion SET lifecycle = ?1 WHERE companion_id = ?2",
            &[
                &crate::codec::encode_lifecycle(lifecycle),
                &crate::codec::encode_id(companion.as_raw()),
            ],
        );

        let report = store.normalize_on_startup().await.unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.normalized.len(), 1);
        let fact = load_fact(&store, companion).await;
        assert_eq!(
            fact.state,
            PresenceState::Stopped,
            "{lifecycle:?} never recovers"
        );
        assert_eq!(fact.active_client, None);
        let hint = load_hint(&store, companion).await;
        assert_eq!(hint.recovery_destination, None);
        assert_eq!(hint.last_client, Some(client));

        // Already normalized rows are left alone: this is a startup
        // normalization, not a periodic sweep.
        let again = store.normalize_on_startup().await.unwrap();
        assert!(again.normalized.is_empty());
        assert!(again.failures.is_empty());
        assert_eq!(load_fact(&store, companion).await, fact);
    }
}

#[tokio::test]
async fn startup_normalization_refuses_an_exhausted_generation_without_guessing() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client = ClientId::generate();
    attach(&store, companion, client, generation).await;
    execute(
        &store,
        "UPDATE presence_attribution SET generation = ?1 WHERE companion_id = ?2",
        &[&i64::MAX, &crate::codec::encode_id(companion.as_raw())],
    );
    let before = load_fact(&store, companion).await;
    assert_eq!(before.generation.as_u64(), i64::MAX as u64);

    let report = store.normalize_on_startup().await.unwrap();
    assert!(
        report.normalized.is_empty(),
        "an exhausted generation commits nothing"
    );
    assert_eq!(report.failures.len(), 1);
    let failure = report.failures[0];
    assert_eq!(failure.companion, companion.as_raw());
    assert_eq!(
        failure.reason,
        StartupNormalizationFailureReason::GenerationExhausted
    );
    assert_eq!(
        load_fact(&store, companion).await,
        before,
        "the old attribution is left untouched"
    );
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
async fn confirm_after_a_stop_is_rejected_and_never_presents() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_b = ClientId::generate();
    let present = attach(&store, companion, client_a, generation).await;
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: present,
                expected_state: PresenceState::Present,
                expected_active: Some(client_a),
            },
            Some(client_b),
            ThinMoveReason::DisconnectObserved,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the move must transition, got {begin:?}");
    };
    store.stop_companion(companion.as_raw()).await.unwrap();

    // Stop outranks the pending transition: the old confirm premise is
    // rejected and never crowns the candidate.
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
    assert!(
        matches!(
            confirmed,
            ConfirmTransitionOutcome::RejectedAsStalePresence { .. }
        ),
        "a stopped row must not confirm a pending transition, got {confirmed:?}"
    );
    let fact = load_fact(&store, companion).await;
    assert_eq!(fact.state, PresenceState::Stopped);
    assert_eq!(fact.active_client, None);
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

#[tokio::test]
async fn recovery_wait_stop_commits_first_and_the_late_restore_loses() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_c = ClientId::generate();
    let present = attach(&store, companion, client_a, generation).await;
    store.normalize_on_startup().await.unwrap();
    let waiting = load_fact(&store, companion).await;
    assert_eq!(waiting.generation.as_u64(), present.as_u64() + 1);

    store.stop_companion(companion.as_raw()).await.unwrap();
    let stopped = load_fact(&store, companion).await;
    assert_eq!(stopped.state, PresenceState::Stopped);
    assert_eq!(
        load_hint(&store, companion).await.recovery_destination,
        None,
        "stop clears the recovery destination"
    );

    let late = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: waiting.generation,
                expected_state: PresenceState::RecoveryWait,
                expected_active: None,
            },
            Some(client_c),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    assert!(
        matches!(late, MoveDecision::RejectedAsStalePresence { .. }),
        "the late summon observes the stopped state, got {late:?}"
    );
    let direct = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: stopped.generation,
                expected_state: PresenceState::Stopped,
                expected_active: None,
            },
            Some(client_c),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    assert!(
        matches!(direct, MoveDecision::DeniedByConstraint { .. }),
        "a stopped companion is never summoned, got {direct:?}"
    );
    assert_eq!(
        load_fact(&store, companion).await.state,
        PresenceState::Stopped
    );
}

#[tokio::test]
async fn fallback_target_lost_before_confirm_never_becomes_present() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let client_a = ClientId::generate();
    let client_b = ClientId::generate();
    let present = attach(&store, companion, client_a, generation).await;

    // The disconnect fallback pins B at begin time.
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: present,
                expected_state: PresenceState::Present,
                expected_active: Some(client_a),
            },
            Some(client_b),
            ThinMoveReason::DisconnectObserved,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the fallback must transition, got {begin:?}");
    };

    // B lost its currentness before confirm: the confirm must not crown it.
    let lost = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client: client_b,
                connection_live: false,
            },
        )
        .await
        .expect("confirm answers");
    let ConfirmTransitionOutcome::Confirmed(fact) = lost else {
        panic!("a not-live confirm must answer, got {lost:?}");
    };
    assert_eq!(fact.state, PresenceState::NoActive);
    assert_eq!(fact.active_client, None);
    let hint = load_hint(&store, companion).await;
    assert_eq!(hint.last_client, Some(client_a));
    assert_eq!(hint.recovery_destination, None);

    // A confirm naming a client other than the pinned target is stale and
    // leaves the transition row untouched; the pinned target can still be
    // confirmed, and only a live premise makes it Present.
    let begin = store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: fact.generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            Some(client_b),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation: next } = begin else {
        panic!("the second fallback must transition, got {begin:?}");
    };
    let unpinned = store
        .confirm_transition(
            companion.as_raw(),
            next,
            LiveReachabilityRef {
                client: client_a,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    assert!(
        matches!(
            unpinned,
            ConfirmTransitionOutcome::RejectedAsStalePresence { .. }
        ),
        "an unpinned confirm must reject, got {unpinned:?}"
    );
    assert_eq!(
        load_fact(&store, companion).await.state,
        PresenceState::InTransition
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
}
