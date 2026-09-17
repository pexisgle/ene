use std::sync::Arc;

use ene_companion::CompanionLifecycle;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceGeneration, PresenceRepository, PresenceState,
    PresenceTechnicalError, RelocationHint, StartupNormalizationFailure,
    StartupNormalizationFailureReason, StartupNormalizationReport, StopCompanionOutcome,
    ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_id, decode_lifecycle, encode_id, encode_move_reason, encode_presence_state, encode_u64,
    lock_shared, presence_unavailable, select_attribution, select_hint,
};
use crate::run_blocking;

const SQL_UPDATE_ATTRIBUTION: &str = "UPDATE presence_attribution SET state = ?1, active_client = ?2, generation = ?3 WHERE companion_id = ?4";

const SQL_INSERT_TRANSITION: &str = "INSERT INTO presence_transition_log (companion_id, old_state, new_state, old_gen, new_gen, reason, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// Synchronous presence primitives for the connection-ownership section.
///
/// [`PresenceRepository`] wraps each of these in its own `spawn_blocking`
/// call, which is the right shape for standalone await callers. The connection
/// table's admission sections are different: they must decide and commit while
/// holding the connection-table lock, inside a single `spawn_blocking`
/// (CCT §10.4), so the same compare/commit is exposed synchronously and never
/// awaited across the section. The sync bodies carry the full slice B
/// semantics (stop outranks begin, encode-checked generations, relocation-hint
/// maintenance), so the close-admission fallback and the async repository
/// agree by construction.
impl Store {
    /// Synchronous [`PresenceRepository::load_attribution`].
    ///
    /// # Errors
    ///
    /// [`PresenceTechnicalError`] when the row cannot be read.
    pub fn load_attribution_sync(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let key = encode_id(companion);
        let guard = lock_shared(&self.conn);
        select_attribution(&guard, &key).map_err(presence_unavailable)
    }

    /// Synchronous [`PresenceRepository::compare_and_begin_transition`].
    ///
    /// # Errors
    ///
    /// [`PresenceTechnicalError`] when the transaction cannot commit.
    pub fn compare_and_begin_transition_sync(
        &self,
        companion: RawId,
        expected: PresenceCheckRef,
        to_client: Option<ClientId>,
        reason: ThinMoveReason,
    ) -> Result<MoveDecision, PresenceTechnicalError> {
        let key = encode_id(companion);
        let target_text = to_client.map(|client| encode_id(client.as_raw()));
        let reason_text = encode_move_reason(reason);
        let now_text = WallClockWithTz::now().to_rfc3339();
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| presence_unavailable(error.to_string()))?;
        let Some(current) = select_attribution(&tx, &key).map_err(presence_unavailable)? else {
            return Ok(MoveDecision::DeniedByConstraint {
                reason: String::from("unknown companion"),
            });
        };
        if current.generation != expected.expected_generation
            || current.state != expected.expected_state
            || current.active_client != expected.expected_active
        {
            return Ok(MoveDecision::RejectedAsStalePresence { current });
        }
        // Stop outranks move and recovery: a stopped companion is never
        // moved, summoned, or recovered by a transition begin.
        if current.state == PresenceState::Stopped {
            return Ok(MoveDecision::DeniedByConstraint {
                reason: String::from("companion stopped"),
            });
        }
        let Some(next_generation) = next_generation(current.generation) else {
            return Ok(MoveDecision::DeniedByConstraint {
                reason: String::from("presence generation exhausted"),
            });
        };
        let next_raw = encode_u64(next_generation.as_u64()).map_err(presence_unavailable)?;
        let current_raw = encode_u64(current.generation.as_u64()).map_err(presence_unavailable)?;
        tx.execute(
            SQL_UPDATE_ATTRIBUTION,
            params![
                encode_presence_state(PresenceState::InTransition),
                target_text,
                next_raw,
                key
            ],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        // Leaving RecoveryWait cancels the recovery intent; the candidate
        // lives only in the attribution and never becomes `last_client`.
        tx.execute(
            SQL_SET_HINT_DESTINATION,
            params![key, Option::<String>::None],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        tx.execute(
            SQL_INSERT_TRANSITION,
            params![
                key,
                encode_presence_state(current.state),
                encode_presence_state(PresenceState::InTransition),
                current_raw,
                next_raw,
                reason_text,
                now_text
            ],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        tx.commit()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        Ok(MoveDecision::TransitioningToNew {
            generation: next_generation,
        })
    }

    /// Synchronous [`PresenceRepository::confirm_transition`].
    ///
    /// # Errors
    ///
    /// [`PresenceTechnicalError`] when the transaction cannot commit.
    pub fn confirm_transition_sync(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<ConfirmTransitionOutcome, PresenceTechnicalError> {
        let key = encode_id(companion);
        let now_text = WallClockWithTz::now().to_rfc3339();
        let confirm_reason = if live.connection_live {
            "confirm_live"
        } else {
            "confirm_not_live"
        };
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| presence_unavailable(error.to_string()))?;
        let Some(current) = select_attribution(&tx, &key).map_err(presence_unavailable)? else {
            return Err(presence_unavailable(String::from(
                "missing presence attribution",
            )));
        };
        // Idempotent: only an `InTransition` row at the transitioning
        // generation moves; anything else reads back unchanged, except a
        // stopped row: stop outranks the transition, so the caller must
        // not read a stop as its own confirmation.
        if current.generation != transitioning_generation
            || current.state != PresenceState::InTransition
        {
            if current.state == PresenceState::Stopped {
                return Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { current });
            }
            return Ok(ConfirmTransitionOutcome::Confirmed(current));
        }
        // Authority pin: a live confirm may only crown the client pinned at
        // begin time (stored as the row's active client). A different
        // claimant leaves the row untouched and observes stale instead —
        // the connection table, not a self-report, decides who is current.
        if live.connection_live && current.active_client != Some(live.client) {
            return Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { current });
        }
        let (target_state, target_text, target_client) = if live.connection_live {
            (
                PresenceState::Present,
                Some(encode_id(live.client.as_raw())),
                Some(live.client),
            )
        } else {
            (PresenceState::NoActive, None, None)
        };
        let current_raw = encode_u64(current.generation.as_u64()).map_err(presence_unavailable)?;
        tx.execute(
            SQL_UPDATE_ATTRIBUTION,
            params![
                encode_presence_state(target_state),
                target_text,
                current_raw,
                key
            ],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        // `last_client` moves only when the attribution becomes Present;
        // a NoActive confirm keeps the last confirmed client as history.
        if let Some(client) = target_client {
            tx.execute(
                SQL_SET_HINT_PRESENT,
                params![key, encode_id(client.as_raw())],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
        } else {
            tx.execute(
                SQL_SET_HINT_DESTINATION,
                params![key, Option::<String>::None],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
        }
        tx.execute(
            SQL_INSERT_TRANSITION,
            params![
                key,
                encode_presence_state(PresenceState::InTransition),
                encode_presence_state(target_state),
                current_raw,
                current_raw,
                confirm_reason,
                now_text
            ],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        tx.commit()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        Ok(ConfirmTransitionOutcome::Confirmed(PresenceAttribution {
            companion,
            state: target_state,
            active_client: target_client,
            generation: transitioning_generation,
        }))
    }
}

/// Sets the recovery destination to `?2` (possibly `NULL`) without touching
/// `last_client`: begin and stop clear the destination, and startup recovery
/// refreshes it. `last_client` is only ever written by the Present confirm.
const SQL_SET_HINT_DESTINATION: &str = "INSERT INTO relocation_hint (companion_id, last_client, recovery_destination) VALUES (?1, NULL, ?2) ON CONFLICT(companion_id) DO UPDATE SET recovery_destination = excluded.recovery_destination";

/// Records the confirmed Present client as `last_client` and clears the
/// recovery destination: the hint leaves `RecoveryWait` together with the
/// attribution.
const SQL_SET_HINT_PRESENT: &str = "INSERT INTO relocation_hint (companion_id, last_client, recovery_destination) VALUES (?1, ?2, NULL) ON CONFLICT(companion_id) DO UPDATE SET last_client = excluded.last_client, recovery_destination = NULL";

const SQL_SELECT_COMPANIONS: &str = "SELECT companion_id, lifecycle FROM companion";

/// The next generation only when it can be encoded into the column's range;
/// a value that cannot be stored is exhaustion, never a wrapped write.
fn next_generation(current: PresenceGeneration) -> Option<PresenceGeneration> {
    let next = current.checked_next()?;
    encode_u64(next.as_u64()).ok().map(|_| next)
}

/// The committed shape of one companion's startup normalization.
struct StartupTarget {
    state: PresenceState,
    active_client: Option<ClientId>,
    generation: PresenceGeneration,
    recovery_destination: Option<ClientId>,
}

/// Plans one companion's startup row from the PR §6.4 table.
///
/// Running rows advance the generation (`Present` → `RecoveryWait` toward the
/// same client, `RecoveryWait` → new generation keeping the destination,
/// `InTransition` → `NoActive`, `NoActive` → keep). Stopped/Deleted rows and
/// rows already `Stopped` only clear the active client and the recovery
/// destination: the lifecycle wins and never recovers. Nothing here moves a
/// companion to a client it was not already confirmed on.
fn plan_startup(
    lifecycle: CompanionLifecycle,
    current: PresenceAttribution,
    hint: Option<RelocationHint>,
) -> Result<StartupTarget, StartupNormalizationFailureReason> {
    let cleanup = StartupTarget {
        state: PresenceState::Stopped,
        active_client: None,
        generation: current.generation,
        recovery_destination: None,
    };
    if lifecycle != CompanionLifecycle::Running || current.state == PresenceState::Stopped {
        return Ok(cleanup);
    }
    let Some(next) = next_generation(current.generation) else {
        return Err(StartupNormalizationFailureReason::GenerationExhausted);
    };
    match current.state {
        PresenceState::Present => {
            let Some(client) = current.active_client else {
                return Err(StartupNormalizationFailureReason::MalformedAttribution);
            };
            Ok(StartupTarget {
                state: PresenceState::RecoveryWait,
                active_client: None,
                generation: next,
                recovery_destination: Some(client),
            })
        }
        PresenceState::InTransition => Ok(StartupTarget {
            state: PresenceState::NoActive,
            active_client: None,
            generation: next,
            recovery_destination: None,
        }),
        PresenceState::NoActive => Ok(StartupTarget {
            state: PresenceState::NoActive,
            active_client: None,
            generation: next,
            recovery_destination: None,
        }),
        PresenceState::RecoveryWait => {
            let Some(destination) = hint.and_then(|hint| hint.recovery_destination) else {
                return Err(StartupNormalizationFailureReason::MissingRecoveryDestination);
            };
            Ok(StartupTarget {
                state: PresenceState::RecoveryWait,
                active_client: None,
                generation: next,
                recovery_destination: Some(destination),
            })
        }
        PresenceState::Stopped => Ok(cleanup),
    }
}

impl PresenceRepository for Store {
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let store = self.clone();
        run_blocking(move || store.load_attribution_sync(companion)).await
    }

    async fn load_hint(
        &self,
        companion: RawId,
    ) -> Result<Option<RelocationHint>, PresenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion);
            let guard = lock_shared(&conn);
            select_hint(&guard, &key).map_err(presence_unavailable)
        })
        .await
    }

    async fn compare_and_begin_transition(
        &self,
        companion: RawId,
        expected: PresenceCheckRef,
        to_client: Option<ClientId>,
        reason: ThinMoveReason,
    ) -> Result<MoveDecision, PresenceTechnicalError> {
        let store = self.clone();
        run_blocking(move || {
            store.compare_and_begin_transition_sync(companion, expected, to_client, reason)
        })
        .await
    }

    async fn confirm_transition(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<ConfirmTransitionOutcome, PresenceTechnicalError> {
        let store = self.clone();
        run_blocking(move || {
            store.confirm_transition_sync(companion, transitioning_generation, live)
        })
        .await
    }

    async fn normalize_on_startup(
        &self,
    ) -> Result<StartupNormalizationReport, PresenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let companions: Vec<(String, String)> = {
                let mut statement = guard
                    .prepare(SQL_SELECT_COMPANIONS)
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let rows = statement
                    .query_map((), |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|error| presence_unavailable(error.to_string()))?
            };
            let mut report = StartupNormalizationReport::default();
            for (companion_text, lifecycle_text) in companions {
                // Each companion's normalization is its own short transaction:
                // a refusal or a storage fault in one never rolls back or
                // blocks another companion's committed row.
                let tx = guard
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let companion = decode_id(&companion_text).map_err(presence_unavailable)?;
                let Some(current) =
                    select_attribution(&tx, &companion_text).map_err(presence_unavailable)?
                else {
                    report.failures.push(StartupNormalizationFailure {
                        companion,
                        reason: StartupNormalizationFailureReason::MissingAttribution,
                    });
                    continue;
                };
                let hint = select_hint(&tx, &companion_text).map_err(presence_unavailable)?;
                let lifecycle = decode_lifecycle(&lifecycle_text).map_err(presence_unavailable)?;
                let target = match plan_startup(lifecycle, current, hint) {
                    Ok(target) => target,
                    Err(reason) => {
                        report
                            .failures
                            .push(StartupNormalizationFailure { companion, reason });
                        continue;
                    }
                };
                let already_normalized = current.state == target.state
                    && current.active_client == target.active_client
                    && current.generation == target.generation
                    && hint.and_then(|hint| hint.recovery_destination)
                        == target.recovery_destination;
                if already_normalized {
                    continue;
                }
                let current_raw =
                    encode_u64(current.generation.as_u64()).map_err(presence_unavailable)?;
                let next_raw =
                    encode_u64(target.generation.as_u64()).map_err(presence_unavailable)?;
                let active_text = target
                    .active_client
                    .map(|client| encode_id(client.as_raw()));
                let destination_text = target
                    .recovery_destination
                    .map(|client| encode_id(client.as_raw()));
                tx.execute(
                    SQL_UPDATE_ATTRIBUTION,
                    params![
                        encode_presence_state(target.state),
                        active_text,
                        next_raw,
                        companion_text
                    ],
                )
                .map_err(|error| presence_unavailable(error.to_string()))?;
                tx.execute(
                    SQL_SET_HINT_DESTINATION,
                    params![companion_text, destination_text],
                )
                .map_err(|error| presence_unavailable(error.to_string()))?;
                tx.execute(
                    SQL_INSERT_TRANSITION,
                    params![
                        companion_text,
                        encode_presence_state(current.state),
                        encode_presence_state(target.state),
                        current_raw,
                        next_raw,
                        encode_move_reason(ThinMoveReason::RestartRecovery),
                        WallClockWithTz::now().to_rfc3339()
                    ],
                )
                .map_err(|error| presence_unavailable(error.to_string()))?;
                tx.commit()
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                report.normalized.push(PresenceAttribution {
                    companion,
                    state: target.state,
                    active_client: target.active_client,
                    generation: target.generation,
                });
            }
            Ok(report)
        })
        .await
    }

    async fn stop_companion(
        &self,
        companion: RawId,
    ) -> Result<StopCompanionOutcome, PresenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion);
            let now_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| presence_unavailable(error.to_string()))?;
            let Some(current) = select_attribution(&tx, &key).map_err(presence_unavailable)? else {
                return Ok(StopCompanionOutcome::MissingCompanion);
            };
            let hint = select_hint(&tx, &key).map_err(presence_unavailable)?;
            if current.state == PresenceState::Stopped
                && current.active_client.is_none()
                && hint
                    .as_ref()
                    .and_then(|hint| hint.recovery_destination)
                    .is_none()
            {
                return Ok(StopCompanionOutcome::AlreadyStopped(current));
            }
            let current_raw =
                encode_u64(current.generation.as_u64()).map_err(presence_unavailable)?;
            // Stop clears presence without advancing the generation: the
            // lifecycle change that would make the companion movable again is
            // a separate owner's decision, and the stopped state itself
            // rejects every later begin.
            tx.execute(
                SQL_UPDATE_ATTRIBUTION,
                params![
                    encode_presence_state(PresenceState::Stopped),
                    Option::<String>::None,
                    current_raw,
                    key
                ],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
            tx.execute(
                SQL_SET_HINT_DESTINATION,
                params![key, Option::<String>::None],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
            tx.execute(
                SQL_INSERT_TRANSITION,
                params![
                    key,
                    encode_presence_state(current.state),
                    encode_presence_state(PresenceState::Stopped),
                    current_raw,
                    current_raw,
                    encode_move_reason(ThinMoveReason::Stop),
                    now_text
                ],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| presence_unavailable(error.to_string()))?;
            Ok(StopCompanionOutcome::Stopped(PresenceAttribution {
                companion,
                state: PresenceState::Stopped,
                active_client: None,
                generation: current.generation,
            }))
        })
        .await
    }
}
