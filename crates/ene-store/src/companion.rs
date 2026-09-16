use std::sync::Arc;
use std::sync::Mutex;

use ene_companion::{
    ActivityId, ActivityRepository, AppendHistoryCommand, CommandId, CompanionId,
    CompanionLifecycle, CompanionRepository, CompanionTechnicalError, HistoryAppendOutcome,
    HistoryMessage, HistoryRepository, HistoryRole, ManagementActivity, PresentationMark,
    RecordResumeActivityCommand, ReportStatus, ReportStatusTransition, TaskFact,
    UNDELIVERED_PAGE_MAX, UndeliveredCursor, UndeliveredId, UndeliveredPage, UndeliveredRef,
    UndeliveredRepository, UndeliveredSource, UndeliveredTechnicalError,
};
use ene_permission::CapabilityKind;
use ene_presence::{PresenceGeneration, PresenceState};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    HistoryRow, companion_unavailable, decode_history_message, decode_id, decode_lifecycle,
    decode_report_status, decode_u64, decode_undelivered_source, encode_id, encode_lifecycle,
    encode_presence_state, encode_report_status, encode_role, encode_round_intent, encode_u64,
    encode_undelivered_source, lock_shared, select_attribution, select_consent,
    undelivered_unavailable,
};
use crate::credential::SQL_SELECT_SET_REV;
use crate::run_blocking;

const SQL_FIND_COMPANION: &str = "SELECT companion_id FROM companion LIMIT 1";

const SQL_INSERT_COMPANION: &str =
    "INSERT INTO companion (companion_id, lifecycle, created_at) VALUES (?1, ?2, ?3)";

const SQL_SELECT_LIFECYCLE: &str = "SELECT lifecycle FROM companion WHERE companion_id = ?1";

/// The companion lifecycle read for the Task resume commit (AU17): the same
/// row the History appends compare, read inside the resume transaction so
/// the `Running` requirement linearizes with the revision forward.
pub(crate) const SQL_SELECT_COMPANION_LIFECYCLE: &str =
    "SELECT lifecycle FROM companion WHERE companion_id = ?1";

/// The Owner-message premise read for the Task resume commit (AU17):
/// role and companion of the relied message, without its body.
pub(crate) const SQL_SELECT_HISTORY_PREMISE: &str =
    "SELECT role, companion_id FROM history_message WHERE message_id = ?1";

const SQL_INSERT_ATTRIBUTION: &str = "INSERT INTO presence_attribution (companion_id, state, active_client, generation) VALUES (?1, ?2, ?3, ?4)";

const SQL_INSERT_HINT: &str = "INSERT INTO relocation_hint (companion_id, last_client, recovery_destination) VALUES (?1, NULL, NULL)";

const SQL_INSERT_HISTORY: &str = "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, at_utc, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)";

const SQL_SELECT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND (?2 IS NULL OR round_id = ?2) AND (?3 IS NULL OR at_utc >= ?3) ORDER BY rowid ASC LIMIT ?4";

const SQL_SELECT_ROUND_BY_WIRE: &str =
    "SELECT round_id FROM history_message WHERE companion_id = ?1 AND round_wire = ?2 LIMIT 1";

const SQL_SELECT_RECENT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 ORDER BY rowid DESC LIMIT ?2";

const SQL_SELECT_HISTORY_BY_COMMAND: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND command_id = ?2 ORDER BY rowid ASC LIMIT 1";

/// The single-message bounded read: `message_id` is the primary key, so the
/// lookup touches exactly the addressed row and never scans the table. The
/// companion column is appended after the shared [`HistoryRow`] column list
/// so one decoder serves every History read.
pub(crate) const SQL_SELECT_HISTORY_BY_MESSAGE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random, companion_id FROM history_message WHERE message_id = ?1";

/// Durable identity lookup for one Owner-message premise: the expected row
/// resolves by primary key, never by text or position.
pub(crate) const SQL_SELECT_OWNER_ROWID: &str =
    "SELECT rowid FROM history_message WHERE message_id = ?1";

/// Supersession probe for one reply premise: any accepted Owner row for
/// this companion past the expected rowid, newest or otherwise. Served by
/// the companion-plus-role covering index and stopping at the first hit,
/// so the common current case is one index step, never a History scan.
pub(crate) const SQL_EXISTS_NEWER_OWNER: &str = "SELECT 1 WHERE EXISTS (SELECT 1 FROM history_message WHERE companion_id = ?1 AND role = ?2 AND rowid > ?3 LIMIT 1)";

const SQL_INSERT_UNDELIVERED: &str = "INSERT INTO undelivered (undelivered_id, companion_id, source_kind, source_id, source_phase, status, round_id, presence_generation, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT (companion_id, source_kind, source_id, source_phase) DO NOTHING";

const SQL_SELECT_UNDELIVERED_STATUS: &str =
    "SELECT status FROM undelivered WHERE undelivered_id = ?1";

const SQL_UPDATE_UNDELIVERED_STATUS: &str =
    "UPDATE undelivered SET status = ?1 WHERE undelivered_id = ?2";

/// The unpresented row projection shared by the paged and exact-identity
/// reads, so both decode through [`RawUndelivered`] without drift.
const SQL_UNPRESENTED_COLUMNS: &str = "row_seq, undelivered_id, companion_id, source_kind, source_id, source_phase, status, round_id, presence_generation, created_at";

/// The bounded unpresented page: `Pending` and `PresentationUnknown` only,
/// keyset over the non-reused insertion sequence, with the pass upper bound
/// keeping rows registered while the pass runs out of it.
fn sql_select_unpresented() -> String {
    format!(
        "SELECT {SQL_UNPRESENTED_COLUMNS} FROM undelivered WHERE companion_id = ?1 AND status IN (?2, ?3) AND row_seq > ?4 AND row_seq <= ?5 ORDER BY row_seq ASC LIMIT ?6"
    )
}

/// Exact-identity read: one `IN` lookup over the requested ids, reordered to
/// the requested identity order. The requested count is already page-bounded,
/// so the placeholder list stays bounded. Every report status is returned;
/// the caller decides how to treat resolved-but-presented rows.
fn sql_select_undelivered_by_ids(count: usize) -> String {
    let placeholders: Vec<String> = (0..count).map(|index| format!("?{}", index + 2)).collect();
    format!(
        "SELECT {SQL_UNPRESENTED_COLUMNS} FROM undelivered WHERE companion_id = ?1 AND undelivered_id IN ({})",
        placeholders.join(", ")
    )
}

/// The insertion sequence in force, for the next pass. A read: it writes no
/// marker and advances no generation.
const SQL_SELECT_PASS_BOUND: &str = "SELECT COALESCE(MAX(row_seq), 0) FROM undelivered";

/// Bounded body projections. `substr(CAST(x AS BLOB), ...)` is byte-based,
/// so the cap bounds the bytes read out of SQLite instead of decoding the
/// whole body and truncating afterwards; the byte length of the full body is
/// returned alongside for the caller's `truncated` display fact.
const SQL_EXCERPT_HISTORY: &str = "SELECT length(CAST(body AS BLOB)), substr(CAST(body AS BLOB), 1, ?2) FROM history_message WHERE message_id = ?1";

const SQL_EXCERPT_TASK_REVISION: &str = "SELECT length(CAST(purpose_text AS BLOB)), substr(CAST(purpose_text AS BLOB), 1, ?3) FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_EXCERPT_RESULT: &str = "SELECT length(CAST(body AS BLOB)), substr(CAST(body AS BLOB), 1, ?2) FROM task_result WHERE result_id = ?1";

const SQL_EXCERPT_ATTEMPT: &str = "SELECT length(CAST(real_target AS BLOB)), substr(CAST(real_target AS BLOB), 1, ?2) FROM action_attempt WHERE attempt_id = ?1";

/// Byte-bounded excerpt of one undelivered source's canonical body.
///
/// This is the source owner's bounded projection, never a copy stored on the
/// `undelivered` row. `total_bytes` is the full body length so a caller can
/// report truncation and page the rest; `text` is cut on a UTF-8 character
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeliveredExcerpt {
    pub text: String,
    pub total_bytes: u64,
}

/// Registers one undelivered item inside the parent fact's transaction
/// (AU1a / AU1b).
///
/// The source key is the canonical identity and closed fact kind; the
/// `(companion_id, source_kind, source_id, source_phase)` uniqueness
/// constraint makes a repeated registration a no-op that keeps the existing
/// row. Any other failure (including a constraint violation that is not the
/// source-key one) is returned to the caller, which must roll the parent
/// fact back with it.
pub(crate) fn register_undelivered_tx(
    tx: &rusqlite::Transaction<'_>,
    companion_text: &str,
    undelivered_id: RawId,
    source: &UndeliveredSource,
    round_text: Option<&str>,
    generation_raw: Option<i64>,
    created_at: WallClockWithTz,
) -> Result<(), String> {
    let (kind, id, phase) = encode_undelivered_source(source);
    tx.execute(
        SQL_INSERT_UNDELIVERED,
        params![
            encode_id(undelivered_id),
            companion_text,
            kind,
            id,
            phase,
            encode_report_status(ReportStatus::Pending),
            round_text,
            generation_raw,
            created_at.to_rfc3339(),
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

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
    let at_utc = cmd.at.to_rfc3339_utc();
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
    let Some(current) = select_attribution(&tx, &companion_text).map_err(companion_unavailable)?
    else {
        return Err(companion_unavailable(String::from(
            "missing presence attribution",
        )));
    };
    if current.generation.as_u64() != cmd.expected_generation.as_u64() {
        return Ok((
            HistoryAppendOutcome::StaleExpected {
                current: current.generation,
            },
            None,
        ));
    }
    let generation_raw = encode_u64(current.generation.as_u64()).map_err(companion_unavailable)?;
    if let Some((expected_id, expected_rev)) = cmd.expected_consent.as_ref() {
        // History appends are dialogue turns: the premise names the dialogue
        // consent only, never a learning assignment.
        let current =
            select_consent(&tx, CapabilityKind::Dialogue).map_err(companion_unavailable)?;
        let current_matches = current.as_ref().is_some_and(|record| {
            record.id == *expected_id && record.rev.as_u64() == *expected_rev
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
    if let Some(expected) = cmd.expected_owner_message {
        // A newer accepted Owner input supersedes this turn's owner: the
        // reply would answer input the owner already moved past, in any
        // round. Committed owner rows are accepted inputs by construction
        // — declined intakes never write — so any newer owner rowid for
        // this companion proves supersession, inside this same
        // transaction as the insert it guards. A premise that cannot even
        // be read fails closed rather than adopting against the past.
        let expected_rowid: Option<i64> = tx
            .query_row(
                SQL_SELECT_OWNER_ROWID,
                params![encode_id(expected)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let superseded = match expected_rowid {
            None => true,
            Some(rowid) => tx
                .query_row(
                    SQL_EXISTS_NEWER_OWNER,
                    params![companion_text, encode_role(HistoryRole::Owner), rowid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?
                .is_some(),
        };
        if superseded {
            return Ok((HistoryAppendOutcome::StaleOwnerInput, None));
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
            at_utc,
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
        let undelivered = RawId::new();
        let source = UndeliveredSource::HistoryMessage(message);
        let created_at = WallClockWithTz::now();
        register_undelivered_tx(
            &tx,
            &companion_text,
            undelivered,
            &source,
            Some(&round_text),
            Some(generation_raw),
            created_at,
        )
        .map_err(companion_unavailable)?;
        registered = Some(UndeliveredRef {
            id: UndeliveredId::from_raw(undelivered),
            companion: cmd.companion,
            source,
            status: ReportStatus::Pending,
            created_at,
            round: Some(cmd.round),
            presence_generation: Some(current.generation),
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
            // The relocation hint row is seeded empty with the companion: it
            // records history only, and recovery writes are upserts that
            // never invent a client.
            tx.execute(SQL_INSERT_HINT, params![fresh_text])
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

impl Store {
    /// Synchronous [`HistoryRepository::append_message`] for the
    /// connection-ownership section (CCT §10.4).
    ///
    /// The Host's Client-dependent admission runs this inside
    /// `spawn_blocking` while the connection table verifies currentness, so a
    /// supersession that wins the section leaves no Owner row behind. The
    /// async repository wraps the same `append_history` body, so the two
    /// forms agree by construction.
    ///
    /// # Errors
    ///
    /// [`CompanionTechnicalError`] when the append transaction cannot
    /// commit.
    pub fn append_message_sync(
        &self,
        cmd: AppendHistoryCommand,
    ) -> Result<HistoryAppendOutcome, CompanionTechnicalError> {
        let (outcome, _) = append_history(&self.conn, &cmd, false)?;
        Ok(outcome)
    }

    /// Synchronous [`HistoryRepository::lookup_command`] for the
    /// connection-ownership section (CCT §10.4).
    ///
    /// A read-only point lookup on `(companion, command_id)`; the guarded
    /// append uses it to resolve a concurrent same-command commit without
    /// leaving the section.
    ///
    /// # Errors
    ///
    /// [`CompanionTechnicalError`] when the row cannot be read.
    pub fn lookup_command_sync(
        &self,
        companion: CompanionId,
        command: &CommandId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError> {
        let key = encode_id(companion.as_raw());
        let command_key = encode_id(command.0);
        let guard = lock_shared(&self.conn);
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
    }

    /// Resolves one stored round wire projection to its domain round.
    ///
    /// The round wire is Host-minted and stored with every appended message
    /// of that round, so a round stays addressable after a restart drops the
    /// transient in-memory wire map. An unknown projection answers `Ok(None)`;
    /// storage failures stay errors.
    pub async fn round_for_stored_wire(
        &self,
        companion: CompanionId,
        wire: &str,
    ) -> Result<Option<RawId>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let companion_text = encode_id(companion.as_raw());
        let wire = wire.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<String> = guard
                .query_row(
                    SQL_SELECT_ROUND_BY_WIRE,
                    params![companion_text, wire],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            found
                .map(|text| decode_id(&text).map_err(companion_unavailable))
                .transpose()
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
        self.hint_after_commit(
            run_blocking(move || append_history(&conn, &cmd, register_unpresented)).await,
        )
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

    async fn load_message(
        &self,
        message: RawId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let message_text = encode_id(message);
            let guard = lock_shared(&conn);
            // Point lookup on the `message_id` primary key. The companion
            // column rides the same row so the shared `HistoryRow` decoder
            // is reused; a companion that cannot be decoded is an unreadable
            // row, not a default.
            let found: Option<(HistoryRow, String)> = guard
                .query_row(
                    SQL_SELECT_HISTORY_BY_MESSAGE,
                    params![message_text],
                    |row| Ok((HistoryRow::from_row(row)?, row.get(14)?)),
                )
                .optional()
                .map_err(|error| companion_unavailable(error.to_string()))?;
            found
                .map(|(row, companion_text)| {
                    let companion = CompanionId::from_raw(
                        decode_id(&companion_text).map_err(companion_unavailable)?,
                    );
                    decode_history_message(companion, row).map_err(companion_unavailable)
                })
                .transpose()
        })
        .await
    }

    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        round: Option<RawId>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let round_text = round.map(encode_id);
            // The canonical UTC rendering orders and compares exactly like
            // the instant, unlike the offset-preserving display text.
            let since_text = since.map(|bound| bound.to_rfc3339_utc());
            // SQLite takes a signed limit; a larger request saturates instead
            // of failing, and zero asks for no rows.
            let cap = i64::try_from(limit).unwrap_or(i64::MAX);
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_SELECT_TIMELINE)
                .map_err(|error| companion_unavailable(error.to_string()))?;
            let rows = query
                .query_map(
                    params![key, round_text, since_text, cap],
                    HistoryRow::from_row,
                )
                .map_err(|error| companion_unavailable(error.to_string()))?;
            // The query already applied the round, since, and limit bounds,
            // so exactly the returned rows are decoded.
            let mut timeline = Vec::new();
            for row in rows {
                let row = row.map_err(|error| companion_unavailable(error.to_string()))?;
                timeline
                    .push(decode_history_message(companion, row).map_err(companion_unavailable)?);
            }
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

/// One stored `undelivered` row in column order.
struct RawUndelivered {
    row_seq: i64,
    undelivered_id: String,
    companion_id: String,
    source_kind: String,
    source_id: String,
    source_phase: String,
    status: String,
    round_id: Option<String>,
    presence_generation: Option<i64>,
    created_at: String,
}

impl RawUndelivered {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            row_seq: row.get(0)?,
            undelivered_id: row.get(1)?,
            companion_id: row.get(2)?,
            source_kind: row.get(3)?,
            source_id: row.get(4)?,
            source_phase: row.get(5)?,
            status: row.get(6)?,
            round_id: row.get(7)?,
            presence_generation: row.get(8)?,
            created_at: row.get(9)?,
        })
    }

    /// Decodes every part or fails closed: an unknown source kind / status, a
    /// malformed identity, a source whose owning task does not resolve, and
    /// a malformed timestamp are unreadable rows.
    fn decode(self, conn: &Connection) -> Result<UndeliveredRef, UndeliveredTechnicalError> {
        let companion =
            CompanionId::from_raw(decode_id(&self.companion_id).map_err(undelivered_unavailable)?);
        let source =
            decode_undelivered_source(conn, &self.source_kind, &self.source_id, &self.source_phase)
                .map_err(undelivered_unavailable)?;
        let round = self
            .round_id
            .as_deref()
            .map(decode_id)
            .transpose()
            .map_err(undelivered_unavailable)?;
        let presence_generation = self
            .presence_generation
            .map(decode_u64)
            .transpose()
            .map_err(undelivered_unavailable)?
            .map(PresenceGeneration::from_u64);
        Ok(UndeliveredRef {
            id: UndeliveredId::from_raw(
                decode_id(&self.undelivered_id).map_err(undelivered_unavailable)?,
            ),
            companion,
            source,
            status: decode_report_status(&self.status).map_err(undelivered_unavailable)?,
            created_at: WallClockWithTz::parse_rfc3339(&self.created_at)
                .map_err(|error| undelivered_unavailable(error.to_string()))?,
            round,
            presence_generation,
        })
    }
}

/// The insertion sequence in force; `0` on an empty table.
fn select_pass_bound(conn: &Connection) -> Result<u64, String> {
    let raw: i64 = conn
        .query_row(SQL_SELECT_PASS_BOUND, (), |row| row.get(0))
        .map_err(|error| error.to_string())?;
    decode_u64(raw)
}

impl Store {
    /// Synchronous report-status compare for a connection/receipt ownership
    /// section (CCT §10.4–10.5). Uses the same per-row transaction as the async
    /// repository method. The caller must retain ownership through this call;
    /// checking currentness before scheduling a later write is insufficient.
    ///
    /// # Errors
    ///
    /// Returns [`UndeliveredTechnicalError`] if the transaction cannot commit.
    pub fn compare_and_mark_reported_sync(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError> {
        compare_and_mark_reported(&self.conn, id, expected, mark)
    }
}

fn compare_and_mark_reported(
    conn: &Mutex<Connection>,
    id: UndeliveredId,
    expected: ReportStatus,
    mark: PresentationMark,
) -> Result<ReportStatusTransition, UndeliveredTechnicalError> {
    let key = encode_id(id.as_raw());
    let mut guard = lock_shared(conn);
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
    // `Presented` is absorbing: a duplicate ACK, a stale
    // not-presented receipt, and a race with another receipt all
    // write nothing instead of returning the row to `Pending`.
    if current == ReportStatus::Presented {
        return Ok(ReportStatusTransition::AlreadyPresented);
    }
    if current != expected {
        return Ok(ReportStatusTransition::StaleSource);
    }
    let (next, transition) = match (mark.presented, current) {
        (true, ReportStatus::Pending | ReportStatus::PresentationUnknown) => (
            ReportStatus::Presented,
            ReportStatusTransition::PendingToPresented,
        ),
        // A not-presented mark against a pending row is the
        // presentation start: the row stays re-presentable.
        (false, ReportStatus::Pending) => (
            ReportStatus::PresentationUnknown,
            ReportStatusTransition::MarkedPresentationUnknown,
        ),
        // A not-presented mark against an unknown row is a current
        // receipt that confirmed the item was not presented.
        (false, ReportStatus::PresentationUnknown) => (
            ReportStatus::Pending,
            ReportStatusTransition::FailedToPending,
        ),
        (_, ReportStatus::Presented) => {
            // Handled above; keeping the arm total without a write.
            return Ok(ReportStatusTransition::AlreadyPresented);
        }
    };
    tx.execute(
        SQL_UPDATE_UNDELIVERED_STATUS,
        params![encode_report_status(next), key],
    )
    .map_err(|error| undelivered_unavailable(error.to_string()))?;
    tx.commit()
        .map_err(|error| undelivered_unavailable(error.to_string()))?;
    Ok(transition)
}

impl UndeliveredRepository for Store {
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || compare_and_mark_reported(&conn, id, expected, mark)).await
    }

    async fn undelivered_pass_bound(&self) -> Result<u64, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            select_pass_bound(&guard).map_err(undelivered_unavailable)
        })
        .await
    }

    async fn list_unpresented(
        &self,
        companion: CompanionId,
        cursor: Option<UndeliveredCursor>,
        limit: u32,
    ) -> Result<UndeliveredPage, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            // The page bound is applied by SQL, not after decoding; the design
            // bound is 1..=50.
            let cap = i64::from(limit.clamp(1, UNDELIVERED_PAGE_MAX));
            let guard = lock_shared(&conn);
            // A fresh pass captures the insertion sequence in force; rows
            // registered while the pass runs stay above it and are returned by
            // the next pass.
            let upper = match cursor {
                Some(cursor) => cursor.pass_upper_bound(),
                None => select_pass_bound(&guard).map_err(undelivered_unavailable)?,
            };
            let after = cursor.map_or(0, UndeliveredCursor::after_seq);
            let after_raw = encode_u64(after).map_err(undelivered_unavailable)?;
            let upper_raw = encode_u64(upper).map_err(undelivered_unavailable)?;
            let mut statement = guard
                .prepare(&sql_select_unpresented())
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let rows = statement
                .query_map(
                    params![
                        key,
                        encode_report_status(ReportStatus::Pending),
                        encode_report_status(ReportStatus::PresentationUnknown),
                        after_raw,
                        upper_raw,
                        cap
                    ],
                    RawUndelivered::from_row,
                )
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let mut entries = Vec::new();
            let mut last_seq = after;
            for row in rows {
                let row = row.map_err(|error| undelivered_unavailable(error.to_string()))?;
                last_seq = decode_u64(row.row_seq).map_err(undelivered_unavailable)?;
                entries.push(row.decode(&guard)?);
            }
            let next = (entries.len() == usize::try_from(cap).unwrap_or(usize::MAX)
                && last_seq < upper)
                .then(|| UndeliveredCursor::begin(last_seq, upper));
            Ok(UndeliveredPage {
                entries,
                next,
                pass_upper_bound: upper,
            })
        })
        .await
    }

    async fn load_undelivered_by_ids(
        &self,
        companion: CompanionId,
        ids: &[UndeliveredId],
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError> {
        let requested: Vec<UndeliveredId> = ids
            .iter()
            .take(UNDELIVERED_PAGE_MAX as usize)
            .copied()
            .collect();
        if requested.is_empty() {
            return Ok(Vec::new());
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion.as_raw());
            let guard = lock_shared(&conn);
            let mut statement = guard
                .prepare(&sql_select_undelivered_by_ids(requested.len()))
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let mut values: Vec<String> = Vec::with_capacity(requested.len() + 1);
            values.push(key);
            for id in &requested {
                values.push(encode_id(id.as_raw()));
            }
            let rows = statement
                .query_map(
                    rusqlite::params_from_iter(values.iter()),
                    RawUndelivered::from_row,
                )
                .map_err(|error| undelivered_unavailable(error.to_string()))?;
            let mut found: std::collections::HashMap<UndeliveredId, UndeliveredRef> =
                std::collections::HashMap::with_capacity(requested.len());
            for row in rows {
                let row = row.map_err(|error| undelivered_unavailable(error.to_string()))?;
                let entry = row.decode(&guard)?;
                found.insert(entry.id, entry);
            }
            // Requested order, unresolved ids omitted: the caller compares
            // the length to detect a selection it cannot rehydrate.
            Ok(requested
                .iter()
                .filter_map(|id| found.get(id).cloned())
                .collect())
        })
        .await
    }
}

impl Store {
    /// Loads a byte-bounded excerpt of one undelivered source's canonical
    /// body.
    ///
    /// SELECT-only, and each source kind reads its owner row directly (the
    /// history message primary key, the Task revision snapshot, the result
    /// body, the Action attempt's recorded target). Nothing is copied into
    /// `undelivered`. `None` means this source kind carries no bounded body
    /// (delegation, terminal, activity) or the addressed row is gone; absence
    /// is reported, never defaulted to an empty success.
    pub async fn load_undelivered_excerpt(
        &self,
        source: UndeliveredSource,
        max_bytes: u32,
    ) -> Result<Option<UndeliveredExcerpt>, UndeliveredTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            // A zero cap would return the same page forever; the minimum
            // page is one byte.
            let cap = i64::from(max_bytes.max(1));
            let guard = lock_shared(&conn);
            let found: Option<(i64, Vec<u8>)> = match source {
                UndeliveredSource::HistoryMessage(message) => guard
                    .query_row(
                        SQL_EXCERPT_HISTORY,
                        params![encode_id(message), cap],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| undelivered_unavailable(error.to_string()))?,
                UndeliveredSource::TaskRecord {
                    fact: TaskFact::TaskRevision { task, revision },
                    ..
                } => guard
                    .query_row(
                        SQL_EXCERPT_TASK_REVISION,
                        params![
                            encode_id(task),
                            encode_u64(revision).map_err(undelivered_unavailable)?,
                            cap
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| undelivered_unavailable(error.to_string()))?,
                UndeliveredSource::TaskRecord {
                    fact: TaskFact::ResultRecorded(result) | TaskFact::ResultAdopted(result),
                    ..
                } => guard
                    .query_row(SQL_EXCERPT_RESULT, params![encode_id(result), cap], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .optional()
                    .map_err(|error| undelivered_unavailable(error.to_string()))?,
                UndeliveredSource::TaskRecord {
                    fact: TaskFact::ActionAttempt { attempt, .. },
                    ..
                } => guard
                    .query_row(
                        SQL_EXCERPT_ATTEMPT,
                        params![encode_id(attempt), cap],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| undelivered_unavailable(error.to_string()))?,
                UndeliveredSource::TaskRecord {
                    fact: TaskFact::Delegation(_) | TaskFact::Terminal { .. },
                    ..
                }
                | UndeliveredSource::ActivityRecord(_) => None,
            };
            let Some((total_raw, bytes)) = found else {
                return Ok(None);
            };
            let total_bytes = decode_u64(total_raw).map_err(undelivered_unavailable)?;
            let text = crate::codec::utf8_prefix(&bytes)
                .map_err(undelivered_unavailable)?
                .to_owned();
            Ok(Some(UndeliveredExcerpt { text, total_bytes }))
        })
        .await
    }
}

/// The stored activity kind this slice records; an unknown stored value is
/// an unreadable row and is rejected on read.
const ACTIVITY_KIND_RESUME_INSTRUCTION: &str = "resume_instruction";

const SQL_INSERT_ACTIVITY: &str = "INSERT INTO activity_record (activity_id, companion_id, kind, task_id, task_revision, purpose_adopted_revision, body, created_at, command_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT (command_id) DO NOTHING";

/// The single-record bounded read: `activity_id` is the primary key, so the
/// lookup touches exactly the addressed row and never scans the table.
const SQL_SELECT_ACTIVITY: &str = "SELECT companion_id, kind, task_id, task_revision, purpose_adopted_revision, body, created_at FROM activity_record WHERE activity_id = ?1";

const SQL_SELECT_ACTIVITY_BY_COMMAND: &str = "SELECT activity_id, companion_id, kind, task_id, task_revision, purpose_adopted_revision, body, created_at FROM activity_record WHERE command_id = ?1";

/// The activity premise read for the Task resume commit (AU17): the selected
/// Task ref and purpose the instruction was recorded for, without its body.
pub(crate) const SQL_SELECT_ACTIVITY_PREMISE: &str = "SELECT companion_id, kind, task_id, task_revision, purpose_adopted_revision FROM activity_record WHERE activity_id = ?1";

fn activity_unavailable(reason: impl core::fmt::Display) -> CompanionTechnicalError {
    CompanionTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

/// One decoded `activity_record` row: the stored columns without the
/// primary key, which the caller already holds.
struct StoredActivityRow {
    companion_text: String,
    kind_text: String,
    task_text: Option<String>,
    task_revision: Option<i64>,
    purpose_adopted_revision: Option<i64>,
    body: String,
    created_at: String,
}

fn decode_activity_row(
    activity: ActivityId,
    row: StoredActivityRow,
) -> Result<ManagementActivity, CompanionTechnicalError> {
    if row.kind_text != ACTIVITY_KIND_RESUME_INSTRUCTION {
        return Err(activity_unavailable("unknown activity record kind"));
    }
    let (Some(task_text), Some(task_revision), Some(purpose_adopted_revision)) = (
        row.task_text,
        row.task_revision,
        row.purpose_adopted_revision,
    ) else {
        return Err(activity_unavailable(
            "resume activity record missing its selected task",
        ));
    };
    let task = decode_id(&task_text).map_err(activity_unavailable)?;
    Ok(ManagementActivity {
        id: activity,
        companion: CompanionId::from_raw(
            decode_id(&row.companion_text).map_err(activity_unavailable)?,
        ),
        task: ene_task::TaskRef {
            task: ene_task::TaskId::from_raw(task),
            revision: ene_task::TaskRevision::from_u64(
                decode_u64(task_revision).map_err(activity_unavailable)?,
            ),
        },
        purpose: ene_task::TaskPurposeRef {
            task: ene_task::TaskId::from_raw(task),
            adopted_revision: ene_task::TaskRevision::from_u64(
                decode_u64(purpose_adopted_revision).map_err(activity_unavailable)?,
            ),
        },
        body: row.body,
        created_at: WallClockWithTz::parse_rfc3339(&row.created_at)
            .map_err(activity_unavailable)?,
    })
}

/// Records one resume-instruction activity inside its own short `Immediate`
/// transaction, synchronously on the caller's thread.
///
/// The async [`ActivityRepository`] method wraps this in `run_blocking`; the
/// Host's connection-ownership resume section (CCT §10.4) calls it directly
/// while holding the connection table, so the activity and the AU17 commit it
/// feeds share one supersession boundary.
fn record_resume_activity_locked(
    conn: &Mutex<Connection>,
    cmd: RecordResumeActivityCommand,
) -> Result<ActivityId, CompanionTechnicalError> {
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| activity_unavailable(error.to_string()))?;
    let command_text = encode_id(cmd.command);
    tx.execute(
        SQL_INSERT_ACTIVITY,
        params![
            encode_id(ActivityId::generate().as_raw()),
            encode_id(cmd.companion.as_raw()),
            ACTIVITY_KIND_RESUME_INSTRUCTION,
            encode_id(cmd.task.task.as_raw()),
            encode_u64(cmd.task.revision.as_u64()).map_err(activity_unavailable)?,
            encode_u64(cmd.purpose.adopted_revision.as_u64()).map_err(activity_unavailable)?,
            cmd.body,
            WallClockWithTz::now().to_rfc3339(),
            command_text,
        ],
    )
    .map_err(|error| activity_unavailable(error.to_string()))?;
    // The same epoch key always names the same activity: a retry
    // reads the winner back, and different content under one key
    // fails closed instead of recording a second row.
    let found: Option<(String, StoredActivityRow)> = tx
        .query_row(
            SQL_SELECT_ACTIVITY_BY_COMMAND,
            params![command_text],
            |row| {
                Ok((
                    row.get(0)?,
                    StoredActivityRow {
                        companion_text: row.get(1)?,
                        kind_text: row.get(2)?,
                        task_text: row.get(3)?,
                        task_revision: row.get(4)?,
                        purpose_adopted_revision: row.get(5)?,
                        body: row.get(6)?,
                        created_at: row.get(7)?,
                    },
                ))
            },
        )
        .optional()
        .map_err(|error| activity_unavailable(error.to_string()))?;
    let Some((activity_text, stored)) = found else {
        return Err(activity_unavailable(
            "resume activity record missing after insert",
        ));
    };
    let activity = ActivityId::from_raw(decode_id(&activity_text).map_err(activity_unavailable)?);
    let reread = decode_activity_row(activity, stored)?;
    if reread.companion != cmd.companion
        || reread.task != cmd.task
        || reread.purpose != cmd.purpose
        || reread.body != cmd.body
    {
        return Err(activity_unavailable(
            "resume activity command reuses a key with different content",
        ));
    }
    tx.commit()
        .map_err(|error| activity_unavailable(error.to_string()))?;
    Ok(activity)
}

impl Store {
    /// Records one resume-instruction activity synchronously.
    ///
    /// For callers that hold the connection table across the resume commit
    /// (CCT §10.4); every ordinary caller uses the async
    /// [`ActivityRepository::record_resume_activity`]. The row is written in
    /// its own short transaction exactly as the async path writes it.
    ///
    /// # Errors
    ///
    /// [`CompanionTechnicalError`] when the activity cannot be recorded or
    /// the command key is reused with different content.
    pub fn record_resume_activity_sync(
        &self,
        cmd: RecordResumeActivityCommand,
    ) -> Result<ActivityId, CompanionTechnicalError> {
        record_resume_activity_locked(&self.conn, cmd)
    }
}

impl ActivityRepository for Store {
    async fn record_resume_activity(
        &self,
        cmd: RecordResumeActivityCommand,
    ) -> Result<ActivityId, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || record_resume_activity_locked(&conn, cmd)).await
    }

    async fn load_activity(
        &self,
        activity: ActivityId,
    ) -> Result<Option<ManagementActivity>, CompanionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<StoredActivityRow> = guard
                .query_row(
                    SQL_SELECT_ACTIVITY,
                    params![encode_id(activity.as_raw())],
                    |row| {
                        Ok(StoredActivityRow {
                            companion_text: row.get(0)?,
                            kind_text: row.get(1)?,
                            task_text: row.get(2)?,
                            task_revision: row.get(3)?,
                            purpose_adopted_revision: row.get(4)?,
                            body: row.get(5)?,
                            created_at: row.get(6)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| activity_unavailable(error.to_string()))?;
            found
                .map(|row| decode_activity_row(activity, row))
                .transpose()
        })
        .await
    }
}
