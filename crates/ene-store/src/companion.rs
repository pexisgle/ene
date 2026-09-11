use std::sync::Arc;
use std::sync::Mutex;

use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    CompanionTechnicalError, HistoryAppendOutcome, HistoryMessage, HistoryRepository,
    PresentationMark, ReportStatus, ReportStatusTransition, UndeliveredRef, UndeliveredRepository,
    UndeliveredTechnicalError,
};
use ene_permission::CapabilityKind;
use ene_presence::{PresenceGeneration, PresenceState};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    HistoryRow, SQL_SELECT_ATTRIBUTION, SQL_SELECT_CONSENT, companion_unavailable,
    decode_history_message, decode_id, decode_lifecycle, decode_report_status, decode_u64,
    encode_id, encode_lifecycle, encode_presence_state, encode_report_status, encode_role,
    encode_round_intent, encode_u64, lock_shared, undelivered_unavailable,
};
use crate::credential::SQL_SELECT_SET_REV;
use crate::run_blocking;

const SQL_FIND_COMPANION: &str = "SELECT companion_id FROM companion LIMIT 1";

const SQL_INSERT_COMPANION: &str =
    "INSERT INTO companion (companion_id, lifecycle, created_at) VALUES (?1, ?2, ?3)";

const SQL_SELECT_LIFECYCLE: &str = "SELECT lifecycle FROM companion WHERE companion_id = ?1";

const SQL_INSERT_ATTRIBUTION: &str = "INSERT INTO presence_attribution (companion_id, state, active_client, generation) VALUES (?1, ?2, ?3, ?4)";

const SQL_INSERT_HISTORY: &str = "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

const SQL_SELECT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 ORDER BY rowid ASC";

const SQL_SELECT_RECENT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 ORDER BY rowid DESC LIMIT ?2";

const SQL_SELECT_HISTORY_BY_LOCAL_ID: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND local_id = ?2 ORDER BY rowid ASC LIMIT 1";

const SQL_SELECT_HISTORY_BY_COMMAND: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND command_id = ?2 ORDER BY rowid ASC LIMIT 1";

const SQL_FIND_HISTORY: &str = "SELECT 1 FROM history_message WHERE message_id = ?1";

const SQL_INSERT_UNDELIVERED: &str = "INSERT INTO undelivered (undelivered_id, companion_id, source_message, status, round_id, presence_generation, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_SELECT_UNDELIVERED_STATUS: &str =
    "SELECT status FROM undelivered WHERE undelivered_id = ?1";

const SQL_UPDATE_UNDELIVERED_STATUS: &str =
    "UPDATE undelivered SET status = ?1 WHERE undelivered_id = ?2";

const SQL_SELECT_PENDING: &str = "SELECT undelivered_id, companion_id, source_message, status, round_id, presence_generation FROM undelivered WHERE companion_id = ?1 AND status = ?2 ORDER BY rowid ASC";

/// Appends one history item, optionally registering an undelivered entry
/// for it in the same atomic section.
///
/// Both [`HistoryRepository::append_message`] and
/// [`HistoryRepository::append_reply_with_undelivered`] funnel through
/// here so the lifecycle read, the generation compare, the history insert,
/// and the optional undelivered insert share one `Immediate` transaction.
///
/// Durable replay rests on the client-minted `(companion, command_id)` key:
/// an exact request fingerprint answers
/// [`HistoryAppendOutcome::AlreadyCommittedAs`] without re-appending or
/// re-registering undelivered; a reused key with a different or
/// unreconstructable fingerprint answers
/// [`HistoryAppendOutcome::CommandConflict`] with no side effects. Round
/// identity and wire projection are the accepted result, not request
/// content, so they never decide conflict. The generation premise stays out
/// of the fingerprint (enforced separately above), so a retry under a newer
/// generation view still replays; `NULL` command ids carry no replay key
/// and never collide.
fn append_history(
    conn: &Mutex<Connection>,
    cmd: &AppendHistoryCommand,
    register_unpresented: bool,
) -> Result<(HistoryAppendOutcome, Option<UndeliveredRef>), CompanionTechnicalError> {
    let message = RawId::new();
    let message_text = encode_id(message);
    let companion_text = encode_id(cmd.companion.as_raw());
    let round_text = encode_id(cmd.round);
    let role_text = encode_role(cmd.role);
    let at_text = cmd.at.to_rfc3339();
    let undelivered = RawId::new();
    let undelivered_text = encode_id(undelivered);
    let now_text = WallClockWithTz::now().to_rfc3339();
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| companion_unavailable(error.to_string()))?;
    let lifecycle_text: Option<String> = tx
        .query_row(SQL_SELECT_LIFECYCLE, params![companion_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| companion_unavailable(error.to_string()))?;
    let lifecycle = match lifecycle_text {
        Some(text) => decode_lifecycle(&text).map_err(companion_unavailable)?,
        None => {
            return Ok((
                HistoryAppendOutcome::HeldByLifecycle {
                    lifecycle: CompanionLifecycle::Deleted,
                },
                None,
            ));
        }
    };
    if lifecycle != CompanionLifecycle::Running {
        return Ok((HistoryAppendOutcome::HeldByLifecycle { lifecycle }, None));
    }
    let stored_generation: Option<i64> = tx
        .query_row(SQL_SELECT_ATTRIBUTION, params![companion_text], |row| {
            row.get(2)
        })
        .optional()
        .map_err(|error| companion_unavailable(error.to_string()))?;
    let Some(generation_raw) = stored_generation else {
        return Err(companion_unavailable(String::from(
            "missing presence attribution",
        )));
    };
    let current_number = decode_u64(generation_raw).map_err(companion_unavailable)?;
    if current_number != cmd.expected_generation.as_u64() {
        return Ok((
            HistoryAppendOutcome::StaleExpected {
                current: PresenceGeneration::from_u64(current_number),
            },
            None,
        ));
    }
    if let Some((expected_id, expected_rev)) = cmd.expected_consent.as_ref() {
        // History appends are dialogue turns: the premise names the dialogue
        // consent only, never a learning assignment.
        let stored: Option<(String, i64)> = tx
            .query_row(
                SQL_SELECT_CONSENT,
                params![CapabilityKind::Dialogue.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let current_matches = stored.as_ref().is_some_and(|(id, rev)| {
            id == expected_id && decode_u64(*rev).is_ok_and(|value| value == *expected_rev)
        });
        if !current_matches {
            return Ok((HistoryAppendOutcome::StaleConsent, None));
        }
    }
    if let Some(expected) = cmd.expected_credential_set {
        // The text was scrubbed under this credential-set revision. A set
        // that moved past it may have registered a value still present in
        // the text, so nothing is written.
        let stored: i64 = tx
            .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let current = decode_u64(stored).map_err(companion_unavailable)?;
        if current != expected.as_u64() {
            return Ok((HistoryAppendOutcome::StaleCredentialSet, None));
        }
    }
    if let Some(command) = cmd.command_id {
        let command_text = encode_id(command.0);
        let existing: Option<HistoryRow> = tx
            .query_row(
                SQL_SELECT_HISTORY_BY_COMMAND,
                params![companion_text, command_text],
                HistoryRow::from_row,
            )
            .optional()
            .map_err(|error| companion_unavailable(error.to_string()))?;
        if let Some(row) = existing {
            let stored =
                decode_history_message(cmd.companion, row).map_err(companion_unavailable)?;
            let Some(stored_fingerprint) = stored.request_fingerprint() else {
                return Ok((HistoryAppendOutcome::CommandConflict, None));
            };
            let Some(incoming_fingerprint) = cmd.request_fingerprint() else {
                return Ok((HistoryAppendOutcome::CommandConflict, None));
            };
            if stored_fingerprint != incoming_fingerprint {
                return Ok((HistoryAppendOutcome::CommandConflict, None));
            }
            return Ok((
                HistoryAppendOutcome::AlreadyCommittedAs {
                    message: stored.id,
                    round: stored.round,
                },
                None,
            ));
        }
    }
    let command_text = cmd.command_id.map(|command| encode_id(command.0));
    let (client_counter, client_random) = match cmd.incarnation {
        Some((counter, random)) => (
            Some(encode_u64(counter).map_err(companion_unavailable)?),
            Some(encode_u64(random).map_err(companion_unavailable)?),
        ),
        None => (None, None),
    };
    let (intent_kind, intent_ref) = match cmd.round_intent.as_ref() {
        Some(intent) => {
            let (kind, reference) = encode_round_intent(intent);
            (Some(kind), reference.map(str::to_owned))
        }
        None => (None, None),
    };
    tx.execute(
        SQL_INSERT_HISTORY,
        params![
            message_text,
            companion_text,
            round_text,
            role_text,
            cmd.text,
            cmd.lang,
            at_text,
            generation_raw,
            command_text.as_deref(),
            cmd.local_id.as_deref(),
            cmd.round_wire.as_deref(),
            intent_kind,
            intent_ref,
            client_counter,
            client_random,
        ],
    )
    .map_err(|error| companion_unavailable(error.to_string()))?;
    let mut registered = None;
    if register_unpresented {
        tx.execute(
            SQL_INSERT_UNDELIVERED,
            params![
                undelivered_text,
                companion_text,
                message_text,
                encode_report_status(ReportStatus::Pending),
                round_text,
                generation_raw,
                now_text
            ],
        )
        .map_err(|error| companion_unavailable(error.to_string()))?;
        registered = Some(UndeliveredRef {
            id: undelivered,
            companion: cmd.companion,
            source_message: message,
            status: ReportStatus::Pending,
            round: cmd.round,
            presence_generation: PresenceGeneration::from_u64(current_number),
        });
    }
    tx.commit()
        .map_err(|error| companion_unavailable(error.to_string()))?;
    Ok((HistoryAppendOutcome::CommittedAs { message }, registered))
}

impl CompanionRepository for Store {
    async fn ensure_running_companion(&self) -> Result<CompanionId, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let now_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let found: Option<String> = tx
                .query_row(SQL_FIND_COMPANION, (), |row| row.get(0))
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            if let Some(existing) = found {
                let raw = decode_id(&existing).map_err(companion_unavailable)?;
                return Ok(CompanionId::from_raw(raw));
            }
            let fresh = RawId::new();
            let fresh_text = encode_id(fresh);
            tx.execute(
                SQL_INSERT_COMPANION,
                params![
                    fresh_text,
                    encode_lifecycle(CompanionLifecycle::Running),
                    now_text
                ],
            )
            .map_err(|error| companion_unavailable(error.to_string()))?;
            // The presence seed rides alongside the companion seed so the first
            // generation compare has a current fact to compare against.
            tx.execute(
                SQL_INSERT_ATTRIBUTION,
                params![
                    fresh_text,
                    encode_presence_state(PresenceState::NoActive),
                    Option::<String>::None,
                    0_i64
                ],
            )
            .map_err(|error| companion_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            Ok(CompanionId::from_raw(fresh))
        })
        .await
    }

    async fn load_lifecycle(
        &self,
        companion: CompanionId,
    ) -> Result<Option<CompanionLifecycle>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let guard = lock_shared(&conn);
            let found: Option<String> = guard
                .query_row(SQL_SELECT_LIFECYCLE, params![key], |row| row.get(0))
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            match found {
                Some(text) => {
                    let lifecycle = decode_lifecycle(&text).map_err(companion_unavailable)?;
                    Ok(Some(lifecycle))
                }
                None => Ok(None),
            }
        })
        .await
    }
}

impl HistoryRepository for Store {
    async fn append_message(
        &self,
        cmd: AppendHistoryCommand,
    ) -> Result<HistoryAppendOutcome, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let (outcome, _) = append_history(&conn, &cmd, false)?;
            Ok(outcome)
        })
        .await
    }

    async fn append_reply_with_undelivered(
        &self,
        cmd: AppendHistoryCommand,
        register_unpresented: bool,
    ) -> Result<(HistoryAppendOutcome, Option<UndeliveredRef>), CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || append_history(&conn, &cmd, register_unpresented)).await
    }

    async fn lookup_local_id(
        &self,
        companion: CompanionId,
        local_id: &str,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let local_id = local_id.to_owned();
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let guard = lock_shared(&conn);
            // Correspondence lookup for matching an input to its ack; durable
            // replay keys on `command_id` instead (see `lookup_command`).
            let found: Option<HistoryRow> = guard
                .query_row(
                    SQL_SELECT_HISTORY_BY_LOCAL_ID,
                    params![key, local_id],
                    HistoryRow::from_row,
                )
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            match found {
                Some(row) => {
                    let message =
                        decode_history_message(companion, row).map_err(companion_unavailable)?;
                    Ok(Some(message))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn lookup_command(
        &self,
        companion: CompanionId,
        command: &CommandId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let command = *command;
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let command_key = encode_id(command.0);
            let guard = lock_shared(&conn);
            // Durable replay lookup on the `(companion, command_id)` key; `NULL`
            // command ids never match.
            let found: Option<HistoryRow> = guard
                .query_row(
                    SQL_SELECT_HISTORY_BY_COMMAND,
                    params![key, command_key],
                    HistoryRow::from_row,
                )
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            match found {
                Some(row) => {
                    let message =
                        decode_history_message(companion, row).map_err(companion_unavailable)?;
                    Ok(Some(message))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_SELECT_TIMELINE)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let rows = query
                .query_map(params![key], HistoryRow::from_row)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let mut timeline = Vec::new();
            for row in rows {
                let row = row.map_err(|error| companion_unavailable(error.to_string()))?;
                let message =
                    decode_history_message(companion, row).map_err(companion_unavailable)?;
                if let Some(lower) = since
                    && message.at.as_datetime() < lower.as_datetime()
                {
                    continue;
                }
                timeline.push(message);
            }
            let cap = match usize::try_from(limit) {
                Ok(value) => value,
                Err(_) => usize::MAX,
            };
            timeline.truncate(cap);
            Ok(timeline)
        })
        .await
    }

    async fn load_recent_timeline(
        &self,
        companion: CompanionId,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let cap = encode_u64(limit).map_err(companion_unavailable)?;
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_SELECT_RECENT_TIMELINE)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let rows = query
                .query_map(params![key, cap], HistoryRow::from_row)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let mut timeline = Vec::new();
            for row in rows {
                let row = row.map_err(|error| companion_unavailable(error.to_string()))?;
                let message =
                    decode_history_message(companion, row).map_err(companion_unavailable)?;
                timeline.push(message);
            }
            // The SQL walk is newest-first so the cap keeps the newest items;
            // the caller receives them oldest-first like [`Self::load_timeline`].
            timeline.reverse();
            Ok(timeline)
        })
        .await
    }
}

impl UndeliveredRepository for Store {
    async fn register_if_parent_durable(
        &self,
        entry: UndeliveredRef,
    ) -> Result<bool, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let id_text = encode_id(entry.id);
            let companion_text = encode_id(entry.companion.as_raw());
            let source_text = encode_id(entry.source_message);
            let round_text = encode_id(entry.round);
            let status_text = encode_report_status(entry.status);
            let generation_raw =
                encode_u64(entry.presence_generation.as_u64()).map_err(undelivered_unavailable)?;
            let now_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let parent: Option<i64> = tx
                .query_row(SQL_FIND_HISTORY, params![source_text], |row| row.get(0))
                .optional()
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            if parent.is_none() {
                return Ok(false);
            }
            tx.execute(
                SQL_INSERT_UNDELIVERED,
                params![
                    id_text,
                    companion_text,
                    source_text,
                    status_text,
                    round_text,
                    generation_raw,
                    now_text
                ],
            )
            .map_err(|error| undelivered_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            Ok(true)
        })
        .await
    }

    async fn compare_and_mark_reported(
        &self,
        id: RawId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(id);
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let found: Option<String> = tx
                .query_row(SQL_SELECT_UNDELIVERED_STATUS, params![key], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let Some(status_text) = found else {
                return Ok(ReportStatusTransition::StaleSource);
            };
            let current = decode_report_status(&status_text).map_err(undelivered_unavailable)?;
            // `PresentationUnknown` is sticky: no transition leaves it, so a
            // compare that lands there is stale by definition.
            if current != expected || current == ReportStatus::PresentationUnknown {
                return Ok(ReportStatusTransition::StaleSource);
            }
            let (next, transition) = if mark.presented {
                (
                    ReportStatus::Presented,
                    ReportStatusTransition::PendingToPresented,
                )
            } else {
                (
                    ReportStatus::PresentationUnknown,
                    ReportStatusTransition::MarkedPresentationUnknown,
                )
            };
            tx.execute(
                SQL_UPDATE_UNDELIVERED_STATUS,
                params![encode_report_status(next), key],
            )
            .map_err(|error| undelivered_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            Ok(transition)
        })
        .await
    }

    async fn list_pending(
        &self,
        companion: CompanionId,
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_SELECT_PENDING)
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let rows = query
                .query_map(
                    params![key, encode_report_status(ReportStatus::Pending)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let mut pending = Vec::new();
            for row in rows {
                let (id_text, companion_text, source_text, status_text, round_text, generation_raw) =
                    row.map_err(|error| undelivered_unavailable(error.to_string()))?;
                pending.push(UndeliveredRef {
                    id: decode_id(&id_text).map_err(undelivered_unavailable)?,
                    companion: CompanionId::from_raw(
                        decode_id(&companion_text).map_err(undelivered_unavailable)?,
                    ),
                    source_message: decode_id(&source_text).map_err(undelivered_unavailable)?,
                    status: decode_report_status(&status_text).map_err(undelivered_unavailable)?,
                    round: decode_id(&round_text).map_err(undelivered_unavailable)?,
                    presence_generation: PresenceGeneration::from_u64(
                        decode_u64(generation_raw).map_err(undelivered_unavailable)?,
                    ),
                });
            }
            Ok(pending)
        })
        .await
    }
}
