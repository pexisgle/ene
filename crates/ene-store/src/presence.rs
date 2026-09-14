use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceGeneration, PresenceRepository, PresenceState,
    PresenceTechnicalError, ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    encode_id, encode_move_reason, encode_presence_state, encode_u64, lock_shared,
    presence_unavailable, select_attribution,
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
/// awaited across the section.
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
        let Some(next_generation) = current.generation.checked_next() else {
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
        // generation moves; anything else reads back unchanged.
        if current.generation != transitioning_generation
            || current.state != PresenceState::InTransition
        {
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

impl PresenceRepository for Store {
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let store = self.clone();
        run_blocking(move || store.load_attribution_sync(companion)).await
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
}
