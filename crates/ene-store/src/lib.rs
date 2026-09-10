//! SQLite-backed durable repositories for the Stage 2 owners.
//!
//! [`Store`] owns one `rusqlite::Connection` and implements every repository
//! contract owned elsewhere: [`PresenceRepository`], [`CompanionRepository`],
//! [`HistoryRepository`], [`UndeliveredRepository`], [`ConsentRepository`],
//! [`CredentialRefRepository`], [`CredentialApprovalRepository`],
//! [`DevicePairingRepository`], and [`UsageRepository`]. Owners never depend
//! on this crate; they program against their own traits.
//!
//! Concurrency shape: the connection is `Send` but not `Sync`, so an
//! `Arc<std::sync::Mutex<Connection>>` shares it across callers. Each
//! repository method hands its whole critical section — lock, one short
//! [`TransactionBehavior::Immediate`] transaction (or one plain statement for
//! pure loads), drop the guard — to `run_blocking`, so the synchronous
//! `rusqlite` work happens on the blocking pool instead of on an async worker.
//! The guard and any transaction never cross an `.await`: they live and die
//! inside the blocking closure. Values that cross the boundary are bound
//! parameters, never interpolated into SQL text.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    CompanionTechnicalError, HistoryAppendOutcome, HistoryMessage, HistoryRepository, HistoryRole,
    PresentationMark, ReportStatus, ReportStatusTransition, RoundIntentMark, UndeliveredRef,
    UndeliveredRepository, UndeliveredTechnicalError,
};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository, CredentialTechnicalError,
    DeviceId, DevicePairingRepository, DevicePairingStatus, DeviceRecord,
    PendingCredentialApproval, PendingPairing,
};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository, InferenceTechnicalError,
    UsageFact, UsageRepository, UsageSource,
};
use ene_permission::{
    ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision, IntentFingerprint,
    IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository, IntentResolution,
    PermissionTechnicalError, ShortcutIntentOutcome, consent_mark_rev,
};
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceGeneration, PresenceRepository, PresenceState,
    PresenceTechnicalError, ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

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

/// Runs one synchronous SQLite critical section on the blocking pool.
///
/// A panic inside the blocking task is the task's own panic: resume it
/// rather than reporting it as a store failure.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

/// SQLite-backed host for every Stage 2 repository contract.
///
/// The single `rusqlite::Connection` is `Send` but not `Sync`; an
/// `Arc<std::sync::Mutex<Connection>>` shares it across the repository
/// implementations. Each method runs its whole critical section on the
/// blocking pool through `run_blocking`: it locks, runs one
/// [`TransactionBehavior::Immediate`] transaction (or one plain statement for
/// pure loads), drops the guard, and returns, so the guard and any transaction
/// live entirely inside the blocking closure and never cross an `.await`.
pub struct Store {
    conn: Arc<Mutex<Connection>>,
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
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        run_blocking(move || Self::open_sync(&path)).await
    }

    fn open_sync(path: &Path) -> Result<Self, StoreError> {
        let mut conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Opens an in-memory store and runs migrations, for tests.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`] when the database cannot be created
    /// and [`StoreError::MigrationFailed`] when the schema cannot be prepared.
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        run_blocking(Self::open_in_memory_sync).await
    }

    fn open_in_memory_sync() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::MigrationFailed)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

/// Appends one history item, optionally registering an undelivered entry
/// for it in the same atomic section.
///
/// Both [`HistoryRepository::append_message`] and
/// [`HistoryRepository::append_reply_with_undelivered`] funnel through
/// here so the lifecycle read, the generation compare, the history insert,
/// and the optional undelivered insert share one `Immediate` transaction.
///
/// Durable idempotency rests on the client-minted `(companion,
/// command_id)`: a retry reuses the same command id with a fresh message
/// id, so an in-transaction pre-check compares the stored
/// [`RequestFingerprint`] against the incoming request's — the same
/// fingerprint type every caller builds — and returns the original
/// [`HistoryAppendOutcome::AlreadyCommittedAs`] without re-appending or
/// re-registering undelivered. The fingerprint covers role, body,
/// language, sending incarnation, and the canonical client round
/// intent; round identity and its wire projection are the *accepted
/// result*, not the request: the Host mints them per intake decision
/// and a retry can re-intake into a newer round, so they never decide
/// conflict — the replay answers the stored accept verbatim. A row
/// whose fingerprint cannot be reconstructed (a pre-mark row) proves
/// nothing: it is declined like any conflicting reuse, never guessed.
/// The generation premise stays out of the fingerprint: it is enforced
/// separately above, so a retry under a newer generation view still
/// replays instead of conflicting. This replaces the retired `local_id`
/// pre-check; `local_id` is stored as correspondence metadata only and
/// is never consulted here. `NULL` command ids carry no replay key and
/// never collide. A reused key with a different request answers
/// [`HistoryAppendOutcome::CommandConflict`] instead: declined without
/// side effects, never rebound.
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
        let stored: Option<(String, i64)> = tx
            .query_row(SQL_SELECT_CONSENT, (), |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(|error| companion_unavailable(error.to_string()))?;
        let current_matches = stored.as_ref().is_some_and(|(id, rev)| {
            id == expected_id && decode_u64(*rev).is_ok_and(|value| value == *expected_rev)
        });
        if !current_matches {
            return Ok((HistoryAppendOutcome::StaleConsent, None));
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
            // The durable key owns its request fingerprint, and the
            // same [`RequestFingerprint`] type every caller builds is
            // the only judge: an exact retry replays the original
            // acceptance even when the Host re-intaked it into a newer
            // round (round and its projection are the accepted result
            // and travel verbatim from the stored row), while the same
            // key with a different request — a different round intent
            // included — is declined without side effects. A row
            // without a reconstructable fingerprint proves nothing and
            // declines the same way (fail-closed); so does a keyed
            // incoming command without a round intent.
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

/// Forward-only schema setup.
///
/// The `_schema_version` singleton records the applied version. Pending
/// migrations and the version bump commit together in one transaction, so
/// a crash mid-migration rolls back and the next open retries from
/// scratch; the commit is the sole version-advancement boundary. No other
/// tables exist here: hint, settings, provider, and consent-history
/// storage are explicitly deferred.
mod migrate {
    use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

    /// Schema version applied by [`run`](run).
    ///
    /// Version 2 adds the nullable `history_message.local_id` column with its
    /// device-pairing tables (the companion-local unique index it briefly
    /// carried is dropped again by version 3). Version 3 adds the nullable
    /// `history_message.command_id` column with the durable
    /// `(companion_id, command_id)` unique replay index; `local_id` stays as
    /// correspondence metadata only. Version 4 adds the `credential_pending`
    /// table for registration approvals; the usable marker stays
    /// `credential_ref`, so no approved table is created. Version 8 adds the
    /// nullable `history_message.round_intent` / `round_intent_ref` columns
    /// that persist the client round intent of the request fingerprint.
    const CURRENT_VERSION: u64 = 8;

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

    /// Version 2 upgrade, applied once when the stored version is below 2.
    ///
    /// Forward-only: existing history rows keep `local_id` NULL, and the new
    /// tables use `IF NOT EXISTS` so a fresh database and an upgraded one
    /// converge on the same shape. `NULL` local ids never collide under the
    /// unique index because SQLite treats each `NULL` as distinct. Version 3
    /// drops this index again when it promotes `command_id` to the durable
    /// replay key; the column itself stays as correspondence metadata.
    const MIGRATION_V2: &str = "
ALTER TABLE history_message ADD COLUMN local_id TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_message_companion_local ON history_message (companion_id, local_id);
CREATE TABLE IF NOT EXISTS pairing_pending (
    descriptor TEXT PRIMARY KEY,
    requested_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS paired_device (
    device_id TEXT PRIMARY KEY,
    descriptor TEXT NOT NULL,
    paired_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_paired_device_descriptor ON paired_device (descriptor);
";

    /// Version 3 upgrade, applied once when the stored version is below 3.
    ///
    /// Forward-only: existing history rows keep `command_id` NULL, and `NULL`
    /// command ids never collide under the new unique index because SQLite
    /// treats each `NULL` as distinct. The retired
    /// `(companion_id, local_id)` unique index is dropped; the `local_id`
    /// column stays as stored correspondence metadata, never a key.
    const MIGRATION_V3: &str = "
ALTER TABLE history_message ADD COLUMN command_id TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_message_companion_command ON history_message (companion_id, command_id);
DROP INDEX IF EXISTS idx_history_message_companion_local;
";

    /// Version 4 upgrade, applied once when the stored version is below 4.
    ///
    /// Forward-only: introduces `credential_pending` for registration
    /// approvals. Fresh and upgraded databases converge via `IF NOT EXISTS`;
    /// no pre-existing rows can reference the new table, so nothing is
    /// backfilled. The usable marker stays `credential_ref` (the register
    /// flow creates the ref only at approval time), so no approved table is
    /// created here.
    const MIGRATION_V4: &str = "
CREATE TABLE IF NOT EXISTS credential_pending (
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    PRIMARY KEY (provider, label)
);
";

    /// Version 5 upgrade, applied once when the stored version is below 5.
    ///
    /// Forward-only: adds the opaque wire projections and the replay
    /// fingerprint. `history_message.round_wire` carries the only round
    /// string that ever crosses the wire (minted fresh per round,
    /// unrelated to the domain bytes); `client_counter`/`client_random`
    /// carry the sending incarnation for fingerprint comparison; all three
    /// stay `NULL` on pre-existing rows, which read back as absent. Same
    /// for `paired_device.wire` (opaque device projection with a uniqueness
    /// guard for new rows), except pre-existing paired rows are backfilled
    /// to the legacy continuity projection (their device identity rendering)
    /// so already-provisioned clients keep resolving after migration; new
    /// approvals mint a fresh opaque projection instead. Fresh and upgraded
    /// databases converge via `IF NOT EXISTS`; nothing else is backfilled.
    const MIGRATION_V5: &str = "
ALTER TABLE history_message ADD COLUMN round_wire TEXT NULL;
ALTER TABLE history_message ADD COLUMN client_counter INTEGER NULL;
ALTER TABLE history_message ADD COLUMN client_random INTEGER NULL;
ALTER TABLE paired_device ADD COLUMN wire TEXT NULL;
UPDATE paired_device SET wire = device_id WHERE wire IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_paired_device_wire ON paired_device (wire);
";

    /// Version 6 upgrade, applied once when the stored version is below 6.
    ///
    /// Forward-only: introduces `management_intent` for durable management
    /// intent replay (design §18.2 idempotency keys). Each row binds one
    /// intent key to the fingerprint it decided on plus its terminal
    /// outcome snapshot, so an exact retry replays without re-executing
    /// while a reused id with new content conflicts instead of rebinding.
    /// Fresh and upgraded databases converge via `IF NOT EXISTS`; no
    /// pre-existing rows can reference the new table, so nothing is
    /// backfilled. Intents decided before this version simply have no
    /// replay row: their retries take the normal premise-checked path.
    const MIGRATION_V6: &str = "
CREATE TABLE IF NOT EXISTS management_intent (
    intent_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    base TEXT NOT NULL,
    rationale_origin TEXT NOT NULL,
    rationale_quote TEXT NULL,
    outcome TEXT NOT NULL,
    mark TEXT NULL
);
";

    /// Version 7 upgrade, applied once when the stored version is below 7.
    ///
    /// Forward-only: introduces `inference_attempt` for the provider-I/O
    /// linearization point. Each row claims one ticket's attempt under one
    /// consent premise in the same short transaction that verifies it, so a
    /// consent mutation either precedes the claim (the claim fails stale
    /// before any byte leaves) or follows it (adoption decides separately).
    /// Fresh and upgraded databases converge via `IF NOT EXISTS`; ticket
    /// ids are single-use, so nothing is backfilled and re-claiming one
    /// ticket answers stale instead of sending twice.
    const MIGRATION_V7: &str = "
CREATE TABLE IF NOT EXISTS inference_attempt (
    ticket TEXT PRIMARY KEY,
    consent_id TEXT NOT NULL,
    consent_rev INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    started_at TEXT NOT NULL
);
";

    /// Version 8 upgrade, applied once when the stored version is below 8.
    ///
    /// Forward-only: adds the persisted client round intent of the history
    /// request fingerprint (`round_intent` kind, `round_intent_ref`
    /// payload). The intent is request semantics — Auto, force-new, or a
    /// join of the round reference the Client sent — so a restart can still
    /// decide replay from durable state. Pre-existing rows keep both
    /// columns `NULL`: their intent was never stored, so their sameness
    /// cannot be proven and every replay attempt against them fails closed
    /// instead of being guessed.
    const MIGRATION_V8: &str = "
ALTER TABLE history_message ADD COLUMN round_intent TEXT NULL;
ALTER TABLE history_message ADD COLUMN round_intent_ref TEXT NULL;
";
    /// Creates or upgrades the schema on an open connection.
    ///
    /// Atomic: pending migrations and the version bump commit together in
    /// one transaction, so a crash mid-migration rolls back to the
    /// pre-migration state and the next open retries from scratch. The
    /// commit is the sole version-advancement boundary. Rejects a database
    /// newer than this binary understands instead of guessing.
    pub(super) fn run(conn: &mut Connection) -> Result<(), String> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS _schema_version (version INTEGER NOT NULL)")
            .map_err(|error| error.to_string())?;
        let stored: Option<i64> = tx
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
        tx.execute_batch(SCHEMA)
            .map_err(|error| error.to_string())?;
        if stored_version < 2 {
            tx.execute_batch(MIGRATION_V2)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 3 {
            tx.execute_batch(MIGRATION_V3)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 4 {
            tx.execute_batch(MIGRATION_V4)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 5 {
            tx.execute_batch(MIGRATION_V5)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 6 {
            tx.execute_batch(MIGRATION_V6)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 7 {
            tx.execute_batch(MIGRATION_V7)
                .map_err(|error| error.to_string())?;
        }
        if stored_version < 8 {
            tx.execute_batch(MIGRATION_V8)
                .map_err(|error| error.to_string())?;
        }
        let current = i64::try_from(CURRENT_VERSION)
            .map_err(|_| String::from("schema version out of range"))?;
        if stored.is_none() {
            tx.execute(
                "INSERT INTO _schema_version (version) VALUES (?1)",
                rusqlite::params![current],
            )
            .map_err(|error| error.to_string())?;
        } else if stored_version < CURRENT_VERSION {
            tx.execute(
                "UPDATE _schema_version SET version = ?1",
                rusqlite::params![current],
            )
            .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
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
const SQL_INSERT_HISTORY: &str = "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";
const SQL_SELECT_TIMELINE: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 ORDER BY rowid ASC";
const SQL_SELECT_HISTORY_BY_LOCAL_ID: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND local_id = ?2 ORDER BY rowid ASC LIMIT 1";
const SQL_SELECT_HISTORY_BY_COMMAND: &str = "SELECT message_id, round_id, role, body, lang, at, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random FROM history_message WHERE companion_id = ?1 AND command_id = ?2 ORDER BY rowid ASC LIMIT 1";
const SQL_FIND_HISTORY: &str = "SELECT 1 FROM history_message WHERE message_id = ?1";
const SQL_INSERT_UNDELIVERED: &str = "INSERT INTO undelivered (undelivered_id, companion_id, source_message, status, round_id, presence_generation, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
const SQL_SELECT_UNDELIVERED_STATUS: &str =
    "SELECT status FROM undelivered WHERE undelivered_id = ?1";
const SQL_UPDATE_UNDELIVERED_STATUS: &str =
    "UPDATE undelivered SET status = ?1 WHERE undelivered_id = ?2";
const SQL_SELECT_PENDING: &str = "SELECT undelivered_id, companion_id, source_message, status, round_id, presence_generation FROM undelivered WHERE companion_id = ?1 AND status = ?2 ORDER BY rowid ASC";
const SQL_SELECT_CONSENT: &str =
    "SELECT id, rev, provider, model, credential_id FROM consent_record LIMIT 1";
const SQL_INSERT_CONSENT: &str = "INSERT INTO consent_record (id, rev, provider, model, credential_id) VALUES (?1, ?2, ?3, ?4, ?5)";
const SQL_UPDATE_CONSENT: &str =
    "UPDATE consent_record SET id = ?1, rev = ?2, provider = ?3, model = ?4, credential_id = ?5";
const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";
const SQL_INSERT_INTENT_OUTCOME: &str = "INSERT INTO management_intent (intent_id, kind, target, base, rationale_origin, rationale_quote, outcome, mark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
const SQL_SELECT_INTENT_OUTCOME: &str = "SELECT kind, target, base, rationale_origin, rationale_quote, outcome, mark FROM management_intent WHERE intent_id = ?1";
const SQL_INSERT_ATTEMPT: &str = "INSERT INTO inference_attempt (ticket, consent_id, consent_rev, provider, model, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
const SQL_SELECT_CREDENTIAL: &str =
    "SELECT id, provider, label FROM credential_ref WHERE provider = ?1 AND label = ?2";
const SQL_LIST_CREDENTIALS: &str = "SELECT id, provider, label FROM credential_ref ORDER BY id ASC";
const SQL_INSERT_USAGE: &str = "INSERT INTO usage_fact (ticket, provider, model, input_tokens, output_tokens, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
const SQL_SELECT_PAIRED_BY_DESCRIPTOR: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE descriptor = ?1";
const SQL_SELECT_DEVICE_BY_ID: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE device_id = ?1";
const SQL_SELECT_PENDING_BY_DESCRIPTOR: &str =
    "SELECT descriptor, requested_at FROM pairing_pending WHERE descriptor = ?1";
const SQL_INSERT_PENDING_IGNORE: &str =
    "INSERT OR IGNORE INTO pairing_pending (descriptor, requested_at) VALUES (?1, ?2)";
const SQL_DELETE_PENDING: &str = "DELETE FROM pairing_pending WHERE descriptor = ?1";
const SQL_INSERT_PAIRED: &str =
    "INSERT INTO paired_device (device_id, descriptor, paired_at, wire) VALUES (?1, ?2, ?3, ?4)";
const SQL_SELECT_DEVICE_BY_WIRE: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE wire = ?1";
const SQL_LIST_PENDING_PAIRINGS: &str =
    "SELECT descriptor, requested_at FROM pairing_pending ORDER BY rowid ASC";
const SQL_SELECT_CREDENTIAL_PENDING: &str = "SELECT provider, label, requested_at FROM credential_pending WHERE provider = ?1 AND label = ?2";
const SQL_INSERT_CREDENTIAL_PENDING_IGNORE: &str =
    "INSERT OR IGNORE INTO credential_pending (provider, label, requested_at) VALUES (?1, ?2, ?3)";
const SQL_DELETE_CREDENTIAL_PENDING: &str =
    "DELETE FROM credential_pending WHERE provider = ?1 AND label = ?2";
const SQL_LIST_CREDENTIAL_PENDING: &str =
    "SELECT provider, label, requested_at FROM credential_pending ORDER BY rowid ASC";

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

/// Encodes a client round intent into its stored `(kind, reference)` pair.
///
/// Only an [`RoundIntentMark::Existing`] carries a reference, and it is the
/// Client-supplied round reference verbatim.
fn encode_round_intent(intent: &RoundIntentMark) -> (&'static str, Option<&str>) {
    match intent {
        RoundIntentMark::Auto => ("auto", None),
        RoundIntentMark::New => ("new", None),
        RoundIntentMark::Existing(reference) => ("existing", Some(reference)),
    }
}

/// Reads a stored round intent back into its domain mark.
///
/// `None` intent means the row predates the mark (or carries no command
/// key): the caller fail-closes on replay instead of guessing. A reference
/// without its `existing` kind — or the kind without one — is a malformed
/// row, never a defaulted value.
fn decode_round_intent(
    kind: Option<&str>,
    reference: Option<String>,
) -> Result<Option<RoundIntentMark>, String> {
    match (kind, reference) {
        (None, None) => Ok(None),
        (Some("auto"), None) => Ok(Some(RoundIntentMark::Auto)),
        (Some("new"), None) => Ok(Some(RoundIntentMark::New)),
        (Some("existing"), Some(reference)) => Ok(Some(RoundIntentMark::Existing(reference))),
        _ => Err(String::from("malformed history round intent")),
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

/// Reads one consent row into its domain record.
fn decode_consent(
    id: String,
    rev_raw: i64,
    provider: String,
    model: String,
    credential_id: String,
) -> Result<ConsentRecord, String> {
    let number = decode_u64(rev_raw)?;
    Ok(ConsentRecord {
        id,
        rev: ConsentRevision::from_u64(number),
        provider,
        model,
        credential_id,
    })
}

/// Encodes an intent outcome snapshot into its stored `(kind, mark)` pair.
fn encode_intent_outcome(outcome: &IntentOutcome) -> (&'static str, Option<&str>) {
    match outcome {
        IntentOutcome::StoredAsRuleView { revision } => ("stored", Some(revision)),
        IntentOutcome::AppliedAsOneTime => ("applied", None),
        IntentOutcome::HeldByOperation => ("held", None),
        IntentOutcome::NeedsClarification => ("clarify", None),
        IntentOutcome::StaleBaseView { current } => ("stale", Some(current)),
    }
}

/// Reads one stored outcome snapshot back into its domain outcome.
///
/// Unknown kinds — or a stored outcome without its mark — are malformed
/// rows, never guessed: the caller fails closed.
fn decode_intent_outcome(
    outcome_text: &str,
    mark: Option<String>,
) -> Result<IntentOutcome, String> {
    match (outcome_text, mark) {
        ("stored", Some(revision)) => Ok(IntentOutcome::StoredAsRuleView { revision }),
        ("applied", _) => Ok(IntentOutcome::AppliedAsOneTime),
        ("held", _) => Ok(IntentOutcome::HeldByOperation),
        ("clarify", _) => Ok(IntentOutcome::NeedsClarification),
        ("stale", Some(current)) => Ok(IntentOutcome::StaleBaseView { current }),
        _ => Err(String::from("malformed intent outcome")),
    }
}

/// Reads one paired-device row into its domain record.
///
/// Compares the content fields only: both rows share the key by
/// construction, so the key itself carries no information.
fn fingerprints_match(stored: &IntentFingerprint, incoming: &IntentFingerprint) -> bool {
    stored.kind == incoming.kind
        && stored.target == incoming.target
        && stored.base == incoming.base
        && stored.rationale_origin == incoming.rationale_origin
        && stored.rationale_quote == incoming.rationale_quote
}

/// Maps an existing row to replay-or-conflict against `fingerprint`.
///
/// Shared by the claim check and the insert-race fallback so both answer
/// from the same rule: exact content replays, anything else clarifies.
fn replay_or_conflict<T>(
    stored: IntentOutcomeRecord,
    fingerprint: &IntentFingerprint,
) -> IntentResolution<T> {
    if fingerprints_match(&stored.fingerprint, fingerprint) {
        IntentResolution::Replay(stored)
    } else {
        IntentResolution::Conflict(stored)
    }
}

/// Inserts the decided snapshot, resolving a lost primary-key race to the
/// winner instead of overwriting it.
///
/// Returns `None` when this call stored the row, or the winning row when a
/// concurrent writer committed first (cross-process only; same-process
/// writers serialize on the shared connection, so the pre-check above
/// always wins there). Callers must NOT commit on `Some`: dropping the
/// transaction rolls back any decision writes made after the pre-check, so
/// a loser changes nothing.
fn insert_decided_row_tx(
    tx: &Transaction<'_>,
    fingerprint: &IntentFingerprint,
    outcome: &IntentOutcome,
) -> Result<Option<IntentOutcomeRecord>, String> {
    let (outcome_text, mark) = encode_intent_outcome(outcome);
    match tx.execute(
        SQL_INSERT_INTENT_OUTCOME,
        params![
            fingerprint.intent_id,
            fingerprint.kind,
            fingerprint.target,
            fingerprint.base,
            fingerprint.rationale_origin,
            fingerprint.rationale_quote.as_deref(),
            outcome_text,
            mark,
        ],
    ) {
        Ok(_) => Ok(None),
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
        {
            select_intent_row_tx(tx, &fingerprint.intent_id)?.map_or_else(
                || Err(String::from("intent row vanished after write conflict")),
                |winner| Ok(Some(winner)),
            )
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Builds the domain record for one decoded intent-outcome row.
fn decode_intent_outcome_row(
    intent_id: &str,
    row: IntentOutcomeRow,
) -> Result<IntentOutcomeRecord, String> {
    let (kind, target, base, rationale_origin, rationale_quote, outcome_text, mark) = row;
    let outcome = decode_intent_outcome(&outcome_text, mark)?;
    Ok(IntentOutcomeRecord {
        fingerprint: IntentFingerprint {
            intent_id: intent_id.to_owned(),
            kind,
            target,
            base,
            rationale_origin,
            rationale_quote,
        },
        outcome,
    })
}

/// Reads one intent outcome row inside the caller's transaction.
fn select_intent_row_tx(
    tx: &Transaction<'_>,
    intent_id: &str,
) -> Result<Option<IntentOutcomeRecord>, String> {
    let found: Option<IntentOutcomeRow> = tx
        .query_row(SQL_SELECT_INTENT_OUTCOME, params![intent_id], |row| {
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
        .optional()
        .map_err(|error| error.to_string())?;
    found
        .map(|row| decode_intent_outcome_row(intent_id, row))
        .transpose()
}

/// Reads one paired-device row into its domain record.
///
/// `wire` carries the opaque wire projection; pre-opaque rows store `NULL`,
/// which decodes to the legacy continuity projection (the device identity
/// rendering) so already-provisioned clients keep resolving after migration.
/// New approvals always store a fresh opaque projection instead.
fn decode_device_record(
    device_text: &str,
    descriptor: String,
    paired_text: &str,
    wire: Option<String>,
) -> Result<DeviceRecord, String> {
    let paired_at = WallClockWithTz::parse_rfc3339(paired_text)
        .map_err(|_| String::from("malformed device pairing timestamp"))?;
    Ok(DeviceRecord {
        id: DeviceId(decode_id(device_text)?),
        wire: wire.unwrap_or_else(|| device_text.to_owned()),
        descriptor,
        paired_at,
    })
}

/// Reads one pending-pairing row into its domain fact.
fn decode_pending_pairing(
    descriptor: String,
    requested_text: &str,
) -> Result<PendingPairing, String> {
    let requested_at = WallClockWithTz::parse_rfc3339(requested_text)
        .map_err(|_| String::from("malformed pairing request timestamp"))?;
    Ok(PendingPairing {
        descriptor,
        requested_at,
    })
}

/// Reports whether a credential `(provider, label)` pair is blank.
///
/// Empty or whitespace-only input is treated as absent: callers check this
/// before touching the store, so blank pairs never become stored rows.
fn credential_pair_is_blank(provider: &str, label: &str) -> bool {
    provider.trim().is_empty() || label.trim().is_empty()
}

/// Reads one pending-credential row into its domain fact.
fn decode_pending_credential(
    provider: String,
    label: String,
    requested_text: &str,
) -> Result<PendingCredentialApproval, String> {
    let requested_at = WallClockWithTz::parse_rfc3339(requested_text)
        .map_err(|_| String::from("malformed credential approval timestamp"))?;
    Ok(PendingCredentialApproval {
        provider,
        label,
        requested_at,
    })
}

/// One decoded history row: identity, round, role, body, language,
/// timestamp, generation, optional command-scoped replay identity, optional
/// wire projection and round intent for the replay fingerprint, and
/// optional client-local correspondence ID (`local_id` is stored metadata
/// only, never a key).
///
/// Named fields (instead of the retired positional tuple) so column order
/// lives in exactly one place: [`HistoryRow::from_row`]. All three readers
/// (`lookup_local_id`, `lookup_command`, `load_timeline`) share the column
/// order through that constructor.
struct HistoryRow {
    /// Stored message identity text.
    message_text: String,
    /// Stored round identity text.
    round_text: String,
    /// Stored role text.
    role_text: String,
    /// Stored body text.
    body: String,
    /// Stored language tag.
    lang: String,
    /// Stored timestamp rendering.
    at_text: String,
    /// Stored presence generation count.
    generation_raw: i64,
    /// Stored command-scoped replay identity, if any.
    command_text: Option<String>,
    /// Stored client-local correspondence ID, if any.
    stored_local_id: Option<String>,
    /// Stored opaque round wire projection, if any.
    round_wire: Option<String>,
    /// Stored round intent kind, if any (`NULL` on pre-mark rows).
    round_intent_kind: Option<String>,
    /// Stored round intent reference, if any.
    round_intent_ref: Option<String>,
    /// Stored sending incarnation counter, if any.
    client_counter: Option<i64>,
    /// Stored sending incarnation random, if any.
    client_random: Option<i64>,
}

impl HistoryRow {
    /// Reads one row in the shared column order of the history selects.
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_text: row.get(0)?,
            round_text: row.get(1)?,
            role_text: row.get(2)?,
            body: row.get(3)?,
            lang: row.get(4)?,
            at_text: row.get(5)?,
            generation_raw: row.get(6)?,
            command_text: row.get(7)?,
            stored_local_id: row.get(8)?,
            round_wire: row.get(9)?,
            round_intent_kind: row.get(10)?,
            round_intent_ref: row.get(11)?,
            client_counter: row.get(12)?,
            client_random: row.get(13)?,
        })
    }
}

/// One decoded intent-outcome row: kind, target, base, rationale origin,
/// optional rationale quote, outcome kind, and optional outcome mark.
type IntentOutcomeRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

/// Reads one history row into its domain message.
///
/// `command_text` carries the client-minted replay identity (`None` stores
/// `NULL`, meaning no replay key); `round_wire` carries the opaque wire
/// projection (`None` on pre-opaque rows); the round intent columns decode
/// through [`decode_round_intent`] (never guessed); the client counter/
/// random pair carries the sending incarnation only when both are present
/// and decode (`None` otherwise, including pre-opaque rows); `local_id`
/// carries client-local correspondence metadata only.
fn decode_history_message(
    companion: CompanionId,
    row: HistoryRow,
) -> Result<HistoryMessage, String> {
    let HistoryRow {
        message_text,
        round_text,
        role_text,
        body,
        lang,
        at_text,
        generation_raw,
        command_text,
        stored_local_id,
        round_wire,
        round_intent_kind,
        round_intent_ref,
        client_counter,
        client_random,
    } = row;
    let at = WallClockWithTz::parse_rfc3339(&at_text)
        .map_err(|_| String::from("malformed timeline timestamp"))?;
    let mut command_id = None;
    if let Some(text) = command_text.as_deref() {
        command_id = Some(CommandId(decode_id(text)?));
    }
    let incarnation = match (client_counter, client_random) {
        (Some(counter_raw), Some(random_raw)) => {
            Some((decode_u64(counter_raw)?, decode_u64(random_raw)?))
        }
        (None, None) => None,
        _ => return Err(String::from("malformed history incarnation")),
    };
    let round_intent = decode_round_intent(round_intent_kind.as_deref(), round_intent_ref)?;
    Ok(HistoryMessage {
        id: decode_id(&message_text)?,
        companion,
        round: decode_id(&round_text)?,
        role: decode_role(&role_text)?,
        text: body,
        lang,
        at,
        presence_generation: PresenceGeneration::from_u64(decode_u64(generation_raw)?),
        command_id,
        round_wire,
        round_intent,
        incarnation,
        local_id: stored_local_id,
    })
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
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion);
            let guard = lock_shared(&conn);
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
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion);
            let target_text = to_client.map(|client| encode_id(client.as_raw()));
            let reason_text = encode_move_reason(reason);
            let now_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
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
        })
        .await
    }

    async fn confirm_transition(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<ConfirmTransitionOutcome, PresenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let key = encode_id(companion);
            let now_text = WallClockWithTz::now().to_rfc3339();
            let confirm_reason = if live.connection_live {
                "confirm_live"
            } else {
                "confirm_not_live"
            };
            let mut guard = lock_shared(&conn);
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
            Ok(ConfirmTransitionOutcome::Confirmed(PresenceAttribution {
                companion,
                state: target_state,
                active_client: target_client,
                generation: transitioning_generation,
            }))
        })
        .await
    }
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
            // Pure load: one statement, no transaction. Correspondence lookup for
            // matching an input to its ack; durable replay keys on `command_id`
            // instead (see `lookup_command`).
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
            // Pure load: one statement, no transaction. Durable replay lookup on
            // the `(companion, command_id)` key; `NULL` command ids never match.
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

impl ConsentRepository for Store {
    async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
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
                    let record = decode_consent(id, rev_raw, provider, model, credential_id)
                        .map_err(permission_unavailable)?;
                    Ok(Some(record))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn compare_and_save(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
    ) -> Result<ConsentCommitOutcome, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            let outcome = compare_and_save_row(
                &tx,
                expected.as_ref().map(|(id, rev)| (id.as_str(), rev)),
                &record,
            )
            .map_err(permission_unavailable)?;
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(outcome)
        })
        .await
    }
}

/// Runs the consent compare-and-save inside the caller's transaction.
///
/// Shared by [`ConsentRepository::compare_and_save`] and the intent-atomic
/// variant so the premise check and the write cannot drift apart between
/// the two entry points.
fn compare_and_save_row(
    tx: &Transaction<'_>,
    expected: Option<(&str, &ConsentRevision)>,
    record: &ConsentRecord,
) -> Result<ConsentCommitOutcome, String> {
    let rev_raw = encode_u64(record.rev.as_u64())?;
    let found: Option<(String, i64, String, String, String)> = tx
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
        .map_err(|error| error.to_string())?;
    let current = match found {
        Some((id, stored_rev, provider, model, credential_id)) => Some(decode_consent(
            id,
            stored_rev,
            provider,
            model,
            credential_id,
        )?),
        None => None,
    };
    let matches = match (&current, &expected) {
        (None, None) => true,
        (Some(stored), Some((id, rev))) => stored.id == *id && stored.rev == **rev,
        _ => false,
    };
    if !matches {
        return Ok(ConsentCommitOutcome::StaleCurrent { current });
    }
    if current.is_none() {
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
        .map_err(|error| error.to_string())?;
    } else {
        // Single logical row: the expectation matched, so overwrite it.
        tx.execute(
            SQL_UPDATE_CONSENT,
            params![
                record.id,
                rev_raw,
                record.provider,
                record.model,
                record.credential_id
            ],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(ConsentCommitOutcome::Committed {
        record: record.clone(),
    })
}

impl IntentOutcomeRepository for Store {
    async fn record_intent_outcome(
        &self,
        record: IntentOutcomeRecord,
    ) -> Result<IntentResolution<()>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // Write-once claim first: an existing row is never rewritten.
            if let Some(stored) = select_intent_row_tx(&tx, &record.fingerprint.intent_id)
                .map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &record.fingerprint));
            }
            match insert_decided_row_tx(&tx, &record.fingerprint, &record.outcome)
                .map_err(permission_unavailable)?
            {
                None => {
                    tx.commit()
                        .map_err(|error| permission_unavailable(error.to_string()))?;
                    Ok(IntentResolution::Decided(()))
                }
                Some(winner) => Ok(replay_or_conflict(winner, &record.fingerprint)),
            }
        })
        .await
    }

    async fn lookup_intent_outcome(
        &self,
        intent_id: &str,
    ) -> Result<Option<IntentOutcomeRecord>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let intent_id = intent_id.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<IntentOutcomeRow> = guard
                .query_row(SQL_SELECT_INTENT_OUTCOME, params![intent_id], |row| {
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
                .optional()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            match found {
                Some(row) => decode_intent_outcome_row(&intent_id, row).map(Some),
                None => Ok(None),
            }
            .map_err(permission_unavailable)
        })
        .await
    }

    async fn assign_with_intent(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ConsentCommitOutcome>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // Write-once claim first: an existing row decides without touching
            // consent, so a concurrent same-id send can neither fork the answer
            // nor re-run the compare-and-save.
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            let outcome = compare_and_save_row(
                &tx,
                expected.as_ref().map(|(id, rev)| (id.as_str(), rev)),
                &record,
            )
            .map_err(permission_unavailable)?;
            // The replay row shares the decision transaction: a crash can
            // neither strand a commit without its marker nor a marker without
            // its commit. Stale attempts record their stale snapshot here too,
            // so a retried id always observes the same answer. The commit
            // snapshot carries the committed revision, so replay answers it
            // verbatim.
            let snapshot = match &outcome {
                ConsentCommitOutcome::Committed { record } => IntentOutcome::StoredAsRuleView {
                    revision: record.rev.as_u64().to_string(),
                },
                ConsentCommitOutcome::StaleCurrent { current } => IntentOutcome::StaleBaseView {
                    current: consent_mark_rev(current.as_ref().map(|record| record.rev.as_u64())),
                },
            };
            match insert_decided_row_tx(&tx, &fingerprint, &snapshot)
                .map_err(permission_unavailable)?
            {
                None => {
                    tx.commit()
                        .map_err(|error| permission_unavailable(error.to_string()))?;
                    Ok(IntentResolution::Decided(outcome))
                }
                // Lost a cross-process race after deciding: roll back (dropping
                // `tx` without committing) so the loser changes nothing, and
                // answer from the winner.
                Some(winner) => Ok(replay_or_conflict(winner, &fingerprint)),
            }
        })
        .await
    }

    async fn request_approval_with_intent(
        &self,
        provider: String,
        label: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<IntentOutcomeRecord>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            if credential_pair_is_blank(&provider, &label) {
                return Err(permission_unavailable(String::from(
                    "blank credential pair",
                )));
            }
            let requested_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // Write-once claim first: an existing row decides without touching
            // credential state.
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            // One transaction: the pending insert (or usable recheck) plus the
            // replay row, so the decided snapshot and the state it describes
            // can never strand apart.
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            let outcome = if usable.is_some() {
                IntentOutcome::AppliedAsOneTime
            } else {
                tx.execute(
                    SQL_INSERT_CREDENTIAL_PENDING_IGNORE,
                    params![provider, label, requested_text],
                )
                .map_err(|error| permission_unavailable(error.to_string()))?;
                IntentOutcome::HeldByOperation
            };
            let decided = IntentOutcomeRecord {
                fingerprint,
                outcome,
            };
            match insert_decided_row_tx(&tx, &decided.fingerprint, &decided.outcome)
                .map_err(permission_unavailable)?
            {
                None => {
                    tx.commit()
                        .map_err(|error| permission_unavailable(error.to_string()))?;
                    Ok(IntentResolution::Decided(decided))
                }
                // Lost a cross-process race after deciding: roll back (dropping
                // `tx` without committing) so the loser changes nothing, and
                // answer from the winner.
                Some(winner) => Ok(replay_or_conflict(winner, &decided.fingerprint)),
            }
        })
        .await
    }

    async fn complete_with_intent(
        &self,
        expected_base: String,
        bearer_present: bool,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<IntentOutcomeRecord>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // Write-once claim first: an existing row decides without
            // re-reading consent, so a concurrent same-id send can neither
            // fork the answer nor re-run the mark comparison.
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            // One transaction: compare the base mark, verify completability,
            // and record the decided snapshot together. Every decided outcome
            // is recorded (even stale/clarify), so a retried id always observes
            // the same answer; only store failures hold unrecorded.
            let found: Option<(String, i64, String, String, String)> = tx
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
            let current = match found {
                Some((id, stored_rev, provider, model, credential_id)) => Some(
                    decode_consent(id, stored_rev, provider, model, credential_id)
                        .map_err(permission_unavailable)?,
                ),
                None => None,
            };
            let mark = consent_mark_rev(current.as_ref().map(|record| record.rev.as_u64()));
            let outcome = if mark != expected_base {
                IntentOutcome::StaleBaseView {
                    current: mark.clone(),
                }
            } else if current.is_some() && bearer_present {
                IntentOutcome::AppliedAsOneTime
            } else {
                IntentOutcome::NeedsClarification
            };
            let decided = IntentOutcomeRecord {
                fingerprint,
                outcome,
            };
            match insert_decided_row_tx(&tx, &decided.fingerprint, &decided.outcome)
                .map_err(permission_unavailable)?
            {
                None => {
                    tx.commit()
                        .map_err(|error| permission_unavailable(error.to_string()))?;
                    Ok(IntentResolution::Decided(decided))
                }
                // Lost a cross-process race after deciding: roll back (dropping
                // `tx` without committing) so the loser changes nothing, and
                // answer from the winner.
                Some(winner) => Ok(replay_or_conflict(winner, &decided.fingerprint)),
            }
        })
        .await
    }

    async fn shortcut_with_intent(
        &self,
        provider: String,
        model: String,
        credential_id: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ShortcutIntentOutcome>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // Write-once claim first: an existing row decides without reading
            // consent, so a concurrent same-id send can neither fork the answer
            // nor re-run the route check.
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            // One transaction: read current, and — only when the stored route
            // already equals the requested one — insert the `Stored` snapshot
            // for the current revision. No state changes either way.
            let found: Option<(String, i64, String, String, String)> = tx
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
            let current = match found {
                Some((id, stored_rev, provider, model, credential_id)) => Some(
                    decode_consent(id, stored_rev, provider, model, credential_id)
                        .map_err(permission_unavailable)?,
                ),
                None => None,
            };
            let matches = current.as_ref().is_some_and(|stored| {
                stored.provider == provider
                    && stored.model == model
                    && stored.credential_id == credential_id
            });
            if !matches {
                return Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss {
                    current,
                }));
            }
            let Some(record) = current else {
                return Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss {
                    current: None,
                }));
            };
            let snapshot = IntentOutcome::StoredAsRuleView {
                revision: record.rev.as_u64().to_string(),
            };
            match insert_decided_row_tx(&tx, &fingerprint, &snapshot)
                .map_err(permission_unavailable)?
            {
                None => {
                    tx.commit()
                        .map_err(|error| permission_unavailable(error.to_string()))?;
                    Ok(IntentResolution::Decided(ShortcutIntentOutcome::Hit {
                        current: record,
                    }))
                }
                // Lost a cross-process race after deciding: roll back (dropping
                // `tx` without committing) so the loser changes nothing, and
                // answer from the winner.
                Some(winner) => Ok(replay_or_conflict(winner, &fingerprint)),
            }
        })
        .await
    }
}

impl InferenceAttemptRepository for Store {
    async fn begin_inference_attempt(
        &self,
        attempt: InferenceAttempt,
    ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let rev_raw = encode_u64(attempt.expected_consent.1).map_err(inference_unavailable)?;
            let ticket_text = encode_id(attempt.ticket.0);
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            // The linearization point: read, compare, and claim share one short
            // transaction that never spans provider I/O. A mutation that
            // committed first fails the compare (no byte leaves); a mutation
            // that commits after only affects result adoption, never the fact
            // that this attempt started under a verified premise.
            let stored: Option<(String, i64)> = tx
                .query_row(SQL_SELECT_CONSENT, (), |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let current_matches = stored.as_ref().is_some_and(|(id, rev)| {
                id == &attempt.expected_consent.0
                    && decode_u64(*rev).is_ok_and(|value| value == attempt.expected_consent.1)
            });
            if !current_matches {
                return Ok(AttemptBeginOutcome::Stale);
            }
            let started_text = WallClockWithTz::now().to_rfc3339();
            match tx.execute(
                SQL_INSERT_ATTEMPT,
                params![
                    ticket_text,
                    attempt.expected_consent.0,
                    rev_raw,
                    attempt.provider,
                    attempt.model,
                    started_text,
                ],
            ) {
                Ok(_) => {}
                // A duplicate ticket re-claims an already-started attempt: stale
                // (never send twice), never a storage error.
                Err(error)
                    if error.sqlite_error_code()
                        == Some(rusqlite::ErrorCode::ConstraintViolation) =>
                {
                    return Ok(AttemptBeginOutcome::Stale);
                }
                Err(error) => return Err(inference_unavailable(error.to_string())),
            }
            tx.commit()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            Ok(AttemptBeginOutcome::Started)
        })
        .await
    }
}

impl CredentialRefRepository for Store {
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            guard
                .execute(
                    SQL_UPSERT_CREDENTIAL,
                    params![cred.id(), cred.provider(), cred.label()],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<(String, String, String)> = guard
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((_, provider_name, label_name)) = found else {
                return Ok(None);
            };
            let cred = CredentialRef::new(provider_name, label_name)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(Some(cred))
        })
        .await
    }

    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
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
                let (_, provider, label) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                let cred = CredentialRef::new(provider, label)
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                refs.push(cred);
            }
            Ok(refs)
        })
        .await
    }
}

/// Mints a one-time pairing secret for display-once custody.
///
/// Secrets are never stored: the caller shows the returned string once on a
/// trusted surface and holds it only in memory afterwards.
fn fresh_pairing_secret() -> String {
    RawId::new().as_uuid().to_string()
}

impl DevicePairingRepository for Store {
    async fn request_pairing(
        &self,
        descriptor: String,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let requested = WallClockWithTz::now();
            let requested_text = requested.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // An already-paired descriptor short-circuits: re-requests leave the
            // stored record untouched.
            let paired: Option<(String, String, String, Option<String>)> = tx
                .query_row(
                    SQL_SELECT_PAIRED_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if let Some((device_text, stored_descriptor, paired_text, wire)) = paired {
                let device =
                    decode_device_record(&device_text, stored_descriptor, &paired_text, wire)
                        .map_err(credential_unavailable)?;
                return Ok(DevicePairingStatus::Paired { device });
            }
            // `INSERT OR IGNORE` keeps a previously stored pending entry: the
            // re-read below returns it unchanged instead of refreshing its time.
            tx.execute(
                SQL_INSERT_PENDING_IGNORE,
                params![descriptor, requested_text],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let stored: Option<(String, String)> = tx
                .query_row(
                    SQL_SELECT_PENDING_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_descriptor, stored_requested)) = stored else {
                return Err(credential_unavailable(String::from(
                    "pairing request vanished after insert",
                )));
            };
            let pending = decode_pending_pairing(stored_descriptor, &stored_requested)
                .map_err(credential_unavailable)?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(DevicePairingStatus::Pending { pending })
        })
        .await
    }

    async fn approve_pending(
        &self,
        descriptor: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let descriptor = descriptor.to_owned();
        run_blocking(move || {
            let device_id = RawId::new();
            let device_text = encode_id(device_id);
            let paired_at = WallClockWithTz::now();
            let paired_text = paired_at.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // Re-approving an already-paired descriptor returns the existing
            // record unchanged with a freshly minted secret (rotation); no
            // fresh identity is stored. Secrets are never stored: the caller
            // displays the returned string once on a trusted surface.
            let paired: Option<(String, String, String, Option<String>)> = tx
                .query_row(
                    SQL_SELECT_PAIRED_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if let Some((stored_text, stored_descriptor, stored_paired, wire)) = paired {
                let device =
                    decode_device_record(&stored_text, stored_descriptor, &stored_paired, wire)
                        .map_err(credential_unavailable)?;
                return Ok(Some((device, fresh_pairing_secret())));
            }
            let pending: Option<(String, String)> = tx
                .query_row(
                    SQL_SELECT_PENDING_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_descriptor, _)) = pending else {
                return Ok(None);
            };
            // The pending delete and the paired insert share one transaction so
            // an approval never strands a descriptor in both tables or neither.
            // The wire projection is minted fresh here, unrelated to the device
            // identity bytes: it is the only device string that ever crosses
            // the wire.
            let wire = RawId::new().as_uuid().to_string();
            tx.execute(SQL_DELETE_PENDING, params![descriptor])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_INSERT_PAIRED,
                params![device_text, stored_descriptor, paired_text, wire],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(Some((
                DeviceRecord {
                    id: DeviceId(device_id),
                    wire,
                    descriptor: stored_descriptor,
                    paired_at,
                },
                fresh_pairing_secret(),
            )))
        })
        .await
    }

    async fn find_device(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let id = *id;
        run_blocking(move || {
            let key = encode_id(id.0);
            let guard = lock_shared(&conn);
            let found: Option<(String, String, String, Option<String>)> = guard
                .query_row(SQL_SELECT_DEVICE_BY_ID, params![key], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            match found {
                Some((device_text, descriptor, paired_text, wire)) => {
                    let device = decode_device_record(&device_text, descriptor, &paired_text, wire)
                        .map_err(credential_unavailable)?;
                    Ok(Some(device))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn find_device_by_wire(
        &self,
        wire: &str,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let wire = wire.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<(String, String, String, Option<String>)> = guard
                .query_row(SQL_SELECT_DEVICE_BY_WIRE, params![wire], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            match found {
                Some((device_text, descriptor, paired_text, stored_wire)) => {
                    let device =
                        decode_device_record(&device_text, descriptor, &paired_text, stored_wire)
                            .map_err(credential_unavailable)?;
                    Ok(Some(device))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_LIST_PENDING_PAIRINGS)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let rows = query
                .query_map((), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let mut pending = Vec::new();
            for row in rows {
                let (descriptor, requested_text) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                pending.push(
                    decode_pending_pairing(descriptor, &requested_text)
                        .map_err(credential_unavailable)?,
                );
            }
            Ok(pending)
        })
        .await
    }
}

impl CredentialApprovalRepository for Store {
    async fn request_approval(
        &self,
        provider: String,
        label: String,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store, so
            // they never become stored rows.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let requested_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // An already-usable pair short-circuits: re-requests record nothing.
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if usable.is_some() {
                return Ok(false);
            }
            // `INSERT OR IGNORE` keeps a previously stored pending entry: a
            // repeat request reports `false` instead of refreshing its time.
            let inserted = tx
                .execute(
                    SQL_INSERT_CREDENTIAL_PENDING_IGNORE,
                    params![provider, label, requested_text],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if inserted == 0 {
                return Ok(false);
            }
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(true)
        })
        .await
    }

    async fn approve_pending(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // A known pending entry is consumed first so a pair that is somehow
            // both pending and usable never strands its pending row. The delete
            // and the usable-ref upsert share this transaction: a crash between
            // them could otherwise strand an approval with no usable marker (or
            // vice versa), forcing the Owner to re-request and re-approve. The
            // ref id follows the same `provider:label` convention the Host uses
            // when it builds refs for assignment, so both paths name one row.
            let pending: Option<(String, String, String)> = tx
                .query_row(
                    SQL_SELECT_CREDENTIAL_PENDING,
                    params![provider, label],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if pending.is_some() {
                tx.execute(SQL_DELETE_CREDENTIAL_PENDING, params![provider, label])
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                tx.execute(
                    SQL_UPSERT_CREDENTIAL,
                    params![format!("{provider}:{label}"), provider, label,],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
                tx.commit()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                return Ok(true);
            }
            // Re-approving an already-usable pair is idempotent with no change.
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(usable.is_some())
        })
        .await
    }

    async fn is_approved(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let guard = lock_shared(&conn);
            // Pure load: one statement, no transaction. The `credential_ref`
            // row is the usable marker; pending-only pairs report `false`.
            let found: Option<(String, String, String)> = guard
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(found.is_some())
        })
        .await
    }

    async fn list_pending(
        &self,
    ) -> Result<Vec<PendingCredentialApproval>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_LIST_CREDENTIAL_PENDING)
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
            let mut pending = Vec::new();
            for row in rows {
                let (provider, label, requested_text) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                pending.push(
                    decode_pending_credential(provider, label, &requested_text)
                        .map_err(credential_unavailable)?,
                );
            }
            Ok(pending)
        })
        .await
    }
}

impl UsageRepository for Store {
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(fact.ticket.0);
            let input_column =
                encode_optional_count(fact.input_tokens).map_err(inference_unavailable)?;
            let output_column =
                encode_optional_count(fact.output_tokens).map_err(inference_unavailable)?;
            let guard = lock_shared(&conn);
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
        })
        .await
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test fixtures construct validated credential refs"
)]
mod tests {
    use super::Store;
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
            params![super::encode_id(companion.as_raw())],
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
    async fn local_id_lookup_is_correspondence_metadata_not_replay_key_replacing_local_id_uniqueness()
     {
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
        let companion_text = super::encode_id(companion.as_raw());
        let message_id = RawId::new();
        let message_text = super::encode_id(message_id);
        let round_id = RawId::new();
        let round_text = super::encode_id(round_id);
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
                    super::encode_lifecycle(CompanionLifecycle::Running),
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
            super::encode_id(device.id.0),
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
        let device_text = super::encode_id(device_id);
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
                    super::encode_id(RawId::new()),
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
        let reapproved =
            CredentialApprovalRepository::approve_pending(&store, "acme", "main").await;
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
            ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint,
            IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository as _, IntentResolution,
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
    async fn request_approval_with_intent_decides_atomically() {
        use ene_permission::{
            IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository as _,
            IntentResolution,
        };

        fn intent(id: &str) -> IntentOutcomeRecord {
            IntentOutcomeRecord {
                fingerprint: IntentFingerprint {
                    intent_id: id.to_owned(),
                    kind: String::from("register"),
                    target: String::from("credential:acme:main"),
                    base: String::from("consent-none"),
                    rationale_origin: String::from("management-surface"),
                    rationale_quote: None,
                },
                outcome: IntentOutcome::HeldByOperation,
            }
        }

        let Some(store) = open_memory().await else {
            return;
        };
        let decided = store
            .request_approval_with_intent(
                String::from("acme"),
                String::from("main"),
                intent("reg-1").fingerprint,
            )
            .await;
        assert!(
            matches!(
                decided,
                Ok(IntentResolution::Decided(ref snapshot))
                    if snapshot.outcome == IntentOutcome::HeldByOperation
            ),
            "first registration must hold, got {decided:?}"
        );
        let found = store.lookup_intent_outcome("reg-1").await;
        assert!(
            matches!(found, Ok(Some(_))),
            "the held decision must leave its replay row"
        );
        let usable = store
            .request_approval_with_intent(
                String::from("acme"),
                String::from("main"),
                intent("reg-2").fingerprint,
            )
            .await;
        assert!(
            usable.is_ok(),
            "repeat registration must decide, got {usable:?}"
        );
    }

    #[tokio::test]
    async fn complete_with_intent_decides_atomically() {
        use ene_permission::{
            ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint,
            IntentOutcome, IntentOutcomeRepository as _, IntentResolution,
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
            ConsentRecord, ConsentRepository as _, ConsentRevision, IntentFingerprint,
            IntentOutcome, IntentOutcomeRepository as _, IntentResolution, ShortcutIntentOutcome,
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
            AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _,
            InferenceTicketId,
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
        let reapprove =
            CredentialApprovalRepository::approve_pending(&second, "acme", "usable").await;
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
}
