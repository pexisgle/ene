use std::sync::Arc;

use ene_companion::CompanionLifecycle;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceErasureRepository, PresenceGeneration, PresenceRepository,
    PresenceState, PresenceTechnicalError, RelocationHint, StartupNormalizationFailure,
    StartupNormalizationFailureReason, StartupNormalizationReport, StopCompanionOutcome,
    ThinMoveReason,
};
use ene_preservation::{ErasureConditionRef, LocalErasurePass};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_id, decode_lifecycle, encode_id, encode_move_reason, encode_presence_state, encode_u64,
    lock_shared, presence_unavailable, select_attribution, select_hint,
};
use crate::erasure::{ERASURE_BATCH_ROWS, erasure_count};
use crate::preservation::condition_is_current;
use crate::run_blocking;

const SQL_UPDATE_ATTRIBUTION: &str = "UPDATE presence_attribution SET state = ?1, active_client = ?2, generation = ?3 WHERE companion_id = ?4";

const SQL_INSERT_TRANSITION: &str = "INSERT INTO presence_transition_log (companion_id, old_state, new_state, old_gen, new_gen, reason, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

fn write_attribution_tx(
    tx: &rusqlite::Transaction<'_>,
    old: PresenceAttribution,
    new: PresenceAttribution,
    reason: &str,
    at: &str,
) -> Result<(), PresenceTechnicalError> {
    let key = encode_id(new.companion);
    let old_raw = encode_u64(old.generation.as_u64()).map_err(presence_unavailable)?;
    let new_raw = encode_u64(new.generation.as_u64()).map_err(presence_unavailable)?;
    let client = new.active_client.map(|client| encode_id(client.as_raw()));
    tx.execute(
        SQL_UPDATE_ATTRIBUTION,
        params![encode_presence_state(new.state), client, new_raw, key],
    )
    .map_err(|error| presence_unavailable(error.to_string()))?;
    tx.execute(
        SQL_INSERT_TRANSITION,
        params![
            key,
            encode_presence_state(old.state),
            encode_presence_state(new.state),
            old_raw,
            new_raw,
            reason,
            at
        ],
    )
    .map_err(|error| presence_unavailable(error.to_string()))?;
    Ok(())
}

impl Store {
    pub fn load_attribution_sync(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let key = encode_id(companion);
        let guard = lock_shared(&self.conn);
        select_attribution(&guard, &key).map_err(presence_unavailable)
    }

    pub fn compare_and_begin_transition_sync(
        &self,
        companion: RawId,
        expected: PresenceCheckRef,
        to_client: Option<ClientId>,
        reason: ThinMoveReason,
    ) -> Result<MoveDecision, PresenceTechnicalError> {
        let key = encode_id(companion);
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
        write_attribution_tx(
            &tx,
            current,
            PresenceAttribution {
                companion,
                state: PresenceState::InTransition,
                active_client: to_client,
                generation: next_generation,
            },
            reason_text,
            &now_text,
        )?;
        tx.execute(
            SQL_UPSERT_HINT,
            params![key, Option::<String>::None, Option::<String>::None],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        tx.commit()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        Ok(MoveDecision::TransitioningToNew {
            generation: next_generation,
        })
    }

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
        if current.generation != transitioning_generation {
            return Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { current });
        }
        if current.state == PresenceState::Stopped {
            return Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { current });
        }
        if current.state != PresenceState::InTransition {
            return Ok(ConfirmTransitionOutcome::Confirmed(current));
        }
        if live.connection_live && current.active_client != Some(live.client) {
            return Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { current });
        }
        let (target_state, target_client) = if live.connection_live {
            (PresenceState::Present, Some(live.client))
        } else {
            (PresenceState::NoActive, None)
        };
        let confirmed = PresenceAttribution {
            companion,
            state: target_state,
            active_client: target_client,
            generation: transitioning_generation,
        };
        write_attribution_tx(&tx, current, confirmed, confirm_reason, &now_text)?;
        if let Some(client) = target_client {
            tx.execute(
                SQL_UPSERT_HINT,
                params![
                    key,
                    Some(encode_id(client.as_raw())),
                    Option::<String>::None
                ],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
        } else {
            tx.execute(
                SQL_UPSERT_HINT,
                params![key, Option::<String>::None, Option::<String>::None],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        Ok(ConfirmTransitionOutcome::Confirmed(confirmed))
    }
}

pub(crate) const SQL_UPSERT_HINT: &str = "INSERT INTO relocation_hint (companion_id, last_client, recovery_destination) VALUES (?1, ?2, ?3) ON CONFLICT(companion_id) DO UPDATE SET last_client = COALESCE(excluded.last_client, relocation_hint.last_client), recovery_destination = excluded.recovery_destination";

const SQL_SELECT_COMPANIONS: &str = "SELECT companion_id, lifecycle FROM companion";

fn next_generation(current: PresenceGeneration) -> Option<PresenceGeneration> {
    let next = current.checked_next()?;
    encode_u64(next.as_u64()).ok().map(|_| next)
}

struct StartupTarget {
    state: PresenceState,
    active_client: Option<ClientId>,
    generation: PresenceGeneration,
    recovery_destination: Option<ClientId>,
}

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
                let destination_text = target
                    .recovery_destination
                    .map(|client| encode_id(client.as_raw()));
                write_attribution_tx(
                    &tx,
                    current,
                    PresenceAttribution {
                        companion,
                        state: target.state,
                        active_client: target.active_client,
                        generation: target.generation,
                    },
                    encode_move_reason(ThinMoveReason::RestartRecovery),
                    &WallClockWithTz::now().to_rfc3339(),
                )?;
                tx.execute(
                    SQL_UPSERT_HINT,
                    params![companion_text, Option::<String>::None, destination_text],
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
            let stopped = PresenceAttribution {
                companion,
                state: PresenceState::Stopped,
                active_client: None,
                generation: current.generation,
            };
            write_attribution_tx(
                &tx,
                current,
                stopped,
                encode_move_reason(ThinMoveReason::Stop),
                &now_text,
            )?;
            tx.execute(
                SQL_UPSERT_HINT,
                params![key, Option::<String>::None, Option::<String>::None],
            )
            .map_err(|error| presence_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| presence_unavailable(error.to_string()))?;
            Ok(StopCompanionOutcome::Stopped(stopped))
        })
        .await
    }
}

const SQL_ERASE_ATTRIBUTION_IDENTITY: &str = "DELETE FROM presence_attribution
     WHERE companion_id IN (
         SELECT companion_id FROM presence_attribution
         WHERE instr(companion_id, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_ATTRIBUTION_ACTIVE_CLIENT: &str = "UPDATE presence_attribution
     SET state = 'stopped', active_client = NULL
     WHERE companion_id IN (
         SELECT companion_id FROM presence_attribution
         WHERE active_client IS NOT NULL AND instr(active_client, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_HINT_IDENTITY: &str = "DELETE FROM relocation_hint
     WHERE companion_id IN (
         SELECT companion_id FROM relocation_hint
         WHERE instr(companion_id, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_HINT_LAST_CLIENT: &str = "UPDATE relocation_hint
     SET last_client = NULL
     WHERE companion_id IN (
         SELECT companion_id FROM relocation_hint
         WHERE last_client IS NOT NULL AND instr(last_client, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_HINT_RECOVERY_DESTINATION: &str = "UPDATE relocation_hint
     SET recovery_destination = NULL
     WHERE companion_id IN (
         SELECT companion_id FROM relocation_hint
         WHERE recovery_destination IS NOT NULL AND instr(recovery_destination, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_TRANSITION_LOG: &str = "DELETE FROM presence_transition_log
     WHERE transition_seq IN (
         SELECT transition_seq FROM presence_transition_log
         WHERE instr(companion_id, ?1) > 0
         LIMIT ?2
     )";

const SQL_COUNT_PRESENCE_TARGET: &str = "SELECT
     (SELECT COUNT(*) FROM presence_attribution
      WHERE instr(companion_id, ?1) > 0
         OR instr(COALESCE(active_client, ''), ?1) > 0)
   + (SELECT COUNT(*) FROM relocation_hint
      WHERE instr(companion_id, ?1) > 0
         OR instr(COALESCE(last_client, ''), ?1) > 0
         OR instr(COALESCE(recovery_destination, ''), ?1) > 0)
   + (SELECT COUNT(*) FROM presence_transition_log
      WHERE instr(companion_id, ?1) > 0)";

impl PresenceErasureRepository for Store {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<LocalErasurePass, PresenceTechnicalError>> + Send
    {
        let conn = Arc::clone(&self.conn);
        let target = target.to_owned();
        async move {
            run_blocking(move || {
                let mut guard = lock_shared(&conn);
                let tx = guard
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                if !condition_is_current(&tx, condition)
                    .map_err(|error| presence_unavailable(error.to_string()))?
                {
                    return Ok(LocalErasurePass::NotCurrent);
                }
                let attribution = tx
                    .execute(
                        SQL_ERASE_ATTRIBUTION_IDENTITY,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let stopped = tx
                    .execute(
                        SQL_ERASE_ATTRIBUTION_ACTIVE_CLIENT,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let hints = tx
                    .execute(SQL_ERASE_HINT_IDENTITY, params![target, ERASURE_BATCH_ROWS])
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let last_clients = tx
                    .execute(
                        SQL_ERASE_HINT_LAST_CLIENT,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let destinations = tx
                    .execute(
                        SQL_ERASE_HINT_RECOVERY_DESTINATION,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let transitions = tx
                    .execute(
                        SQL_ERASE_TRANSITION_LOG,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let remainder: i64 = tx
                    .query_row(SQL_COUNT_PRESENCE_TARGET, params![target], |row| row.get(0))
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                let erased = erasure_count(
                    attribution + stopped + hints + last_clients + destinations + transitions,
                )
                .map_err(presence_unavailable)?;
                let remainder = erasure_count(remainder).map_err(presence_unavailable)?;
                tx.commit()
                    .map_err(|error| presence_unavailable(error.to_string()))?;
                Ok(LocalErasurePass::Applied { erased, remainder })
            })
            .await
        }
    }
}
