use std::sync::Arc;

use ene_inference::cost::{TokenRate, project_cost};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRecord, InferenceAttemptRepository,
    InferenceTechnicalError, TaskAgentAttemptPremise, UsageCostRecord, UsageFact, UsageRepository,
    UsageSource,
};
use ene_permission::{CapabilityKind, ConsumerKind};
use ene_preservation::ErasureConditionRef;
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_consumer, decode_currency, decode_id, decode_pricing_reference, decode_purpose,
    decode_u64, decode_wall_clock, encode_consumer, encode_currency, encode_id,
    encode_optional_count, encode_pricing_reference, encode_purpose, encode_u64,
    encode_usage_source, encode_wall_clock, inference_unavailable, lock_shared, select_consent,
};
use crate::credential::SQL_SELECT_SET_REV;
use crate::run_blocking;

const SQL_INSERT_ATTEMPT: &str = "INSERT INTO inference_attempt (ticket, capability, consumer, purpose, consent_id, consent_rev, credential_set_rev, provider, model, started_at, delegation_id, task_id, task_revision, data_use_count, pricing_snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

const SQL_INSERT_DATA_USE: &str =
    "INSERT INTO inference_attempt_data_use (ticket, ordinal, source) VALUES (?1, ?2, ?3)";

const SQL_SELECT_DATA_USE: &str =
    "SELECT ordinal, source FROM inference_attempt_data_use WHERE ticket = ?1 ORDER BY ordinal";

const SQL_SELECT_ATTEMPT: &str = "SELECT capability, consumer, purpose, provider, model, delegation_id, task_id, task_revision, data_use_count FROM inference_attempt WHERE ticket = ?1";

const SQL_SELECT_ATTEMPT_TICKET: &str = "SELECT ticket FROM inference_attempt WHERE ticket = ?1";

const SQL_SELECT_DELEGATION_PREMISE: &str =
    "SELECT task_id, task_revision FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_TASK_STATE: &str = "SELECT revision, progress FROM task WHERE task_id = ?1";

/// The delegation's final result row, i.e. its execution seal. The row's
/// existence — never a liveness or completion flag — refuses new claims.
const SQL_SELECT_DELEGATION_RESULT: &str =
    "SELECT result_id FROM task_result WHERE delegation_id = ?1";

const SQL_INSERT_USAGE: &str = "INSERT INTO usage_fact (ticket, provider, model, input_tokens, cached_input_tokens, output_tokens, source, pricing_snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(ticket) DO NOTHING";

const SQL_SELECT_ATTEMPT_ROUTE: &str =
    "SELECT provider, model, pricing_snapshot FROM inference_attempt WHERE ticket = ?1";

const SQL_SELECT_USAGE_COST: &str = "SELECT provider, model, input_tokens, cached_input_tokens, output_tokens, source, pricing_snapshot FROM usage_fact WHERE ticket = ?1";

/// One reviewed revision is one immutable row per `(provider, model)`; both
/// the publish path and the read path select the same full shape.
const SQL_SELECT_PRICING_BY_ROUTE: &str = "SELECT id, provider, model, currency, input_rate, cached_input_rate, output_rate, effective_at, source_revision FROM pricing_snapshot WHERE provider = ?1 AND model = ?2 AND source_revision = ?3";

const SQL_SELECT_PRICING_BY_ID: &str = "SELECT id, provider, model, currency, input_rate, cached_input_rate, output_rate, effective_at, source_revision FROM pricing_snapshot WHERE id = ?1";

const SQL_INSERT_PRICING: &str = "INSERT INTO pricing_snapshot (id, provider, model, currency, input_rate, cached_input_rate, output_rate, effective_at, source_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

impl InferenceAttemptRepository for Store {
    async fn begin_inference_attempt(
        &self,
        attempt: InferenceAttempt,
    ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            // The consumer and its correlation are one premise: a Task Agent
            // attempt without one would skip the delegation/revision compare,
            // and a non-Task-Agent attempt with one would persist a
            // correlation it has no right to. The public trait is callable
            // directly, so the claim enforces the pairing itself instead of
            // trusting `AuthorizedInference`'s construction path.
            if (attempt.consumer == ConsumerKind::TaskAgent) != attempt.task_agent.is_some() {
                return Err(inference_unavailable(String::from(
                    "task agent consumer and correlation disagree",
                )));
            }
            let rev_raw =
                encode_u64(attempt.expected_consent.1.as_u64()).map_err(inference_unavailable)?;
            let credential_set_raw = encode_u64(attempt.expected_credential_set.as_u64())
                .map_err(inference_unavailable)?;
            let ticket_text = encode_id(attempt.ticket.0);
            let correlation = encode_task_agent(attempt.task_agent.as_ref())?;
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            // Duplicate detection is the first check in the transaction: a
            // re-claimed ticket is a duplicate no matter how the consent,
            // credential set, delegation, or Task revision moved since the
            // first claim, so the answer is always `Stale` instead of
            // depending on which premise check happens to fail first. This
            // also keeps the design rule "a duplicate claim never sends
            // twice" independent of the premise state.
            let already_claimed: Option<String> = tx
                .query_row(SQL_SELECT_ATTEMPT_TICKET, params![ticket_text], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            if already_claimed.is_some() {
                return Ok(AttemptBeginOutcome::Stale);
            }
            // The linearization point: read, compare, and claim share one short
            // transaction that never spans provider I/O. A mutation that
            // committed first fails the compare (no byte leaves); a mutation
            // that commits after only affects result adoption, never the fact
            // that this attempt started under a verified premise.
            let current = select_consent(&tx, attempt.capability).map_err(inference_unavailable)?;
            let current_matches = current.as_ref().is_some_and(|record| {
                record.id == attempt.expected_consent.0
                    && record.rev.as_u64() == attempt.expected_consent.1.as_u64()
            });
            if !current_matches {
                return Ok(AttemptBeginOutcome::Stale);
            }
            // The credential-set premise rides the same claim transaction:
            // a prompt scrubbed before a credential became registered must
            // not reach the provider, even though the consent still holds.
            let stored_set: i64 = tx
                .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let current_set = decode_u64(stored_set).map_err(inference_unavailable)?;
            if current_set != attempt.expected_credential_set.as_u64() {
                return Ok(AttemptBeginOutcome::Stale);
            }
            // Task Agent turns add the delegation/task premise to the same
            // atomic compare: a steering forward or a gone delegation row
            // refuses the start before any provider I/O, and the three
            // conditions keep the shared consent slot from starting a turn
            // for an already-advanced Task.
            match check_task_agent_premise(&tx, attempt.task_agent.as_ref())? {
                TaskPremiseCheck::Current => {}
                TaskPremiseCheck::Stale => {
                    return Ok(AttemptBeginOutcome::TaskPremiseStale);
                }
            }
            // The data-use currentness compare rides the same atomic interval:
            // a condition that committed first is seen here and holds the
            // send (no attempt row, no provider byte), and a condition that
            // commits after only affects already-started uses. The query reads
            // the canonical store itself, so an empty active set is a genuine
            // "not covered" answer rather than a default.
            match check_data_use_currentness(&tx, &correlation.data_use)? {
                DataUseCheck::Clear => {}
                DataUseCheck::Covered(_condition) => {
                    return Ok(AttemptBeginOutcome::DataUseHeld);
                }
            }
            let data_use_count =
                encode_u64(correlation.data_use.len() as u64).map_err(inference_unavailable)?;
            // The reviewed pricing snapshot is published and bound in the
            // same claim transaction as the attempt (usage-cost-cap §9): the
            // ticket's cost fact can only reference the rate this attempt was
            // admitted under, and a catalog revision that is not current
            // here never rewrites an already-published row.
            let pricing_reference = match attempt.pricing.as_ref() {
                None => None,
                Some(snapshot) => {
                    if snapshot.provider != attempt.provider || snapshot.model != attempt.model {
                        return Err(inference_unavailable(String::from(
                            "pricing snapshot route disagrees with the attempt route",
                        )));
                    }
                    Some(publish_pricing_snapshot(&tx, snapshot)?)
                }
            };
            let started_text = WallClockWithTz::now().to_rfc3339();
            match tx.execute(
                SQL_INSERT_ATTEMPT,
                params![
                    ticket_text,
                    attempt.capability.as_str(),
                    encode_consumer(attempt.consumer),
                    encode_purpose(attempt.purpose),
                    attempt.expected_consent.0,
                    rev_raw,
                    credential_set_raw,
                    attempt.provider,
                    attempt.model,
                    started_text,
                    correlation.delegation,
                    correlation.task,
                    correlation.task_revision,
                    data_use_count,
                    pricing_reference,
                ],
            ) {
                Ok(_) => {}
                // The explicit pre-check already answered a duplicate, so this
                // constraint violation only covers a writer that committed
                // between that read and this insert (cross-process): stale
                // (never send twice), never a storage error.
                Err(error)
                    if error.sqlite_error_code()
                        == Some(rusqlite::ErrorCode::ConstraintViolation) =>
                {
                    return Ok(AttemptBeginOutcome::Stale);
                }
                Err(error) => return Err(inference_unavailable(error.to_string())),
            }
            // The ordered source correlation lands in the same transaction as
            // the attempt row and its count: a crash can never leave a claimed
            // send whose logical-input provenance is unknown.
            for (ordinal, source) in correlation.data_use.iter().enumerate() {
                let ordinal_raw = encode_u64(ordinal as u64).map_err(inference_unavailable)?;
                tx.execute(
                    SQL_INSERT_DATA_USE,
                    params![ticket_text, ordinal_raw, source],
                )
                .map_err(|error| inference_unavailable(error.to_string()))?;
            }
            tx.commit()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            Ok(AttemptBeginOutcome::Started)
        })
        .await
    }

    async fn load_inference_attempt(
        &self,
        ticket: ene_inference::InferenceTicketId,
    ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(ticket.0);
            let guard = lock_shared(&conn);
            let found: Option<RawAttempt> = guard
                .query_row(SQL_SELECT_ATTEMPT, params![ticket_text], raw_attempt_row)
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            found
                .map(|raw| decode_attempt_record(&guard, ticket, raw))
                .transpose()
        })
        .await
    }
}

/// The encoded Task Agent correlation group. The whole group is present or
/// absent together; a partial group can never be written. `data_use` is empty
/// exactly for a non-Task-Agent attempt; a Task Agent attempt whose logical
/// input names no canonical source is refused instead of being recorded as
/// "no use" (its purpose entry always provides at least one source).
struct EncodedCorrelation {
    delegation: Option<String>,
    task: Option<String>,
    task_revision: Option<i64>,
    data_use: Vec<String>,
}

fn encode_task_agent(
    premise: Option<&TaskAgentAttemptPremise>,
) -> Result<EncodedCorrelation, InferenceTechnicalError> {
    match premise {
        None => Ok(EncodedCorrelation {
            delegation: None,
            task: None,
            task_revision: None,
            data_use: Vec::new(),
        }),
        Some(premise) => {
            if premise.data_use.is_empty() {
                return Err(inference_unavailable(String::from(
                    "task agent attempt carries no data-use correlation",
                )));
            }
            Ok(EncodedCorrelation {
                delegation: Some(encode_id(premise.delegation)),
                task: Some(encode_id(premise.task)),
                task_revision: Some(
                    encode_u64(premise.task_revision.as_u64()).map_err(inference_unavailable)?,
                ),
                data_use: premise
                    .data_use
                    .iter()
                    .map(|source| encode_id(*source))
                    .collect(),
            })
        }
    }
}

enum DataUseCheck {
    Clear,
    Covered(ErasureConditionRef),
}

/// Compares the attempt's canonical source correlation against the canonical
/// current erasure-condition store, inside the claim transaction.
///
/// Every source is probed against the condition source correlation with a
/// bounded `LIMIT 1` lookup; any covering condition is [`DataUseCheck::Covered`]
/// and the claim refuses. An empty probe result across all sources is the
/// authoritative "not covered" — there is no sentinel and no default. A
/// malformed stored identity is a technical error (fail closed), never a
/// silent "not covering". Structural corruption, including orphan sources,
/// fails closed through the shared closure-aware preservation query.
fn check_data_use_currentness(
    tx: &rusqlite::Transaction<'_>,
    data_use: &[String],
) -> Result<DataUseCheck, InferenceTechnicalError> {
    for source in data_use {
        if let Some(condition) = crate::preservation::covering_condition(tx, source)
            .map_err(|error| inference_unavailable(error.to_string()))?
        {
            return Ok(DataUseCheck::Covered(condition));
        }
    }
    Ok(DataUseCheck::Clear)
}

enum TaskPremiseCheck {
    Current,
    Stale,
}

/// Task Agent premise compare inside the claim transaction.
///
/// The five canonical AU14 conditions: (1) the delegation row exists, (2) its
/// stated `(task, revision)` equals the premise, (3) the current Task row is
/// still at the relied revision, (4) the current `task.progress` is
/// non-terminal, and (5) the delegation is not sealed (no `task_result` row).
/// Missing rows and (3)(4)(5) mismatches are [`TaskPremiseCheck::Stale`]
/// (domain stale, no write, no provider I/O); a disagreement (2), an unknown
/// stored progress name, and malformed stored values are technical errors,
/// never a fabricated stale. The inference side imports no Task lifecycle
/// type: terminal progress and the execution seal are both refused as a task
/// premise mismatch, and the Task side re-reads to explain which one.
fn check_task_agent_premise(
    tx: &rusqlite::Transaction<'_>,
    premise: Option<&TaskAgentAttemptPremise>,
) -> Result<TaskPremiseCheck, InferenceTechnicalError> {
    let Some(premise) = premise else {
        return Ok(TaskPremiseCheck::Current);
    };
    let stored: Option<(String, i64)> = tx
        .query_row(
            SQL_SELECT_DELEGATION_PREMISE,
            params![encode_id(premise.delegation)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some((delegation_task_text, delegation_revision_raw)) = stored else {
        return Ok(TaskPremiseCheck::Stale);
    };
    let delegation_task = decode_id(&delegation_task_text).map_err(inference_unavailable)?;
    let delegation_revision = decode_u64(delegation_revision_raw).map_err(inference_unavailable)?;
    if delegation_task != premise.task || delegation_revision != premise.task_revision.as_u64() {
        return Err(inference_unavailable(String::from(
            "delegation correlation disagrees with the attempt premise",
        )));
    }
    let current: Option<(i64, Option<String>)> = tx
        .query_row(
            SQL_SELECT_TASK_STATE,
            params![encode_id(premise.task)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some((current_raw, progress_raw)) = current else {
        return Ok(TaskPremiseCheck::Stale);
    };
    let current_revision = decode_u64(current_raw).map_err(inference_unavailable)?;
    if current_revision != premise.task_revision.as_u64() {
        return Ok(TaskPremiseCheck::Stale);
    }
    let progress_text = progress_raw
        .ok_or_else(|| inference_unavailable(String::from("task progress is missing")))?;
    let progress = ene_task::TaskProgress::from_name(&progress_text)
        .ok_or_else(|| inference_unavailable(String::from("unknown task progress")))?;
    if progress.is_terminal() {
        return Ok(TaskPremiseCheck::Stale);
    }
    let sealed: Option<String> = tx
        .query_row(
            SQL_SELECT_DELEGATION_RESULT,
            params![encode_id(premise.delegation)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    if sealed.is_some() {
        return Ok(TaskPremiseCheck::Stale);
    }
    Ok(TaskPremiseCheck::Current)
}

/// One stored reviewed rate, exactly as `pricing_snapshot` holds it.
struct RawPricing {
    id: String,
    provider: String,
    model: String,
    currency: String,
    input_rate: i64,
    cached_input_rate: i64,
    output_rate: i64,
    effective_at: String,
    source_revision: i64,
}

fn raw_pricing_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPricing> {
    Ok(RawPricing {
        id: row.get(0)?,
        provider: row.get(1)?,
        model: row.get(2)?,
        currency: row.get(3)?,
        input_rate: row.get(4)?,
        cached_input_rate: row.get(5)?,
        output_rate: row.get(6)?,
        effective_at: row.get(7)?,
        source_revision: row.get(8)?,
    })
}

/// Decodes one stored rate row into the reviewed snapshot it represents.
///
/// The reference is the text the row is keyed by; the caller verifies it
/// against the content-derived reference, so a row edited after publication
/// is unreadable rather than silently repricing history.
fn decode_pricing(raw: RawPricing) -> Result<PricingSnapshot, InferenceTechnicalError> {
    Ok(PricingSnapshot {
        provider: raw.provider,
        model: raw.model,
        currency: decode_currency(&raw.currency).map_err(inference_unavailable)?,
        input_rate: TokenRate::from_micros_per_million(
            decode_u64(raw.input_rate).map_err(inference_unavailable)?,
        ),
        cached_input_rate: TokenRate::from_micros_per_million(
            decode_u64(raw.cached_input_rate).map_err(inference_unavailable)?,
        ),
        output_rate: TokenRate::from_micros_per_million(
            decode_u64(raw.output_rate).map_err(inference_unavailable)?,
        ),
        effective_at: decode_wall_clock(&raw.effective_at).map_err(inference_unavailable)?,
        source_revision: PricingCatalogRevision::new(
            decode_u64(raw.source_revision).map_err(inference_unavailable)?,
        ),
    })
}

/// Publishes the resolved snapshot under its content-derived reference and
/// returns the reference text the attempt binds.
///
/// Publication is idempotent for one `(provider, model, revision)`: an
/// existing row must have exactly the resolved content and reference.
/// Anything else means the stored revision is not the reviewed one, so the
/// claim fails closed instead of binding a rate the catalog never published.
fn publish_pricing_snapshot(
    tx: &rusqlite::Transaction<'_>,
    snapshot: &PricingSnapshot,
) -> Result<String, InferenceTechnicalError> {
    let revision = encode_u64(snapshot.source_revision.as_u64()).map_err(inference_unavailable)?;
    let reference = encode_pricing_reference(snapshot.reference());
    let stored: Option<RawPricing> = tx
        .query_row(
            SQL_SELECT_PRICING_BY_ROUTE,
            params![snapshot.provider, snapshot.model, revision],
            raw_pricing_row,
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some(raw) = stored else {
        tx.execute(
            SQL_INSERT_PRICING,
            params![
                reference,
                snapshot.provider,
                snapshot.model,
                encode_currency(snapshot.currency),
                encode_u64(snapshot.input_rate.micros_per_million())
                    .map_err(inference_unavailable)?,
                encode_u64(snapshot.cached_input_rate.micros_per_million())
                    .map_err(inference_unavailable)?,
                encode_u64(snapshot.output_rate.micros_per_million())
                    .map_err(inference_unavailable)?,
                encode_wall_clock(snapshot.effective_at),
                revision,
            ],
        )
        .map_err(|error| inference_unavailable(error.to_string()))?;
        return Ok(reference);
    };
    let stored_reference = raw.id.clone();
    let decoded = decode_pricing(raw)?;
    if stored_reference != reference || decoded != *snapshot {
        return Err(inference_unavailable(String::from(
            "stored pricing snapshot disagrees with the reviewed revision",
        )));
    }
    Ok(reference)
}

/// One stored usage fact row, exactly as `usage_fact` holds it.
struct RawUsageRow {
    provider: String,
    model: String,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    source: String,
    pricing_snapshot: Option<String>,
}

fn raw_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawUsageRow> {
    Ok(RawUsageRow {
        provider: row.get(0)?,
        model: row.get(1)?,
        input_tokens: row.get(2)?,
        cached_input_tokens: row.get(3)?,
        output_tokens: row.get(4)?,
        source: row.get(5)?,
        pricing_snapshot: row.get(6)?,
    })
}

/// Decodes a stored usage fact, refusing rows whose shape disagrees with
/// their source.
///
/// Unknown keeps every count `NULL` and Reported keeps all three with cached
/// input a subset of input; anything else is unreadable. Zero-filling a
/// disagreement would fabricate usage.
fn decode_usage_fact(
    ticket: ene_inference::InferenceTicketId,
    raw: &RawUsageRow,
) -> Result<UsageFact, InferenceTechnicalError> {
    let input_tokens = raw
        .input_tokens
        .map(decode_u64)
        .transpose()
        .map_err(inference_unavailable)?;
    let cached_input_tokens = raw
        .cached_input_tokens
        .map(decode_u64)
        .transpose()
        .map_err(inference_unavailable)?;
    let output_tokens = raw
        .output_tokens
        .map(decode_u64)
        .transpose()
        .map_err(inference_unavailable)?;
    let source = match raw.source.as_str() {
        "reported" => UsageSource::Reported,
        "unknown" => UsageSource::Unknown,
        _ => {
            return Err(inference_unavailable(String::from(
                "unknown usage source in usage fact",
            )));
        }
    };
    let fact = UsageFact {
        ticket,
        provider: raw.provider.clone(),
        model: raw.model.clone(),
        input_tokens,
        cached_input_tokens,
        output_tokens,
        source,
    };
    match (
        fact.source,
        input_tokens,
        cached_input_tokens,
        output_tokens,
    ) {
        (UsageSource::Unknown, None, None, None) => {}
        (UsageSource::Reported, Some(input), Some(cached), Some(_)) if cached <= input => {}
        _ => {
            return Err(inference_unavailable(String::from(
                "stored usage fact shape is inconsistent",
            )));
        }
    }
    Ok(fact)
}

/// Reads the pricing snapshot a usage fact is bound to.
///
/// A dangling reference, a row whose content no longer derives its own
/// reference, or a route that disagrees with the usage attribution is a
/// technical error: the cost fact must never be projected from a rate that
/// does not belong to the ticket.
fn load_pricing_snapshot(
    conn: &Connection,
    reference_text: &str,
    provider: &str,
    model: &str,
) -> Result<PricingSnapshot, InferenceTechnicalError> {
    let reference = decode_pricing_reference(reference_text).map_err(inference_unavailable)?;
    let stored: Option<RawPricing> = conn
        .query_row(
            SQL_SELECT_PRICING_BY_ID,
            params![encode_pricing_reference(reference)],
            raw_pricing_row,
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some(raw) = stored else {
        return Err(inference_unavailable(String::from(
            "usage fact references a missing pricing snapshot",
        )));
    };
    let stored_reference = raw.id.clone();
    let snapshot = decode_pricing(raw)?;
    if stored_reference != encode_pricing_reference(reference) || snapshot.reference() != reference
    {
        return Err(inference_unavailable(String::from(
            "stored pricing snapshot does not match its reference",
        )));
    }
    if snapshot.provider != provider || snapshot.model != model {
        return Err(inference_unavailable(String::from(
            "pricing snapshot route disagrees with the usage attribution",
        )));
    }
    Ok(snapshot)
}

struct RawAttempt {
    capability: String,
    consumer: Option<String>,
    purpose: Option<String>,
    provider: String,
    model: String,
    delegation_id: Option<String>,
    task_id: Option<String>,
    task_revision: Option<i64>,
    data_use_count: Option<i64>,
}

fn raw_attempt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAttempt> {
    Ok(RawAttempt {
        capability: row.get(0)?,
        consumer: row.get(1)?,
        purpose: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        delegation_id: row.get(5)?,
        task_id: row.get(6)?,
        task_revision: row.get(7)?,
        data_use_count: row.get(8)?,
    })
}

/// Reads and validates one attempt's ordered source correlation.
///
/// The attempt row's count is the expected child count: a missing count, a
/// child-count disagreement, or a non-contiguous ordinal sequence is a
/// corrupted correlation and fails closed, never a silently smaller or empty
/// set. The `source` identities are decoded so a malformed row is unreadable.
fn decode_data_use(
    conn: &Connection,
    ticket_text: &str,
    count_raw: Option<i64>,
) -> Result<Vec<RawId>, InferenceTechnicalError> {
    let count = count_raw.ok_or_else(|| {
        inference_unavailable(String::from("inference attempt row missing data_use_count"))
    })?;
    let count = decode_u64(count).map_err(inference_unavailable)?;
    let mut statement = conn
        .prepare(SQL_SELECT_DATA_USE)
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let rows: Vec<(i64, String)> = statement
        .query_map(params![ticket_text], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| inference_unavailable(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    if rows.len() as u64 != count {
        return Err(inference_unavailable(String::from(
            "inference attempt data_use count disagrees with its rows",
        )));
    }
    let mut data_use = Vec::with_capacity(rows.len());
    for (expected, (ordinal, source)) in rows.iter().enumerate() {
        if *ordinal != expected as i64 {
            return Err(inference_unavailable(String::from(
                "inference attempt data_use ordinals are not contiguous",
            )));
        }
        data_use.push(decode_id(source).map_err(inference_unavailable)?);
    }
    Ok(data_use)
}

/// A claimed attempt is composed only when it is internally consistent:
/// known capability/consumer/purpose names, a Task Agent consumer whose
/// correlation group is complete and whose `data_use` is non-empty (or a
/// non-Task-Agent consumer with no correlation and no data use), and a
/// `data_use` child relation whose count, order, and identities agree with
/// the attempt row. Anything else is an unreadable row, never guessed.
fn decode_attempt_record(
    conn: &Connection,
    ticket: ene_inference::InferenceTicketId,
    raw: RawAttempt,
) -> Result<InferenceAttemptRecord, InferenceTechnicalError> {
    let capability = CapabilityKind::from_name(&raw.capability).ok_or_else(|| {
        inference_unavailable(String::from("unknown inference capability in attempt row"))
    })?;
    let consumer = decode_consumer(raw.consumer.as_deref().ok_or_else(|| {
        inference_unavailable(String::from("inference attempt row missing consumer"))
    })?)
    .map_err(inference_unavailable)?;
    let purpose = decode_purpose(raw.purpose.as_deref().ok_or_else(|| {
        inference_unavailable(String::from("inference attempt row missing purpose"))
    })?)
    .map_err(inference_unavailable)?;
    let data_use = decode_data_use(conn, &encode_id(ticket.0), raw.data_use_count)?;
    let task_agent = match (raw.delegation_id, raw.task_id, raw.task_revision) {
        (None, None, None) => {
            if !data_use.is_empty() {
                return Err(inference_unavailable(String::from(
                    "non-task-agent attempt carries a data-use correlation",
                )));
            }
            None
        }
        (Some(delegation), Some(task), Some(revision_raw)) => {
            if data_use.is_empty() {
                return Err(inference_unavailable(String::from(
                    "task agent attempt has no data-use correlation",
                )));
            }
            Some(TaskAgentAttemptPremise {
                delegation: decode_id(&delegation).map_err(inference_unavailable)?,
                task: decode_id(&task).map_err(inference_unavailable)?,
                task_revision: RevisionInner::from_u64(
                    decode_u64(revision_raw).map_err(inference_unavailable)?,
                ),
                data_use,
            })
        }
        _ => {
            return Err(inference_unavailable(String::from(
                "partial task agent attempt correlation",
            )));
        }
    };
    if (consumer == ConsumerKind::TaskAgent) != task_agent.is_some() {
        return Err(inference_unavailable(String::from(
            "task agent consumer and correlation disagree",
        )));
    }
    Ok(InferenceAttemptRecord {
        ticket,
        consumer,
        capability,
        purpose,
        provider: raw.provider,
        model: raw.model,
        task_agent,
    })
}

impl UsageRepository for Store {
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(fact.ticket.0);
            // The fact must be internally consistent before it touches the
            // database: Reported carries all three counts with cached input a
            // subset of input, Unknown carries none. Zero-as-unknown cannot
            // survive this boundary.
            let reported = fact.source == UsageSource::Reported;
            if reported != (fact.input_tokens.is_some() && fact.output_tokens.is_some()) {
                return Err(inference_unavailable(String::from(
                    "usage source and counts disagree",
                )));
            }
            let input_column =
                encode_optional_count(fact.input_tokens).map_err(inference_unavailable)?;
            let output_column =
                encode_optional_count(fact.output_tokens).map_err(inference_unavailable)?;
            let cached_column = match (fact.cached_input_tokens, fact.input_tokens) {
                (Some(cached), Some(input)) if cached <= input => {
                    encode_optional_count(Some(cached)).map_err(inference_unavailable)?
                }
                (None, _) if !reported => {
                    encode_optional_count(None).map_err(inference_unavailable)?
                }
                _ => {
                    return Err(inference_unavailable(String::from(
                        "cached tokens are not an input subset",
                    )));
                }
            };
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            // Attribution belongs to the claimed attempt, and the rate the
            // attempt was admitted under is the only one this fact may bind.
            // Refuse orphan facts and route substitutions rather than
            // manufacturing correspondence.
            let route: Option<(String, String, Option<String>)> = tx
                .query_row(SQL_SELECT_ATTEMPT_ROUTE, [&ticket_text], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let Some((attempt_provider, attempt_model, pricing_reference)) = route else {
                return Err(inference_unavailable(String::from(
                    "usage attempt route mismatch",
                )));
            };
            if attempt_provider != fact.provider || attempt_model != fact.model {
                return Err(inference_unavailable(String::from(
                    "usage attempt route mismatch",
                )));
            }
            // The first settlement is terminal, including Unknown. Serialize
            // writers in SQLite; duplicates cannot replace it with later counts.
            tx.execute(
                SQL_INSERT_USAGE,
                params![
                    ticket_text,
                    fact.provider,
                    fact.model,
                    input_column,
                    cached_column,
                    output_column,
                    encode_usage_source(fact.source),
                    pricing_reference
                ],
            )
            .map_err(|error| inference_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn load_usage_cost(
        &self,
        ticket: ene_inference::InferenceTicketId,
    ) -> Result<Option<UsageCostRecord>, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(ticket.0);
            let guard = lock_shared(&conn);
            let found: Option<RawUsageRow> = guard
                .query_row(SQL_SELECT_USAGE_COST, params![ticket_text], raw_usage_row)
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let Some(raw) = found else {
                return Ok(None);
            };
            let usage = decode_usage_fact(ticket, &raw)?;
            // The binding is written once, at the claim, and copied at the
            // settlement. Requiring both rows to agree means an edit to one
            // of them cannot silently reprice a historical fact.
            let attempt_route: Option<(String, String, Option<String>)> = guard
                .query_row(SQL_SELECT_ATTEMPT_ROUTE, params![ticket_text], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let Some((attempt_provider, attempt_model, attempt_pricing)) = attempt_route else {
                return Err(inference_unavailable(String::from(
                    "usage fact has no claimed attempt",
                )));
            };
            if attempt_provider != usage.provider
                || attempt_model != usage.model
                || attempt_pricing != raw.pricing_snapshot
            {
                return Err(inference_unavailable(String::from(
                    "usage fact pricing binding disagrees with its attempt",
                )));
            }
            let pricing = match raw.pricing_snapshot.as_deref() {
                None => None,
                Some(reference) => Some(load_pricing_snapshot(
                    &guard,
                    reference,
                    &usage.provider,
                    &usage.model,
                )?),
            };
            let cost = project_cost(&usage, pricing.as_ref()).map_err(|error| {
                InferenceTechnicalError::CostProjectionFailed {
                    reason: error.to_string(),
                }
            })?;
            Ok(Some(UsageCostRecord { usage, cost }))
        })
        .await
    }
}
