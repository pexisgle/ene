use std::sync::Mutex;
use std::sync::MutexGuard;

use ene_companion::{
    ActionCertaintyWire, CommandId, CompanionId, CompanionLifecycle, CompanionTechnicalError,
    HistoryMessage, HistoryRole, ReportStatus, RoundIntentMark, TaskFact, TerminalKindWire,
    UndeliveredSource, UndeliveredTechnicalError,
};
use ene_credential::{
    CredentialTechnicalError, DeviceId, DeviceRecord, PendingCredentialApproval, PendingPairing,
};
use ene_inference::cost::CurrencyCode;
use ene_inference::pricing::PricingSnapshotRef;
use ene_inference::{InferenceTechnicalError, UsageSource};
use ene_permission::{
    CapabilityKind, ConsentRecord, ConsentRevision, ConsumerKind, IntentFingerprint, IntentOutcome,
    IntentOutcomeRecord, IntentResolution, PermissionTechnicalError, PurposeKind,
};
use ene_presence::{
    ClientId, PresenceAttribution, PresenceGeneration, PresenceState, PresenceTechnicalError,
    RelocationHint, ThinMoveReason,
};
use ene_preservation::PreservationTechnicalError;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

const SQL_SELECT_ATTRIBUTION: &str =
    "SELECT state, active_client, generation FROM presence_attribution WHERE companion_id = ?1";

const SQL_SELECT_HINT: &str =
    "SELECT last_client, recovery_destination FROM relocation_hint WHERE companion_id = ?1";

const SQL_SELECT_CONSENT: &str =
    "SELECT id, rev, provider, model, credential_id FROM consent_record WHERE capability = ?1";

pub(crate) const SQL_SELECT_CREDENTIAL: &str =
    "SELECT id, provider, label FROM credential_ref WHERE provider = ?1 AND label = ?2";

pub(crate) const SQL_INSERT_CREDENTIAL_PENDING_IGNORE: &str =
    "INSERT OR IGNORE INTO credential_pending (provider, label, requested_at) VALUES (?1, ?2, ?3)";

pub(crate) const SQL_SELECT_INTENT_OUTCOME: &str = "SELECT kind, target, base, rationale_origin, rationale_quote, outcome, mark FROM management_intent WHERE intent_id = ?1";

pub(crate) const SQL_INSERT_INTENT_OUTCOME: &str = "INSERT INTO management_intent (intent_id, kind, target, base, rationale_origin, rationale_quote, outcome, mark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

pub(crate) fn lock_shared(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    match conn.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) fn encode_id(id: RawId) -> String {
    id.as_uuid().as_hyphenated().to_string()
}

pub(crate) fn decode_id(text: &str) -> Result<RawId, String> {
    let parsed = text
        .parse()
        .map_err(|_| String::from("malformed identity text"))?;
    let raw = RawId::from_uuid(parsed);
    // Only the canonical rendering `encode_id` writes is readable: another
    // spelling (simple, braced, urn, uppercase) would be a second durable key
    // for one identity, so it is an unreadable row.
    if encode_id(raw) != text {
        return Err(String::from("malformed identity text"));
    }
    Ok(raw)
}

/// Pricing references have a text form by design: the durable row is what
/// historical cost facts join on. Malformed text never becomes a fresh or
/// default reference.
pub(crate) fn decode_pricing_reference(text: &str) -> Result<PricingSnapshotRef, String> {
    PricingSnapshotRef::from_text(text)
        .ok_or_else(|| String::from("malformed pricing snapshot reference"))
}

pub(crate) fn decode_currency(text: &str) -> Result<CurrencyCode, String> {
    CurrencyCode::from_code(text).ok_or_else(|| String::from("unknown currency code"))
}

pub(crate) fn encode_wall_clock(at: WallClockWithTz) -> String {
    at.to_rfc3339_utc()
}

pub(crate) fn decode_wall_clock(text: &str) -> Result<WallClockWithTz, String> {
    WallClockWithTz::parse_rfc3339(text).map_err(|_| String::from("malformed timestamp"))
}

pub(crate) fn encode_u64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| String::from("count out of range"))
}

pub(crate) fn decode_u64(raw: i64) -> Result<u64, String> {
    u64::try_from(raw).map_err(|_| String::from("count out of range"))
}

pub(crate) fn encode_optional_count(value: Option<u64>) -> Result<Option<i64>, String> {
    match value {
        Some(number) => Ok(Some(encode_u64(number)?)),
        None => Ok(None),
    }
}

pub(crate) fn encode_lifecycle(lifecycle: CompanionLifecycle) -> &'static str {
    match lifecycle {
        CompanionLifecycle::Running => "running",
        CompanionLifecycle::Stopped => "stopped",
        CompanionLifecycle::Deleted => "deleted",
    }
}

pub(crate) fn decode_lifecycle(text: &str) -> Result<CompanionLifecycle, String> {
    match text {
        "running" => Ok(CompanionLifecycle::Running),
        "stopped" => Ok(CompanionLifecycle::Stopped),
        "deleted" => Ok(CompanionLifecycle::Deleted),
        _ => Err(String::from("unknown companion lifecycle")),
    }
}

pub(crate) fn encode_role(role: HistoryRole) -> &'static str {
    match role {
        HistoryRole::Owner => "owner",
        HistoryRole::Companion => "companion",
    }
}

pub(crate) fn decode_role(text: &str) -> Result<HistoryRole, String> {
    match text {
        "owner" => Ok(HistoryRole::Owner),
        "companion" => Ok(HistoryRole::Companion),
        _ => Err(String::from("unknown history role")),
    }
}

pub(crate) fn encode_round_intent(intent: &RoundIntentMark) -> (&'static str, Option<&str>) {
    match intent {
        RoundIntentMark::Auto => ("auto", None),
        RoundIntentMark::New => ("new", None),
        RoundIntentMark::Existing(reference) => ("existing", Some(reference)),
    }
}

/// `None` intent means the row carries no command key (replies): the caller
/// fail-closes on replay instead of guessing. A kind and reference that
/// disagree are a malformed row, never defaulted.
pub(crate) fn decode_round_intent(
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

pub(crate) fn encode_presence_state(state: PresenceState) -> &'static str {
    match state {
        PresenceState::Present => "present",
        PresenceState::NoActive => "no_active",
        PresenceState::InTransition => "in_transition",
        PresenceState::Stopped => "stopped",
        PresenceState::RecoveryWait => "recovery_wait",
    }
}

pub(crate) fn decode_presence_state(text: &str) -> Result<PresenceState, String> {
    match text {
        "present" => Ok(PresenceState::Present),
        "no_active" => Ok(PresenceState::NoActive),
        "in_transition" => Ok(PresenceState::InTransition),
        "stopped" => Ok(PresenceState::Stopped),
        "recovery_wait" => Ok(PresenceState::RecoveryWait),
        _ => Err(String::from("unknown presence state")),
    }
}

pub(crate) fn encode_report_status(status: ReportStatus) -> &'static str {
    match status {
        ReportStatus::Pending => "pending",
        ReportStatus::Presented => "presented",
        ReportStatus::PresentationUnknown => "presentation_unknown",
    }
}

pub(crate) fn decode_report_status(text: &str) -> Result<ReportStatus, String> {
    match text {
        "pending" => Ok(ReportStatus::Pending),
        "presented" => Ok(ReportStatus::Presented),
        "presentation_unknown" => Ok(ReportStatus::PresentationUnknown),
        _ => Err(String::from("unknown report status")),
    }
}

pub(crate) fn utf8_prefix(bytes: &[u8]) -> Result<&str, String> {
    match core::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if error.error_len().is_none() => {
            core::str::from_utf8(&bytes[..error.valid_up_to()])
                .map_err(|_| String::from("malformed bounded excerpt bytes"))
        }
        Err(_) => Err(String::from("malformed bounded excerpt bytes")),
    }
}

pub(crate) const SOURCE_KIND_TASK_REVISION: &str = "task_revision";
pub(crate) const SOURCE_KIND_DELEGATION: &str = "delegation";
pub(crate) const SOURCE_KIND_ACTION_ATTEMPT: &str = "action_attempt";
pub(crate) const SOURCE_KIND_RESULT_RECORDED: &str = "result_recorded";
pub(crate) const SOURCE_KIND_RESULT_ADOPTED: &str = "result_adopted";
pub(crate) const SOURCE_KIND_TERMINAL: &str = "terminal";
pub(crate) const SOURCE_KIND_HISTORY_MESSAGE: &str = "history_message";
pub(crate) const SOURCE_KIND_ACTIVITY_RECORD: &str = "activity_record";

pub(crate) fn encode_undelivered_source(source: &UndeliveredSource) -> (String, String, String) {
    let kind;
    let id;
    let phase;
    match source {
        UndeliveredSource::TaskRecord { fact, .. } => match fact {
            TaskFact::TaskRevision { task, revision } => {
                kind = SOURCE_KIND_TASK_REVISION;
                id = encode_id(*task);
                phase = revision.to_string();
            }
            TaskFact::Delegation(delegation) => {
                kind = SOURCE_KIND_DELEGATION;
                id = encode_id(*delegation);
                phase = String::new();
            }
            TaskFact::ActionAttempt { attempt, certainty } => {
                kind = SOURCE_KIND_ACTION_ATTEMPT;
                id = encode_id(*attempt);
                phase = certainty.as_str().to_owned();
            }
            TaskFact::ResultRecorded(result) => {
                kind = SOURCE_KIND_RESULT_RECORDED;
                id = encode_id(*result);
                phase = String::new();
            }
            TaskFact::ResultAdopted(result) => {
                kind = SOURCE_KIND_RESULT_ADOPTED;
                id = encode_id(*result);
                phase = String::new();
            }
            TaskFact::Terminal { task, progress } => {
                kind = SOURCE_KIND_TERMINAL;
                id = encode_id(*task);
                phase = progress.as_str().to_owned();
            }
        },
        UndeliveredSource::HistoryMessage(message) => {
            kind = SOURCE_KIND_HISTORY_MESSAGE;
            id = encode_id(*message);
            phase = String::new();
        }
        UndeliveredSource::ActivityRecord(activity) => {
            kind = SOURCE_KIND_ACTIVITY_RECORD;
            id = encode_id(*activity);
            phase = String::new();
        }
    }
    (kind.to_owned(), id, phase)
}

const SQL_SOURCE_DELEGATION_TASK: &str = "SELECT task_id FROM delegation WHERE delegation_id = ?1";
const SQL_SOURCE_ATTEMPT_TASK: &str = "SELECT d.task_id FROM action_attempt a JOIN delegation d ON d.delegation_id = a.delegation_id WHERE a.attempt_id = ?1";
const SQL_SOURCE_RESULT_TASK: &str = "SELECT task_id FROM task_result WHERE result_id = ?1";

fn resolve_source_task(
    conn: &Connection,
    sql: &str,
    id: &str,
    what: &str,
) -> Result<RawId, String> {
    let found: Option<String> = conn
        .query_row(sql, params![id], |row| row.get(0))
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(task_text) = found else {
        return Err(format!("{what} source names no owning task"));
    };
    decode_id(&task_text)
}

/// Decodes one stored source key back to its typed form.
///
/// An unknown kind, an undecodable identity, a phase that is not the kind's
/// canonical rendering (a non-canonical revision decimal, a phase present for
/// a kind that has none, an unknown certainty, an unknown terminal phase) are
/// unreadable rows and fail closed. Delegation-, attempt-, and result-owned
/// facts resolve
/// their owning task from the canonical rows at read time (the delegation
/// row, the attempt's delegation row, the result row): the `task` field is
/// the owning task, never the source identity itself, so report composition
/// finds the task behind every fact.
pub(crate) fn decode_undelivered_source(
    conn: &Connection,
    kind: &str,
    id: &str,
    phase: &str,
) -> Result<UndeliveredSource, String> {
    let raw = decode_id(id)?;
    match kind {
        SOURCE_KIND_TASK_REVISION => {
            let revision = phase
                .parse::<u64>()
                .map_err(|_| String::from("malformed task revision source phase"))?;
            if phase != revision.to_string() {
                return Err(String::from("malformed task revision source phase"));
            }
            Ok(UndeliveredSource::TaskRecord {
                task: raw,
                fact: TaskFact::TaskRevision {
                    task: raw,
                    revision,
                },
            })
        }
        SOURCE_KIND_DELEGATION => {
            if !phase.is_empty() {
                return Err(String::from("malformed delegation source phase"));
            }
            let task = resolve_source_task(conn, SQL_SOURCE_DELEGATION_TASK, id, "delegation")?;
            Ok(UndeliveredSource::TaskRecord {
                task,
                fact: TaskFact::Delegation(raw),
            })
        }
        SOURCE_KIND_ACTION_ATTEMPT => {
            let certainty = ActionCertaintyWire::from_name(phase)
                .ok_or_else(|| String::from("unknown action certainty source phase"))?;
            let task = resolve_source_task(conn, SQL_SOURCE_ATTEMPT_TASK, id, "action attempt")?;
            Ok(UndeliveredSource::TaskRecord {
                task,
                fact: TaskFact::ActionAttempt {
                    attempt: raw,
                    certainty,
                },
            })
        }
        SOURCE_KIND_RESULT_RECORDED => {
            if !phase.is_empty() {
                return Err(String::from("malformed recorded result source phase"));
            }
            let task = resolve_source_task(conn, SQL_SOURCE_RESULT_TASK, id, "recorded result")?;
            Ok(UndeliveredSource::TaskRecord {
                task,
                fact: TaskFact::ResultRecorded(raw),
            })
        }
        SOURCE_KIND_RESULT_ADOPTED => {
            if !phase.is_empty() {
                return Err(String::from("malformed adopted result source phase"));
            }
            let task = resolve_source_task(conn, SQL_SOURCE_RESULT_TASK, id, "adopted result")?;
            Ok(UndeliveredSource::TaskRecord {
                task,
                fact: TaskFact::ResultAdopted(raw),
            })
        }
        SOURCE_KIND_TERMINAL => {
            let progress = TerminalKindWire::from_name(phase)
                .ok_or_else(|| String::from("unknown terminal source phase"))?;
            Ok(UndeliveredSource::TaskRecord {
                task: raw,
                fact: TaskFact::Terminal {
                    task: raw,
                    progress,
                },
            })
        }
        SOURCE_KIND_HISTORY_MESSAGE => {
            if !phase.is_empty() {
                return Err(String::from("malformed history message source phase"));
            }
            Ok(UndeliveredSource::HistoryMessage(raw))
        }
        SOURCE_KIND_ACTIVITY_RECORD => {
            if !phase.is_empty() {
                return Err(String::from("malformed activity record source phase"));
            }
            Ok(UndeliveredSource::ActivityRecord(raw))
        }
        _ => Err(String::from("unknown undelivered source kind")),
    }
}

pub(crate) fn encode_usage_source(source: UsageSource) -> &'static str {
    match source {
        UsageSource::Reported => "reported",
        UsageSource::Unknown => "unknown",
    }
}

pub(crate) fn decode_usage_source(text: &str) -> Result<UsageSource, String> {
    match text {
        "reported" => Ok(UsageSource::Reported),
        "unknown" => Ok(UsageSource::Unknown),
        _ => Err(String::from("unknown usage source")),
    }
}

/// Consumer/purpose storage vocabulary is owned by `ene-permission`; unknown
/// stored names are unreadable rows and fail closed on decode.
pub(crate) fn decode_consumer(text: &str) -> Result<ConsumerKind, String> {
    ConsumerKind::from_name(text).ok_or_else(|| String::from("unknown inference consumer"))
}

pub(crate) fn decode_purpose(text: &str) -> Result<PurposeKind, String> {
    PurposeKind::from_name(text).ok_or_else(|| String::from("unknown inference purpose"))
}

pub(crate) fn encode_move_reason(reason: ThinMoveReason) -> &'static str {
    match reason {
        ThinMoveReason::InitialAttach => "initial_attach",
        ThinMoveReason::DisconnectObserved => "disconnect_observed",
        ThinMoveReason::RestartRecovery => "restart_recovery",
        ThinMoveReason::Stop => "stop",
    }
}

pub(crate) fn presence_unavailable(reason: impl core::fmt::Display) -> PresenceTechnicalError {
    PresenceTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn companion_unavailable(reason: impl core::fmt::Display) -> CompanionTechnicalError {
    CompanionTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn undelivered_unavailable(
    reason: impl core::fmt::Display,
) -> UndeliveredTechnicalError {
    UndeliveredTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn permission_unavailable(reason: impl core::fmt::Display) -> PermissionTechnicalError {
    PermissionTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn credential_unavailable(reason: impl core::fmt::Display) -> CredentialTechnicalError {
    CredentialTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn inference_unavailable(reason: impl core::fmt::Display) -> InferenceTechnicalError {
    InferenceTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn preservation_storage(_: rusqlite::Error) -> PreservationTechnicalError {
    PreservationTechnicalError::StorageUnavailable
}

pub(crate) fn preservation_corrupt() -> PreservationTechnicalError {
    PreservationTechnicalError::CorruptState
}

pub(crate) fn select_consent(
    conn: &Connection,
    capability: CapabilityKind,
) -> Result<Option<ConsentRecord>, String> {
    let found: Option<(String, i64, String, String, String)> = conn
        .query_row(SQL_SELECT_CONSENT, params![capability.as_str()], |row| {
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
    found
        .map(|(id, rev_raw, provider, model, credential_id)| {
            decode_consent(capability, id, rev_raw, provider, model, credential_id)
        })
        .transpose()
}

pub(crate) fn decode_consent(
    capability: CapabilityKind,
    id: String,
    rev_raw: i64,
    provider: String,
    model: String,
    credential_id: String,
) -> Result<ConsentRecord, String> {
    let number = decode_u64(rev_raw)?;
    Ok(ConsentRecord {
        capability,
        id,
        rev: ConsentRevision::from_u64(number),
        provider,
        model,
        credential_id,
    })
}

pub(crate) fn encode_intent_outcome(outcome: &IntentOutcome) -> (&'static str, Option<&str>) {
    match outcome {
        IntentOutcome::StoredAsRuleView { revision } => ("stored", Some(revision)),
        IntentOutcome::AppliedAsOneTime => ("applied", None),
        IntentOutcome::HeldByOperation => ("held", None),
        IntentOutcome::NeedsClarification => ("clarify", None),
        IntentOutcome::RevisionExhausted => ("exhausted", None),
        IntentOutcome::StaleBaseView { current } => ("stale", Some(current)),
    }
}

pub(crate) fn decode_intent_outcome(
    outcome_text: &str,
    mark: Option<String>,
) -> Result<IntentOutcome, String> {
    match (outcome_text, mark) {
        ("stored", Some(revision)) => Ok(IntentOutcome::StoredAsRuleView { revision }),
        ("applied", None) => Ok(IntentOutcome::AppliedAsOneTime),
        ("held", None) => Ok(IntentOutcome::HeldByOperation),
        ("clarify", None) => Ok(IntentOutcome::NeedsClarification),
        ("exhausted", None) => Ok(IntentOutcome::RevisionExhausted),
        ("stale", Some(current)) => Ok(IntentOutcome::StaleBaseView { current }),
        _ => Err(String::from("malformed intent outcome")),
    }
}

pub(crate) fn fingerprints_match(stored: &IntentFingerprint, incoming: &IntentFingerprint) -> bool {
    stored.kind == incoming.kind
        && stored.target == incoming.target
        && stored.base == incoming.base
        && stored.rationale_origin == incoming.rationale_origin
        && stored.rationale_quote == incoming.rationale_quote
}

/// Shared by every write-once claim check: an existing row decides, and exact
/// content replays while anything else clarifies.
pub(crate) fn replay_or_conflict<T>(
    stored: IntentOutcomeRecord,
    fingerprint: &IntentFingerprint,
) -> IntentResolution<T> {
    if fingerprints_match(&stored.fingerprint, fingerprint) {
        IntentResolution::Replay(stored)
    } else {
        IntentResolution::Conflict(stored)
    }
}

/// Stores the decided row for an intent whose write-once claim already ran in
/// the same `BEGIN IMMEDIATE` transaction. A constraint violation here is torn
/// state, never a lost race: the write lock is held from the claim through
/// this insert, so no other writer can commit the key in between.
pub(crate) fn insert_decided_row_tx(
    tx: &Transaction<'_>,
    fingerprint: &IntentFingerprint,
    outcome: &IntentOutcome,
) -> Result<(), String> {
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
        Ok(_) => Ok(()),
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
        {
            Err(String::from("intent row appeared after write-once claim"))
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn decode_intent_outcome_row(
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

pub(crate) fn select_intent_row(
    conn: &Connection,
    intent_id: &str,
) -> Result<Option<IntentOutcomeRecord>, String> {
    let found: Option<IntentOutcomeRow> = conn
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

/// Decodes one `paired_device` row. The wire projection is non-null: an
/// approval always stores a freshly minted opaque wire, so a row without one
/// is unreadable rather than a legacy identity rendering.
pub(crate) fn decode_device_record(
    device_text: &str,
    descriptor: String,
    paired_text: &str,
    wire: String,
) -> Result<DeviceRecord, String> {
    let paired_at = decode_wall_clock(paired_text)
        .map_err(|_| String::from("malformed device pairing timestamp"))?;
    Ok(DeviceRecord {
        id: DeviceId(decode_id(device_text)?),
        wire,
        descriptor,
        paired_at,
    })
}

pub(crate) fn decode_pending_pairing(
    pending_id: String,
    descriptor: String,
    requested_text: &str,
    origin_connection: String,
) -> Result<PendingPairing, String> {
    let requested_at = decode_wall_clock(requested_text)
        .map_err(|_| String::from("malformed pairing request timestamp"))?;
    Ok(PendingPairing {
        pending_id,
        descriptor,
        requested_at,
        origin_connection,
    })
}

pub(crate) fn credential_pair_is_blank(provider: &str, label: &str) -> bool {
    provider.trim().is_empty() || label.trim().is_empty()
}

pub(crate) fn decode_pending_credential(
    provider: String,
    label: String,
    requested_text: &str,
) -> Result<PendingCredentialApproval, String> {
    let requested_at = decode_wall_clock(requested_text)
        .map_err(|_| String::from("malformed credential approval timestamp"))?;
    Ok(PendingCredentialApproval {
        provider,
        label,
        requested_at,
    })
}

/// `None` command text means no replay key; `None` wire projection is
/// unreadable stored state (every current writer persists one); the
/// incarnation appears only when both counter and random are present and
/// decode.
pub(crate) fn decode_history_message(
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
        round_wire,
        round_intent_kind,
        round_intent_ref,
        client_counter,
        client_random,
    } = row;
    let at =
        decode_wall_clock(&at_text).map_err(|_| String::from("malformed timeline timestamp"))?;
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
    })
}

pub(crate) fn select_attribution(
    conn: &Connection,
    key: &str,
) -> Result<Option<PresenceAttribution>, String> {
    let found: Option<(String, Option<String>, i64)> = conn
        .query_row(SQL_SELECT_ATTRIBUTION, params![key], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(|error| error.to_string())?;
    found
        .map(|(state_text, active_text, generation_raw)| {
            decode_attribution(
                decode_id(key)?,
                &state_text,
                active_text.as_deref(),
                generation_raw,
            )
        })
        .transpose()
}

pub(crate) fn decode_attribution(
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

pub(crate) fn select_hint(conn: &Connection, key: &str) -> Result<Option<RelocationHint>, String> {
    let found: Option<(Option<String>, Option<String>)> = conn
        .query_row(SQL_SELECT_HINT, params![key], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(|error| error.to_string())?;
    found
        .map(|(last_text, destination_text)| {
            decode_hint(key, last_text.as_deref(), destination_text.as_deref())
        })
        .transpose()
}

pub(crate) fn decode_hint(
    companion_text: &str,
    last_text: Option<&str>,
    destination_text: Option<&str>,
) -> Result<RelocationHint, String> {
    let decode_client = |text: Option<&str>| -> Result<Option<ClientId>, String> {
        text.map(|value| decode_id(value).map(ClientId::from_raw))
            .transpose()
    };
    Ok(RelocationHint {
        companion: decode_id(companion_text)?,
        last_client: decode_client(last_text)?,
        recovery_destination: decode_client(destination_text)?,
    })
}

/// Named fields keep column order in exactly one place:
/// [`HistoryRow::from_row`]. The readers (`lookup_command`, `load_message`,
/// `load_timeline`, `load_recent_timeline`, and `append_history`'s command
/// lookup) share the column order through that constructor.
pub(crate) struct HistoryRow {
    message_text: String,
    round_text: String,
    role_text: String,
    body: String,
    lang: String,
    at_text: String,
    generation_raw: i64,
    command_text: Option<String>,
    round_wire: Option<String>,
    round_intent_kind: Option<String>,
    round_intent_ref: Option<String>,
    client_counter: Option<i64>,
    client_random: Option<i64>,
}

impl HistoryRow {
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_text: row.get(0)?,
            round_text: row.get(1)?,
            role_text: row.get(2)?,
            body: row.get(3)?,
            lang: row.get(4)?,
            at_text: row.get(5)?,
            generation_raw: row.get(6)?,
            command_text: row.get(7)?,
            round_wire: row.get(8)?,
            round_intent_kind: row.get(9)?,
            round_intent_ref: row.get(10)?,
            client_counter: row.get(11)?,
            client_random: row.get(12)?,
        })
    }
}

pub(crate) type IntentOutcomeRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);
