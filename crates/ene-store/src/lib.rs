//! SQLite-backed durable repositories for the Stage 2 owners.
//!
//! [`Store`] owns one `rusqlite::Connection` and implements every repository
//! contract owned elsewhere: [`PresenceRepository`], [`CompanionRepository`],
//! [`HistoryRepository`], [`UndeliveredRepository`], [`ConsentRepository`],
//! [`CredentialRefRepository`], and [`UsageRepository`]. Owners never depend
//! on this crate; they program against their own traits.
//!
//! Concurrency shape: the connection is `Send` but not `Sync`, so a
//! `std::sync::Mutex` shares it across callers. Each repository method locks,
//! runs one short [`TransactionBehavior::Immediate`] transaction (or one plain
//! statement for pure loads), drops the guard, and only then returns. The
//! guard and any transaction never cross an `.await`: methods perform no
//! awaits while locked, verified by inspection. Values that cross the
//! boundary are bound parameters, never interpolated into SQL text.

use std::path::Path;
use std::sync::Mutex;
use std::sync::MutexGuard;

use ene_companion::{
    AppendHistoryCommand, CompanionId, CompanionLifecycle, CompanionRepository,
    CompanionTechnicalError, HistoryAppendOutcome, HistoryMessage, HistoryRepository, HistoryRole,
    PresentationMark, ReportStatus, ReportStatusTransition, UndeliveredRef, UndeliveredRepository,
    UndeliveredTechnicalError,
};
use ene_credential::{CredentialRef, CredentialRefRepository, CredentialTechnicalError};
use ene_inference::{InferenceTechnicalError, UsageFact, UsageRepository, UsageSource};
use ene_permission::{ConsentRecord, ConsentRepository, ConsentRevision, PermissionTechnicalError};
use ene_presence::{
    ClientId, LiveReachabilityRef, MoveDecision, PresenceAttribution, PresenceCheckRef,
    PresenceGeneration, PresenceRepository, PresenceState, PresenceTechnicalError, ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Failures opening or migrating the SQLite backing file.
///
/// Messages carry the short backend cause only. Paths are non-secret but are
/// kept out of messages for operational brevity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// Opening the database file failed.
    #[error("store open failed: {0}")]
    OpenFailed(String),
    /// Schema migration failed.
    #[error("store migration failed: {0}")]
    MigrationFailed(String),
}

/// SQLite-backed host for every Stage 2 repository contract.
///
/// The single `rusqlite::Connection` is `Send` but not `Sync`; the mutex
/// shares it across the repository implementations. Critical sections are
/// short and synchronous: each method locks, runs one
/// [`TransactionBehavior::Immediate`] transaction (or one plain statement for
/// pure loads), drops the guard, and only then returns, so the guard and any
/// transaction never cross an `.await` (verified by inspection: repository
/// bodies contain no awaits while locked).
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) the file-backed store and runs migrations.
    ///
    /// Reopening an existing file is idempotent: the schema setup and the
    /// running-companion seed tolerate an already-migrated database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`] when the file cannot be opened and
    /// [`StoreError::MigrationFailed`] when the schema cannot be prepared.
    #[expect(
        clippy::unused_async,
        reason = "requested as async; open runs synchronously with no await"
    )]
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "requested as async; open runs synchronously with no await"
    )]
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Opens an in-memory store and runs migrations, for tests.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`] when the database cannot be created
    /// and [`StoreError::MigrationFailed`] when the schema cannot be prepared.
    #[expect(
        clippy::unused_async,
        reason = "requested as async; open runs synchronously with no await"
    )]
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "requested as async; open runs synchronously with no await"
    )]
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Appends one history item, optionally registering an undelivered entry
    /// for it in the same atomic section.
    ///
    /// Both [`HistoryRepository::append_message`] and
    /// [`HistoryRepository::append_reply_with_undelivered`] funnel through
    /// here so the lifecycle read, the generation compare, the history insert,
    /// and the optional undelivered insert share one `Immediate` transaction.
    fn append_history(
        &self,
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
        let mut guard = lock_shared(&self.conn);
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
                generation_raw
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
}

/// Forward-only schema setup.
///
/// The `_schema_version` singleton records the applied version. Setup runs
/// `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` statements and
/// then bumps the singleton, so reopening a migrated file changes nothing.
/// No other tables exist here: device, hint, settings, provider, and
/// consent-history storage are explicitly deferred.
mod migrate {
    use rusqlite::{Connection, OptionalExtension};

    /// Schema version applied by [`run`](run).
    const CURRENT_VERSION: u64 = 1;

    /// Forward-only schema: tables first, then the supporting indexes.
    const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS companion (
    companion_id TEXT PRIMARY KEY,
    lifecycle TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS history_message (
    message_id TEXT PRIMARY KEY,
    companion_id TEXT NOT NULL,
    round_id TEXT NOT NULL,
    role TEXT NOT NULL,
    body TEXT NOT NULL,
    lang TEXT NOT NULL,
    at TEXT NOT NULL,
    presence_generation INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS undelivered (
    undelivered_id TEXT PRIMARY KEY,
    companion_id TEXT NOT NULL,
    source_message TEXT NOT NULL,
    status TEXT NOT NULL,
    round_id TEXT NOT NULL,
    presence_generation INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS presence_attribution (
    companion_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    active_client TEXT,
    generation INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS presence_transition_log (
    transition_seq INTEGER PRIMARY KEY AUTOINCREMENT,
    companion_id TEXT NOT NULL,
    old_state TEXT NOT NULL,
    new_state TEXT NOT NULL,
    old_gen INTEGER NOT NULL,
    new_gen INTEGER NOT NULL,
    reason TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS consent_record (
    id TEXT PRIMARY KEY,
    rev INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    credential_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS credential_ref (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    UNIQUE (provider, label)
);
CREATE TABLE IF NOT EXISTS usage_fact (
    ticket TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    source TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_history_message_companion ON history_message (companion_id);
CREATE INDEX IF NOT EXISTS idx_history_message_round ON history_message (round_id);
CREATE INDEX IF NOT EXISTS idx_undelivered_companion_status ON undelivered (companion_id, status);
";

    /// Creates or upgrades the schema on an open connection.
    ///
    /// Idempotent: rerunning on a migrated database changes nothing. Rejects
    /// a database newer than this binary understands instead of guessing.
    pub(super) fn run(conn: &Connection) -> Result<(), String> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS _schema_version (version INTEGER NOT NULL)")
            .map_err(|error| error.to_string())?;
        let stored: Option<i64> = conn
            .query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| error.to_string())?;
        let stored_version = match stored {
            Some(raw) => {
                u64::try_from(raw).map_err(|_| String::from("schema version out of range"))?
            }
            None => 0,
        };
        if stored_version > CURRENT_VERSION {
            return Err(String::from("schema version newer than supported"));
        }
        conn.execute_batch(SCHEMA)
            .map_err(|error| error.to_string())?;
        let current = i64::try_from(CURRENT_VERSION)
            .map_err(|_| String::from("schema version out of range"))?;
        if stored.is_none() {
            conn.execute(
                "INSERT INTO _schema_version (version) VALUES (?1)",
                rusqlite::params![current],
            )
            .map_err(|error| error.to_string())?;
        } else if stored_version < CURRENT_VERSION {
            conn.execute(
                "UPDATE _schema_version SET version = ?1",
                rusqlite::params![current],
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

const SQL_FIND_COMPANION: &str = "SELECT companion_id FROM companion LIMIT 1";
const SQL_INSERT_COMPANION: &str =
    "INSERT INTO companion (companion_id, lifecycle, created_at) VALUES (?1, ?2, ?3)";
const SQL_SELECT_LIFECYCLE: &str = "SELECT lifecycle FROM companion WHERE companion_id = ?1";
const SQL_INSERT_ATTRIBUTION: &str = "INSERT INTO presence_attribution (companion_id, state, active_client, generation) VALUES (?1, ?2, ?3, ?4)";
const SQL_SELECT_ATTRIBUTION: &str =
    "SELECT state, active_client, generation FROM presence_attribution WHERE companion_id = ?1";
const SQL_UPDATE_ATTRIBUTION: &str = "UPDATE presence_attribution SET state = ?1, active_client = ?2, generation = ?3 WHERE companion_id = ?4";
const SQL_INSERT_TRANSITION: &str = "INSERT INTO presence_transition_log (companion_id, old_state, new_state, old_gen, new_gen, reason, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
const SQL_INSERT_HISTORY: &str = "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, presence_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
const SQL_SELECT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation FROM history_message WHERE companion_id = ?1 ORDER BY rowid ASC";
const SQL_FIND_HISTORY: &str = "SELECT 1 FROM history_message WHERE message_id = ?1";
const SQL_INSERT_UNDELIVERED: &str = "INSERT INTO undelivered (undelivered_id, companion_id, source_message, status, round_id, presence_generation, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
const SQL_SELECT_UNDELIVERED_STATUS: &str =
    "SELECT status FROM undelivered WHERE undelivered_id = ?1";
const SQL_UPDATE_UNDELIVERED_STATUS: &str =
    "UPDATE undelivered SET status = ?1 WHERE undelivered_id = ?2";
const SQL_SELECT_PENDING: &str = "SELECT undelivered_id, companion_id, source_message, status, round_id, presence_generation FROM undelivered WHERE companion_id = ?1 AND status = ?2 ORDER BY rowid ASC";
const SQL_SELECT_CONSENT: &str =
    "SELECT id, rev, provider, model, credential_id FROM consent_record LIMIT 1";
const SQL_DELETE_CONSENT: &str = "DELETE FROM consent_record";
const SQL_INSERT_CONSENT: &str = "INSERT INTO consent_record (id, rev, provider, model, credential_id) VALUES (?1, ?2, ?3, ?4, ?5)";
const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";
const SQL_SELECT_CREDENTIAL: &str =
    "SELECT id, provider, label FROM credential_ref WHERE provider = ?1 AND label = ?2";
const SQL_LIST_CREDENTIALS: &str = "SELECT id, provider, label FROM credential_ref ORDER BY id ASC";
const SQL_INSERT_USAGE: &str = "INSERT INTO usage_fact (ticket, provider, model, input_tokens, output_tokens, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

/// Locks the shared connection, recovering from poisoning.
///
/// Poisoning only follows a panic inside a critical section; sections here
/// perform no panicking work while holding the guard, so recovery preserves
/// the committed state. Recovery (rather than erroring) also keeps lock
/// handling out of every repository's error vocabulary.
fn lock_shared(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    match conn.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Encodes an identity as lowercase hyphenated text for `TEXT` columns.
fn encode_id(id: RawId) -> String {
    id.as_uuid().as_hyphenated().to_string()
}

/// Decodes identity text written by [`encode_id`].
///
/// The `uuid` crate is not a direct dependency, so parsing goes through
/// [`str::parse`]: the target type is inferred from [`RawId::from_uuid`].
fn decode_id(text: &str) -> Result<RawId, String> {
    let parsed = text
        .parse()
        .map_err(|_| String::from("malformed identity text"))?;
    Ok(RawId::from_uuid(parsed))
}

/// Encodes a `u64` count for an `INTEGER` column.
fn encode_u64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| String::from("count out of range"))
}

/// Decodes an `INTEGER` column back to a `u64` count.
fn decode_u64(raw: i64) -> Result<u64, String> {
    u64::try_from(raw).map_err(|_| String::from("count out of range"))
}

/// Encodes an optional `u64` count, preserving unknown as `NULL` (never zero).
fn encode_optional_count(value: Option<u64>) -> Result<Option<i64>, String> {
    match value {
        Some(number) => Ok(Some(encode_u64(number)?)),
        None => Ok(None),
    }
}

fn encode_lifecycle(lifecycle: CompanionLifecycle) -> &'static str {
    match lifecycle {
        CompanionLifecycle::Running => "running",
        CompanionLifecycle::Stopped => "stopped",
        CompanionLifecycle::Deleted => "deleted",
    }
}

fn decode_lifecycle(text: &str) -> Result<CompanionLifecycle, String> {
    match text {
        "running" => Ok(CompanionLifecycle::Running),
        "stopped" => Ok(CompanionLifecycle::Stopped),
        "deleted" => Ok(CompanionLifecycle::Deleted),
        _ => Err(String::from("unknown companion lifecycle")),
    }
}

fn encode_role(role: HistoryRole) -> &'static str {
    match role {
        HistoryRole::Owner => "owner",
        HistoryRole::Companion => "companion",
    }
}

fn decode_role(text: &str) -> Result<HistoryRole, String> {
    match text {
        "owner" => Ok(HistoryRole::Owner),
        "companion" => Ok(HistoryRole::Companion),
        _ => Err(String::from("unknown history role")),
    }
}

fn encode_presence_state(state: PresenceState) -> &'static str {
    match state {
        PresenceState::Present => "present",
        PresenceState::NoActive => "no_active",
        PresenceState::InTransition => "in_transition",
        PresenceState::Stopped => "stopped",
        PresenceState::RecoveryWait => "recovery_wait",
    }
}

fn decode_presence_state(text: &str) -> Result<PresenceState, String> {
    match text {
        "present" => Ok(PresenceState::Present),
        "no_active" => Ok(PresenceState::NoActive),
        "in_transition" => Ok(PresenceState::InTransition),
        "stopped" => Ok(PresenceState::Stopped),
        "recovery_wait" => Ok(PresenceState::RecoveryWait),
        _ => Err(String::from("unknown presence state")),
    }
}

fn encode_report_status(status: ReportStatus) -> &'static str {
    match status {
        ReportStatus::Pending => "pending",
        ReportStatus::Presented => "presented",
        ReportStatus::PresentationUnknown => "presentation_unknown",
    }
}

fn decode_report_status(text: &str) -> Result<ReportStatus, String> {
    match text {
        "pending" => Ok(ReportStatus::Pending),
        "presented" => Ok(ReportStatus::Presented),
        "presentation_unknown" => Ok(ReportStatus::PresentationUnknown),
        _ => Err(String::from("unknown report status")),
    }
}

fn encode_usage_source(source: UsageSource) -> &'static str {
    match source {
        UsageSource::Reported => "reported",
        UsageSource::Estimated => "estimated",
        UsageSource::Unknown => "unknown",
    }
}

fn encode_move_reason(reason: ThinMoveReason) -> &'static str {
    match reason {
        ThinMoveReason::InitialAttach => "initial_attach",
        ThinMoveReason::DisconnectObserved => "disconnect_observed",
        ThinMoveReason::RestartRecovery => "restart_recovery",
    }
}

fn presence_unavailable(reason: String) -> PresenceTechnicalError {
    PresenceTechnicalError::StorageUnavailable { reason }
}

fn companion_unavailable(reason: String) -> CompanionTechnicalError {
    CompanionTechnicalError::StorageUnavailable { reason }
}

fn undelivered_unavailable(reason: String) -> UndeliveredTechnicalError {
    UndeliveredTechnicalError::StorageUnavailable { reason }
}

fn permission_unavailable(reason: String) -> PermissionTechnicalError {
    PermissionTechnicalError::StorageUnavailable { reason }
}

fn credential_unavailable(reason: String) -> CredentialTechnicalError {
    CredentialTechnicalError::StorageUnavailable { reason }
}

fn inference_unavailable(reason: String) -> InferenceTechnicalError {
    InferenceTechnicalError::StorageUnavailable { reason }
}

/// Reads one attribution row into its domain fact.
fn decode_attribution(
    companion: RawId,
    state_text: &str,
    active_text: Option<&str>,
    generation_raw: i64,
) -> Result<PresenceAttribution, String> {
    let state = decode_presence_state(state_text)?;
    let mut active_client = None;
    if let Some(text) = active_text {
        active_client = Some(ClientId::from_raw(decode_id(text)?));
    }
    let generation = PresenceGeneration::from_u64(decode_u64(generation_raw)?);
    Ok(PresenceAttribution {
        companion,
        state,
        active_client,
        generation,
    })
}

impl PresenceRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let key = encode_id(companion);
        let guard = lock_shared(&self.conn);
        let found: Option<(String, Option<String>, i64)> = guard
            .query_row(SQL_SELECT_ATTRIBUTION, params![key], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        match found {
            Some((state_text, active_text, generation_raw)) => {
                let fact = decode_attribution(
                    companion,
                    &state_text,
                    active_text.as_deref(),
                    generation_raw,
                )
                .map_err(presence_unavailable)?;
                Ok(Some(fact))
            }
            None => Ok(None),
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn compare_and_begin_transition(
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
        let found: Option<(String, Option<String>, i64)> = tx
            .query_row(SQL_SELECT_ATTRIBUTION, params![key], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        let Some((state_text, active_text, generation_raw)) = found else {
            return Ok(MoveDecision::DeniedByConstraint {
                reason: String::from("unknown companion"),
            });
        };
        let current = decode_attribution(
            companion,
            &state_text,
            active_text.as_deref(),
            generation_raw,
        )
        .map_err(presence_unavailable)?;
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
                generation_raw,
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

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn confirm_transition(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<PresenceAttribution, PresenceTechnicalError> {
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
        let found: Option<(String, Option<String>, i64)> = tx
            .query_row(SQL_SELECT_ATTRIBUTION, params![key], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        let Some((state_text, active_text, generation_raw)) = found else {
            return Err(presence_unavailable(String::from(
                "missing presence attribution",
            )));
        };
        let current = decode_attribution(
            companion,
            &state_text,
            active_text.as_deref(),
            generation_raw,
        )
        .map_err(presence_unavailable)?;
        // Idempotent: only an `InTransition` row at the transitioning
        // generation moves; anything else reads back unchanged.
        if current.generation != transitioning_generation
            || current.state != PresenceState::InTransition
        {
            return Ok(current);
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
        tx.execute(
            SQL_UPDATE_ATTRIBUTION,
            params![
                encode_presence_state(target_state),
                target_text,
                generation_raw,
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
                generation_raw,
                generation_raw,
                confirm_reason,
                now_text
            ],
        )
        .map_err(|error| presence_unavailable(error.to_string()))?;
        tx.commit()
            .map_err(|error| presence_unavailable(error.to_string()))?;
        Ok(PresenceAttribution {
            companion,
            state: target_state,
            active_client: target_client,
            generation: transitioning_generation,
        })
    }
}

impl CompanionRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn ensure_running_companion(&self) -> Result<CompanionId, CompanionTechnicalError> {
        let now_text = WallClockWithTz::now().to_rfc3339();
        let mut guard = lock_shared(&self.conn);
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
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn load_lifecycle(
        &self,
        companion: CompanionId,
    ) -> Result<Option<CompanionLifecycle>, CompanionTechnicalError> {
        let key = encode_id(companion.as_raw());
        let guard = lock_shared(&self.conn);
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
    }
}

impl HistoryRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn append_message(
        &self,
        cmd: AppendHistoryCommand,
    ) -> Result<HistoryAppendOutcome, CompanionTechnicalError> {
        let (outcome, _) = self.append_history(&cmd, false)?;
        Ok(outcome)
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn append_reply_with_undelivered(
        &self,
        cmd: AppendHistoryCommand,
        register_unpresented: bool,
    ) -> Result<(HistoryAppendOutcome, Option<UndeliveredRef>), CompanionTechnicalError> {
        self.append_history(&cmd, register_unpresented)
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError> {
        let key = encode_id(companion.as_raw());
        let guard = lock_shared(&self.conn);
        let mut query = guard
            .prepare(SQL_SELECT_TIMELINE)
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let rows = query
            .query_map(params![key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let mut timeline = Vec::new();
        for row in rows {
            let (message_text, round_text, role_text, body, lang, at_text, generation_raw) =
                row.map_err(|error| companion_unavailable(error.to_string()))?;
            let at = WallClockWithTz::parse_rfc3339(&at_text)
                .map_err(|_| companion_unavailable(String::from("malformed timeline timestamp")))?;
            if let Some(lower) = since
                && at.as_datetime() < lower.as_datetime()
            {
                continue;
            }
            timeline.push(HistoryMessage {
                id: decode_id(&message_text).map_err(companion_unavailable)?,
                companion,
                round: decode_id(&round_text).map_err(companion_unavailable)?,
                role: decode_role(&role_text).map_err(companion_unavailable)?,
                text: body,
                lang,
                at,
                presence_generation: PresenceGeneration::from_u64(
                    decode_u64(generation_raw).map_err(companion_unavailable)?,
                ),
            });
        }
        let cap = match usize::try_from(limit) {
            Ok(value) => value,
            Err(_) => usize::MAX,
        };
        timeline.truncate(cap);
        Ok(timeline)
    }
}

impl UndeliveredRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn register_if_parent_durable(
        &self,
        entry: UndeliveredRef,
    ) -> Result<bool, UndeliveredTechnicalError> {
        let id_text = encode_id(entry.id);
        let companion_text = encode_id(entry.companion.as_raw());
        let source_text = encode_id(entry.source_message);
        let round_text = encode_id(entry.round);
        let status_text = encode_report_status(entry.status);
        let generation_raw =
            encode_u64(entry.presence_generation.as_u64()).map_err(undelivered_unavailable)?;
        let now_text = WallClockWithTz::now().to_rfc3339();
        let mut guard = lock_shared(&self.conn);
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
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn compare_and_mark_reported(
        &self,
        id: RawId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError> {
        let key = encode_id(id);
        let mut guard = lock_shared(&self.conn);
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
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn list_pending(
        &self,
        companion: CompanionId,
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError> {
        let key = encode_id(companion.as_raw());
        let guard = lock_shared(&self.conn);
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
    }
}

impl ConsentRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
        let guard = lock_shared(&self.conn);
        let found: Option<(String, i64, String, String, String)> = guard
            .query_row(SQL_SELECT_CONSENT, (), |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()
            .map_err(|error| permission_unavailable(error.to_string()))?;
        match found {
            Some((id, rev_raw, provider, model, credential_id)) => {
                let number = decode_u64(rev_raw).map_err(permission_unavailable)?;
                Ok(Some(ConsentRecord {
                    id,
                    rev: ConsentRevision::from_u64(number),
                    provider,
                    model,
                    credential_id,
                }))
            }
            None => Ok(None),
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn save_current(&self, record: ConsentRecord) -> Result<(), PermissionTechnicalError> {
        let rev_raw = encode_u64(record.rev.as_u64()).map_err(permission_unavailable)?;
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| permission_unavailable(error.to_string()))?;
        // Single logical row: evict any previous consent before inserting.
        tx.execute(SQL_DELETE_CONSENT, ())
            .map_err(|error| permission_unavailable(error.to_string()))?;
        tx.execute(
            SQL_INSERT_CONSENT,
            params![
                record.id,
                rev_raw,
                record.provider,
                record.model,
                record.credential_id
            ],
        )
        .map_err(|error| permission_unavailable(error.to_string()))?;
        tx.commit()
            .map_err(|error| permission_unavailable(error.to_string()))?;
        Ok(())
    }
}

impl CredentialRefRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError> {
        let guard = lock_shared(&self.conn);
        guard
            .execute(
                SQL_UPSERT_CREDENTIAL,
                params![cred.id, cred.provider, cred.label],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(())
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
        let guard = lock_shared(&self.conn);
        let found: Option<(String, String, String)> = guard
            .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(found.map(|(id, provider_name, label_name)| CredentialRef {
            id,
            provider: provider_name,
            label: label_name,
        }))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        let guard = lock_shared(&self.conn);
        let mut query = guard
            .prepare(SQL_LIST_CREDENTIALS)
            .map_err(|error| credential_unavailable(error.to_string()))?;
        let rows = query
            .query_map((), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| credential_unavailable(error.to_string()))?;
        let mut refs = Vec::new();
        for row in rows {
            let (id, provider, label) =
                row.map_err(|error| credential_unavailable(error.to_string()))?;
            refs.push(CredentialRef {
                id,
                provider,
                label,
            });
        }
        Ok(refs)
    }
}

impl UsageRepository for Store {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "repository contract is async; the atomic section stays synchronous so no await is held while locked"
    )]
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError> {
        let ticket_text = encode_id(fact.ticket.0);
        let input_column =
            encode_optional_count(fact.input_tokens).map_err(inference_unavailable)?;
        let output_column =
            encode_optional_count(fact.output_tokens).map_err(inference_unavailable)?;
        let guard = lock_shared(&self.conn);
        // Plain insert: a duplicate ticket violates the primary key and maps
        // to `StorageUnavailable`, never a panic.
        guard
            .execute(
                SQL_INSERT_USAGE,
                params![
                    ticket_text,
                    fact.provider,
                    fact.model,
                    input_column,
                    output_column,
                    encode_usage_source(fact.source)
                ],
            )
            .map_err(|error| inference_unavailable(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use ene_companion::{
        AppendHistoryCommand, CompanionId, CompanionLifecycle, CompanionRepository,
        HistoryAppendOutcome, HistoryRepository, HistoryRole, PresentationMark, ReportStatus,
        ReportStatusTransition, UndeliveredRef, UndeliveredRepository,
    };
    use ene_credential::{CredentialRef, CredentialRefRepository};
    use ene_inference::{InferenceTicketId, UsageFact, UsageRepository, UsageSource};
    use ene_permission::{ConsentRecord, ConsentRepository, ConsentRevision};
    use ene_presence::{
        ClientId, LiveReachabilityRef, MoveDecision, PresenceCheckRef, PresenceGeneration,
        PresenceRepository, PresenceState, ThinMoveReason,
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
        }
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
                    super::encode_lifecycle(CompanionLifecycle::Stopped),
                    super::encode_id(companion.as_raw())
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
            .append_reply_with_undelivered(
                history_command(companion, generation, "reply body"),
                true,
            )
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
        let pending = store.list_pending(companion).await;
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
        let pending_after = store.list_pending(companion).await;
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
        let Ok(fact) = confirmed else {
            return;
        };
        assert_eq!(fact.state, PresenceState::Present);
        assert_eq!(fact.active_client, Some(client));
        assert_eq!(fact.generation, next);
    }

    #[tokio::test]
    async fn consent_save_and_load_roundtrip() {
        let Some(store) = open_memory().await else {
            return;
        };
        let empty = store.load_current().await;
        assert!(matches!(empty, Ok(None)), "fresh store holds no consent");
        let record = ConsentRecord {
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(3),
            provider: String::from("acme"),
            model: String::from("dialogue-1"),
            credential_id: String::from("cred-1"),
        };
        let saved = store.save_current(record.clone()).await;
        assert!(saved.is_ok(), "consent save must succeed");
        let loaded = store.load_current().await;
        assert!(loaded.is_ok(), "consent load must succeed");
        let Ok(Some(current)) = loaded else {
            return;
        };
        assert_eq!(current, record, "consent must round-trip");
        let replacement = ConsentRecord {
            rev: ConsentRevision::from_u64(4),
            ..record
        };
        let replaced = store.save_current(replacement.clone()).await;
        assert!(replaced.is_ok(), "consent replacement must succeed");
        let reloaded = store.load_current().await;
        let Ok(Some(single)) = reloaded else {
            return;
        };
        assert_eq!(single, replacement, "save must replace the single row");
    }

    #[tokio::test]
    async fn credential_save_load_and_list() {
        let Some(store) = open_memory().await else {
            return;
        };
        let missing = store.load_ref("acme", "main").await;
        assert!(matches!(missing, Ok(None)), "fresh store holds no refs");
        let cred = CredentialRef {
            id: String::from("acme:main"),
            provider: String::from("acme"),
            label: String::from("main"),
        };
        let saved = store.save_ref(cred.clone()).await;
        assert!(saved.is_ok(), "ref save must succeed");
        let loaded = store.load_ref("acme", "main").await;
        assert!(loaded.is_ok(), "ref load must succeed");
        let Ok(Some(found)) = loaded else {
            return;
        };
        assert_eq!(found, cred, "ref must round-trip");
        let second = CredentialRef {
            id: String::from("acme:backup"),
            provider: String::from("acme"),
            label: String::from("backup"),
        };
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
                params![super::encode_id(ticket)],
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
}
