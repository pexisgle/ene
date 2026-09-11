use std::sync::Mutex;
use std::sync::MutexGuard;

use ene_companion::{
    CommandId, CompanionId, CompanionLifecycle, CompanionTechnicalError, HistoryMessage,
    HistoryRole, ReportStatus, RoundIntentMark, UndeliveredTechnicalError,
};
use ene_credential::{
    CredentialTechnicalError, DeviceId, DeviceRecord, PendingCredentialApproval, PendingPairing,
};
use ene_inference::{InferenceTechnicalError, UsageSource};
use ene_permission::{
    CapabilityKind, ConsentRecord, ConsentRevision, IntentFingerprint, IntentOutcome,
    IntentOutcomeRecord, IntentResolution, PermissionTechnicalError,
};
use ene_presence::{
    ClientId, PresenceAttribution, PresenceGeneration, PresenceState, PresenceTechnicalError,
    ThinMoveReason,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(crate) const SQL_SELECT_ATTRIBUTION: &str =
    "SELECT state, active_client, generation FROM presence_attribution WHERE companion_id = ?1";

pub(crate) const SQL_SELECT_CONSENT: &str =
    "SELECT id, rev, provider, model, credential_id FROM consent_record WHERE capability = ?1";

pub(crate) const SQL_SELECT_CREDENTIAL: &str =
    "SELECT id, provider, label FROM credential_ref WHERE provider = ?1 AND label = ?2";

pub(crate) const SQL_INSERT_CREDENTIAL_PENDING_IGNORE: &str =
    "INSERT OR IGNORE INTO credential_pending (provider, label, requested_at) VALUES (?1, ?2, ?3)";

pub(crate) const SQL_SELECT_INTENT_OUTCOME: &str = "SELECT kind, target, base, rationale_origin, rationale_quote, outcome, mark FROM management_intent WHERE intent_id = ?1";

pub(crate) const SQL_INSERT_INTENT_OUTCOME: &str = "INSERT INTO management_intent (intent_id, kind, target, base, rationale_origin, rationale_quote, outcome, mark) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

/// Poisoning only follows a panic inside a critical section; sections here
/// perform no panicking work while holding the guard, so recovery preserves
/// the committed state. Recovering (rather than erroring) also keeps lock
/// handling out of every repository's error vocabulary.
pub(crate) fn lock_shared(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    match conn.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) fn encode_id(id: RawId) -> String {
    id.as_uuid().as_hyphenated().to_string()
}

/// The `uuid` crate is not a direct dependency, so parsing goes through
/// [`str::parse`], with the target type inferred from [`RawId::from_uuid`].
pub(crate) fn decode_id(text: &str) -> Result<RawId, String> {
    let parsed = text
        .parse()
        .map_err(|_| String::from("malformed identity text"))?;
    Ok(RawId::from_uuid(parsed))
}

pub(crate) fn encode_u64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| String::from("count out of range"))
}

pub(crate) fn decode_u64(raw: i64) -> Result<u64, String> {
    u64::try_from(raw).map_err(|_| String::from("count out of range"))
}

/// Unknown counts stay `NULL`, never zero.
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

/// Only [`RoundIntentMark::Existing`] carries a reference, stored verbatim.
pub(crate) fn encode_round_intent(intent: &RoundIntentMark) -> (&'static str, Option<&str>) {
    match intent {
        RoundIntentMark::Auto => ("auto", None),
        RoundIntentMark::New => ("new", None),
        RoundIntentMark::Existing(reference) => ("existing", Some(reference)),
    }
}

/// `None` intent means the row predates the mark (or carries no command
/// key): the caller fail-closes on replay instead of guessing. A kind and
/// reference that disagree are a malformed row, never defaulted.
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

pub(crate) fn encode_usage_source(source: UsageSource) -> &'static str {
    match source {
        UsageSource::Reported => "reported",
        UsageSource::Unknown => "unknown",
    }
}

pub(crate) fn encode_move_reason(reason: ThinMoveReason) -> &'static str {
    match reason {
        ThinMoveReason::InitialAttach => "initial_attach",
        ThinMoveReason::DisconnectObserved => "disconnect_observed",
        ThinMoveReason::RestartRecovery => "restart_recovery",
    }
}

pub(crate) fn presence_unavailable(reason: String) -> PresenceTechnicalError {
    PresenceTechnicalError::StorageUnavailable { reason }
}

pub(crate) fn companion_unavailable(reason: String) -> CompanionTechnicalError {
    CompanionTechnicalError::StorageUnavailable { reason }
}

pub(crate) fn undelivered_unavailable(reason: String) -> UndeliveredTechnicalError {
    UndeliveredTechnicalError::StorageUnavailable { reason }
}

pub(crate) fn permission_unavailable(reason: String) -> PermissionTechnicalError {
    PermissionTechnicalError::StorageUnavailable { reason }
}

pub(crate) fn credential_unavailable(reason: impl core::fmt::Display) -> CredentialTechnicalError {
    CredentialTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

pub(crate) fn inference_unavailable(reason: String) -> InferenceTechnicalError {
    InferenceTechnicalError::StorageUnavailable { reason }
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

/// Unknown kinds and missing marks are malformed rows, never guessed: the
/// caller fails closed.
pub(crate) fn decode_intent_outcome(
    outcome_text: &str,
    mark: Option<String>,
) -> Result<IntentOutcome, String> {
    match (outcome_text, mark) {
        ("stored", Some(revision)) => Ok(IntentOutcome::StoredAsRuleView { revision }),
        ("applied", _) => Ok(IntentOutcome::AppliedAsOneTime),
        ("held", _) => Ok(IntentOutcome::HeldByOperation),
        ("clarify", _) => Ok(IntentOutcome::NeedsClarification),
        ("exhausted", _) => Ok(IntentOutcome::RevisionExhausted),
        ("stale", Some(current)) => Ok(IntentOutcome::StaleBaseView { current }),
        _ => Err(String::from("malformed intent outcome")),
    }
}

/// Compares content fields only: both rows share the key by construction, so
/// the key itself carries no information.
pub(crate) fn fingerprints_match(stored: &IntentFingerprint, incoming: &IntentFingerprint) -> bool {
    stored.kind == incoming.kind
        && stored.target == incoming.target
        && stored.base == incoming.base
        && stored.rationale_origin == incoming.rationale_origin
        && stored.rationale_quote == incoming.rationale_quote
}

/// Shared by the claim check and the insert-race fallback so both answer
/// from the same rule: exact content replays, anything else clarifies.
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

/// Returns `None` when this call stored the row, or the winning row when a
/// concurrent writer committed first (cross-process only; same-process
/// writers serialize on the shared connection). Callers must NOT commit on
/// `Some`: dropping the transaction rolls back any decision writes made
/// after the pre-check, so a loser changes nothing.
pub(crate) fn insert_decided_row_tx(
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

pub(crate) fn select_intent_row_tx(
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

/// Pre-opaque rows store `NULL` for `wire` and decode to the legacy
/// continuity projection (the device identity rendering) so
/// already-provisioned clients keep resolving; new approvals always store a
/// fresh opaque projection.
pub(crate) fn decode_device_record(
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

pub(crate) fn decode_pending_pairing(
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

/// Empty or whitespace-only input is absent: callers check this before
/// touching the store, so blank pairs never become stored rows.
pub(crate) fn credential_pair_is_blank(provider: &str, label: &str) -> bool {
    provider.trim().is_empty() || label.trim().is_empty()
}

pub(crate) fn decode_pending_credential(
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

/// `None` command text means no replay key; `None` wire projection marks a
/// pre-opaque row; the incarnation appears only when both counter and random
/// are present and decode; `local_id` is correspondence metadata only.
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

/// Named fields keep column order in exactly one place:
/// [`HistoryRow::from_row`]. All three readers
/// (`lookup_local_id`, `lookup_command`, `load_timeline`) share the column
/// order through that constructor.
pub(crate) struct HistoryRow {
    message_text: String,
    round_text: String,
    role_text: String,
    body: String,
    lang: String,
    at_text: String,
    generation_raw: i64,
    command_text: Option<String>,
    stored_local_id: Option<String>,
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
            stored_local_id: row.get(8)?,
            round_wire: row.get(9)?,
            round_intent_kind: row.get(10)?,
            round_intent_ref: row.get(11)?,
            client_counter: row.get(12)?,
            client_random: row.get(13)?,
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
