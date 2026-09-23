use std::sync::Arc;

use ene_inference::cost::{CostProjectionError, Money, TokenRate, UsageCostFact, project_cost};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};
use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRecord, InferenceAttemptRepository,
    InferenceTechnicalError, InferenceTicketId, ReportedTokenUsage, TaskAgentAttemptPremise,
    UsageCostRecord, UsageFact, UsageRepository, UsageSource, UsageSummaryQuery,
    UsageSummaryRepository, UsageSummaryRow, UsageSummaryStatus,
};
use ene_permission::{CapabilityKind, ConsumerKind, PurposeKind, UsageReservationState};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_consumer, decode_currency, decode_id, decode_pricing_reference, decode_purpose,
    decode_u64, decode_usage_source, decode_wall_clock, encode_consumer, encode_currency,
    encode_id, encode_optional_count, encode_pricing_reference, encode_purpose, encode_u64,
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

const SQL_SELECT_DELEGATION_RESULT: &str =
    "SELECT result_id FROM task_result WHERE delegation_id = ?1";

const SQL_INSERT_USAGE: &str = "INSERT INTO usage_fact (ticket, provider, model, input_tokens, cached_input_tokens, output_tokens, source, pricing_snapshot) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(ticket) DO NOTHING";

const SQL_SELECT_ATTEMPT_ROUTE: &str =
    "SELECT provider, model, pricing_snapshot FROM inference_attempt WHERE ticket = ?1";

const SQL_SELECT_USAGE_COST: &str = "SELECT provider, model, input_tokens, cached_input_tokens, output_tokens, source, pricing_snapshot FROM usage_fact WHERE ticket = ?1";

pub(crate) const SQL_SELECT_USAGE_SUMMARY: &str = "SELECT a.ticket, a.provider, a.model, a.consumer, a.purpose, a.started_at, f.source, f.input_tokens, f.cached_input_tokens, f.output_tokens, f.pricing_snapshot, r.state, r.currency, r.upper_bound_micros, p.id, p.provider, p.model, p.currency, p.input_rate, p.cached_input_rate, p.output_rate, p.effective_at, p.source_revision, a.pricing_snapshot, r.committed_currency, r.committed_micros FROM inference_attempt a LEFT JOIN usage_fact f ON f.ticket = a.ticket LEFT JOIN usage_reservation r ON r.ticket = a.ticket LEFT JOIN pricing_snapshot p ON p.id = f.pricing_snapshot WHERE a.started_at >= ?1 AND a.started_at < ?2 AND (?3 IS NULL OR a.provider = ?3) AND (?4 IS NULL OR a.model = ?4) AND (?5 IS NULL OR a.consumer = ?5) AND (?6 IS NULL OR a.purpose = ?6) AND (r.state IS NULL OR r.state != 'released') AND (?7 IS NULL OR (CASE WHEN f.source = 'reported' THEN 'reported' WHEN f.source = 'unknown' THEN 'unknown' ELSE 'reserved' END) = ?7) AND (?8 IS NULL OR a.started_at < ?8 OR (a.started_at = ?8 AND a.ticket < ?9)) ORDER BY a.started_at DESC, a.ticket DESC LIMIT ?10";

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
            let correlation = encode_correlation(&attempt)?;
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let already_claimed: Option<String> = tx
                .query_row(SQL_SELECT_ATTEMPT_TICKET, params![ticket_text], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            if already_claimed.is_some() {
                return Ok(AttemptBeginOutcome::Stale);
            }
            let current = select_consent(&tx, attempt.capability).map_err(inference_unavailable)?;
            let current_matches = current.as_ref().is_some_and(|record| {
                record.id == attempt.expected_consent.0
                    && record.rev.as_u64() == attempt.expected_consent.1.as_u64()
            });
            if !current_matches {
                return Ok(AttemptBeginOutcome::Stale);
            }
            let stored_set: i64 = tx
                .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let current_set = decode_u64(stored_set).map_err(inference_unavailable)?;
            if current_set != attempt.expected_credential_set.as_u64() {
                return Ok(AttemptBeginOutcome::Stale);
            }
            match check_task_agent_premise(&tx, attempt.task_agent.as_ref())? {
                TaskPremiseCheck::Current => {}
                TaskPremiseCheck::Stale => {
                    return Ok(AttemptBeginOutcome::TaskPremiseStale);
                }
            }
            match check_data_use_currentness(&tx, &correlation.data_use)? {
                DataUseCheck::Clear => {}
                DataUseCheck::Covered => {
                    return Ok(AttemptBeginOutcome::DataUseHeld);
                }
            }
            if crate::preservation::inflight_learning_formation_held(&tx, &correlation.data_use)
                .map_err(|error| inference_unavailable(error.to_string()))?
            {
                return Ok(AttemptBeginOutcome::DataUseHeld);
            }
            let data_use_count =
                encode_u64(correlation.data_use.len() as u64).map_err(inference_unavailable)?;
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
            let now = WallClockWithTz::now();
            match crate::usage_cap::admit_reservation(
                &tx,
                &attempt.ticket,
                crate::usage_cap::ReservationPremise {
                    provider: &attempt.provider,
                    model: &attempt.model,
                    pricing: attempt.pricing.as_ref(),
                    pricing_reference: pricing_reference.as_deref(),
                    estimate: attempt.usage_estimate.as_ref(),
                },
                now,
            )
            .map_err(inference_unavailable)?
            {
                crate::usage_cap::ReservationAdmission::NoCap
                | crate::usage_cap::ReservationAdmission::Reserved => {}
                crate::usage_cap::ReservationAdmission::Held => {
                    return Ok(AttemptBeginOutcome::HeldByCap);
                }
                crate::usage_cap::ReservationAdmission::Indeterminate => {
                    return Ok(AttemptBeginOutcome::CapIndeterminate);
                }
            }
            let started_text = encode_wall_clock(now);
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
                Err(error) => return Err(inference_unavailable(error.to_string())),
            }
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

struct EncodedCorrelation {
    delegation: Option<String>,
    task: Option<String>,
    task_revision: Option<i64>,
    data_use: Vec<String>,
}

fn encode_data_use(data_use: &[RawId]) -> Vec<String> {
    data_use.iter().map(|source| encode_id(*source)).collect()
}

fn encode_correlation(
    attempt: &InferenceAttempt,
) -> Result<EncodedCorrelation, InferenceTechnicalError> {
    match attempt.consumer {
        ConsumerKind::TaskAgent => {
            let Some(premise) = attempt.task_agent.as_ref() else {
                return Err(inference_unavailable(String::from(
                    "task agent attempt carries no correlation premise",
                )));
            };
            if premise.data_use.is_empty() {
                return Err(inference_unavailable(String::from(
                    "task agent attempt carries no data-use correlation",
                )));
            }
            if premise.data_use != attempt.data_use {
                return Err(inference_unavailable(String::from(
                    "task agent attempt correlation disagrees with its premise",
                )));
            }
            Ok(EncodedCorrelation {
                delegation: Some(encode_id(premise.delegation)),
                task: Some(encode_id(premise.task)),
                task_revision: Some(
                    encode_u64(premise.task_revision.as_u64()).map_err(inference_unavailable)?,
                ),
                data_use: encode_data_use(&attempt.data_use),
            })
        }
        ConsumerKind::CompanionLearning => {
            if attempt.task_agent.is_some() {
                return Err(inference_unavailable(String::from(
                    "learning attempt carries a task agent correlation",
                )));
            }
            if attempt.data_use.is_empty() {
                return Err(inference_unavailable(String::from(
                    "learning attempt carries no data-use correlation",
                )));
            }
            Ok(EncodedCorrelation {
                delegation: None,
                task: None,
                task_revision: None,
                data_use: encode_data_use(&attempt.data_use),
            })
        }
        ConsumerKind::CompanionDialogue => {
            if attempt.task_agent.is_some() {
                return Err(inference_unavailable(String::from(
                    "dialogue attempt carries a task agent correlation",
                )));
            }
            Ok(EncodedCorrelation {
                delegation: None,
                task: None,
                task_revision: None,
                data_use: encode_data_use(&attempt.data_use),
            })
        }
    }
}

enum DataUseCheck {
    Clear,
    Covered,
}

fn check_data_use_currentness(
    tx: &rusqlite::Transaction<'_>,
    data_use: &[String],
) -> Result<DataUseCheck, InferenceTechnicalError> {
    for source in data_use {
        if crate::preservation::covering_condition(tx, source)
            .map_err(|error| inference_unavailable(error.to_string()))?
            .is_some()
        {
            return Ok(DataUseCheck::Covered);
        }
    }
    Ok(DataUseCheck::Clear)
}

enum TaskPremiseCheck {
    Current,
    Stale,
}

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

#[derive(Clone)]
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
    raw_pricing_fields(row, 0)
}

fn raw_pricing_fields(row: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<RawPricing> {
    Ok(RawPricing {
        id: row.get(base)?,
        provider: row.get(base + 1)?,
        model: row.get(base + 2)?,
        currency: row.get(base + 3)?,
        input_rate: row.get(base + 4)?,
        cached_input_rate: row.get(base + 5)?,
        output_rate: row.get(base + 6)?,
        effective_at: row.get(base + 7)?,
        source_revision: row.get(base + 8)?,
    })
}

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
    let source = decode_usage_source(&raw.source).map_err(inference_unavailable)?;
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
            if !data_use.is_empty()
                && !matches!(
                    consumer,
                    ConsumerKind::CompanionLearning | ConsumerKind::CompanionDialogue
                )
            {
                return Err(inference_unavailable(String::from(
                    "non-learning attempt carries a data-use correlation",
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
                data_use: data_use.clone(),
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
        data_use,
    })
}

fn settle_reservation(
    tx: &rusqlite::Transaction<'_>,
    ticket_text: &str,
    fact: &UsageFact,
    pricing_reference: Option<&str>,
) -> Result<(), InferenceTechnicalError> {
    let stored: Option<crate::usage_cap::ReservationRow> = tx
        .query_row(
            crate::usage_cap::SQL_SELECT_RESERVATION_BY_TICKET,
            params![ticket_text],
            crate::usage_cap::reservation_row,
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some(raw) = stored else {
        return Ok(());
    };
    let state = ene_permission::UsageReservationState::from_name(&raw.state)
        .ok_or_else(|| inference_unavailable(String::from("unknown usage reservation state")))?;
    if state.is_terminal() {
        return Ok(());
    }
    let committed = match fact.source {
        UsageSource::Unknown => None,
        UsageSource::Reported => {
            let settlement = match pricing_reference {
                None => None,
                Some(reference) => Some(load_pricing_snapshot(
                    tx,
                    reference,
                    &fact.provider,
                    &fact.model,
                )?),
            };
            match project_cost(fact, settlement.as_ref()) {
                Ok(UsageCostFact::Reported(cost)) => Some(cost.total),
                Ok(UsageCostFact::Unknown { .. }) | Err(_) => None,
            }
        }
    };
    let (state, committed_currency, committed_micros) = match committed {
        Some(total) => (
            ene_permission::UsageReservationState::CommittedReported,
            Some(total.currency().as_str()),
            Some(encode_u64(total.micros()).map_err(inference_unavailable)?),
        ),
        None => (
            ene_permission::UsageReservationState::CommittedUnknown,
            None,
            None,
        ),
    };
    tx.execute(
        crate::usage_cap::SQL_SETTLE_RESERVATION,
        params![
            ticket_text,
            state.as_str(),
            committed_currency,
            committed_micros
        ],
    )
    .map_err(|error| inference_unavailable(error.to_string()))?;
    Ok(())
}

impl UsageRepository for Store {
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(fact.ticket.0);
            let reported = fact.source == UsageSource::Reported;
            let all_present = fact.input_tokens.is_some()
                && fact.cached_input_tokens.is_some()
                && fact.output_tokens.is_some();
            let any_present = fact.input_tokens.is_some()
                || fact.cached_input_tokens.is_some()
                || fact.output_tokens.is_some();
            if reported != all_present || (!reported && any_present) {
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
            settle_reservation(&tx, &ticket_text, &fact, pricing_reference.as_deref())?;
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

    async fn load_usage_reservation(
        &self,
        ticket: ene_inference::InferenceTicketId,
    ) -> Result<Option<ene_inference::UsageReservation>, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let ticket_text = encode_id(ticket.0);
            let guard = lock_shared(&conn);
            let found: Option<crate::usage_cap::ReservationRow> = guard
                .query_row(
                    crate::usage_cap::SQL_SELECT_RESERVATION_BY_TICKET,
                    params![ticket_text],
                    crate::usage_cap::reservation_row,
                )
                .optional()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            found
                .as_ref()
                .map(|raw| crate::usage_cap::decode_reservation(raw).map_err(inference_unavailable))
                .transpose()
        })
        .await
    }

    async fn reconcile_orphaned_usage_reservations(&self) -> Result<u64, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            // One bounded read of every reservation still `reserved` (the
            // only stored non-terminal state); each one settles as
            // CommittedUnknown in this transaction. The external consumption
            // cannot be denied after a crash, so the reserved upper bound stays
            // counted and the ticket records an unknown token usage fact:
            // recovery never releases a slot and never estimates zero.
            let orphans: Vec<crate::usage_cap::ReservationRow> = {
                let mut statement = tx
                    .prepare(crate::usage_cap::SQL_SELECT_ORPHANED_RESERVATIONS)
                    .map_err(|error| inference_unavailable(error.to_string()))?;
                statement
                    .query_map((), crate::usage_cap::reservation_row)
                    .map_err(|error| inference_unavailable(error.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| inference_unavailable(error.to_string()))?
            };
            let mut settled = 0_u64;
            for raw in orphans {
                // `SQL_SELECT_ORPHANED_RESERVATIONS` pins `state = 'reserved'`,
                // the only stored non-terminal value, so every returned row is
                // settled here; no second filter can diverge from the query.
                // The attempt row and the reservation were written in one
                // transaction, so a missing or disagreeing route correlation
                // is corruption: fail closed instead of recording an
                // unattributable usage fact.
                let route: Option<(String, String, Option<String>)> = tx
                    .query_row(SQL_SELECT_ATTEMPT_ROUTE, params![raw.ticket], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .optional()
                    .map_err(|error| inference_unavailable(error.to_string()))?;
                let Some((provider, model, pricing)) = route else {
                    return Err(inference_unavailable(String::from(
                        "usage reservation has no claimed attempt",
                    )));
                };
                if provider != raw.provider
                    || model != raw.model
                    || pricing.as_deref() != Some(raw.pricing_snapshot.as_str())
                {
                    return Err(inference_unavailable(String::from(
                        "usage reservation attribution disagrees with its attempt",
                    )));
                }
                tx.execute(
                    SQL_INSERT_USAGE,
                    params![
                        raw.ticket,
                        provider,
                        model,
                        Option::<i64>::None,
                        Option::<i64>::None,
                        Option::<i64>::None,
                        encode_usage_source(UsageSource::Unknown),
                        raw.pricing_snapshot
                    ],
                )
                .map_err(|error| inference_unavailable(error.to_string()))?;
                let changed = tx
                    .execute(
                        crate::usage_cap::SQL_SETTLE_RESERVATION,
                        params![
                            raw.ticket,
                            ene_permission::UsageReservationState::CommittedUnknown.as_str(),
                            Option::<String>::None,
                            Option::<i64>::None
                        ],
                    )
                    .map_err(|error| inference_unavailable(error.to_string()))?;
                if changed == 1 {
                    settled += 1;
                }
            }
            tx.commit()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            Ok(settled)
        })
        .await
    }
}

struct RawUsageSummaryRow {
    ticket: String,
    provider: String,
    model: String,
    consumer: Option<String>,
    purpose: Option<String>,
    started_at: String,
    usage_source: Option<String>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    usage_pricing: Option<String>,
    reservation_state: Option<String>,
    reserved_currency: Option<String>,
    reserved_upper_bound: Option<i64>,
    pricing: Option<RawPricing>,
    attempt_pricing: Option<String>,
    committed_currency: Option<String>,
    committed_micros: Option<i64>,
}

fn raw_usage_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawUsageSummaryRow> {
    let pricing_join = row.get::<_, Option<String>>(14)?;
    let pricing = if pricing_join.is_some() {
        Some(raw_pricing_fields(row, 14)?)
    } else {
        None
    };
    Ok(RawUsageSummaryRow {
        ticket: row.get(0)?,
        provider: row.get(1)?,
        model: row.get(2)?,
        consumer: row.get(3)?,
        purpose: row.get(4)?,
        started_at: row.get(5)?,
        usage_source: row.get(6)?,
        input_tokens: row.get(7)?,
        cached_input_tokens: row.get(8)?,
        output_tokens: row.get(9)?,
        usage_pricing: row.get(10)?,
        reservation_state: row.get(11)?,
        reserved_currency: row.get(12)?,
        reserved_upper_bound: row.get(13)?,
        pricing,
        attempt_pricing: row.get(23)?,
        committed_currency: row.get(24)?,
        committed_micros: row.get(25)?,
    })
}

/// Decodes one joined row into the owner-level usage summary.
///
/// The shapes that agree are the three legal combinations of fact and
/// reservation state; anything else (a reported fact beside a
/// non-reported reservation, a dangling pricing reference, or a committed
/// amount that disagrees with the projected cost) is unreadable and fails
/// closed, never a guessed status or amount. A `reported` fact beside a
/// `CommittedUnknown` reservation is legal: the reservation settled with an
/// unknown amount, as happens when a reported usage's cost cannot be
/// represented, and the row stays viewable as an unknown cost.
fn decode_usage_summary(
    raw: &RawUsageSummaryRow,
) -> Result<UsageSummaryRow, InferenceTechnicalError> {
    let ticket = InferenceTicketId(decode_id(&raw.ticket).map_err(inference_unavailable)?);
    let consumer = decode_consumer(raw.consumer.as_deref().ok_or_else(|| {
        inference_unavailable(String::from("usage summary row missing consumer"))
    })?)
    .map_err(inference_unavailable)?;
    let purpose =
        decode_purpose(raw.purpose.as_deref().ok_or_else(|| {
            inference_unavailable(String::from("usage summary row missing purpose"))
        })?)
        .map_err(inference_unavailable)?;
    let started_at = decode_wall_clock(&raw.started_at).map_err(inference_unavailable)?;
    let state = match raw.reservation_state.as_deref() {
        None => None,
        Some(name) => Some(UsageReservationState::from_name(name).ok_or_else(|| {
            inference_unavailable(String::from("unknown usage reservation state"))
        })?),
    };
    let source = match raw.usage_source.as_deref() {
        None => None,
        Some(text) => Some(decode_usage_source(text).map_err(inference_unavailable)?),
    };
    let upper_bound = match (raw.reserved_currency.as_deref(), raw.reserved_upper_bound) {
        (None, None) => None,
        (Some(currency), Some(micros)) => Some(Money::from_micros(
            decode_currency(currency).map_err(inference_unavailable)?,
            decode_u64(micros).map_err(inference_unavailable)?,
        )),
        _ => {
            return Err(inference_unavailable(String::from(
                "usage summary reservation amount is incomplete",
            )));
        }
    };
    if source.is_some() && raw.usage_pricing != raw.attempt_pricing {
        return Err(inference_unavailable(String::from(
            "usage fact pricing binding disagrees with its attempt",
        )));
    }
    if raw.committed_micros.is_some() != (state == Some(UsageReservationState::CommittedReported)) {
        return Err(inference_unavailable(String::from(
            "usage summary committed amount disagrees with its reservation state",
        )));
    }
    match source {
        Some(UsageSource::Reported) => {
            if !matches!(
                state,
                None | Some(
                    UsageReservationState::CommittedReported
                        | UsageReservationState::CommittedUnknown
                )
            ) {
                return Err(inference_unavailable(String::from(
                    "reported usage carries a non-reported reservation",
                )));
            }
            let counts = (
                raw.input_tokens.map(decode_u64).transpose(),
                raw.cached_input_tokens.map(decode_u64).transpose(),
                raw.output_tokens.map(decode_u64).transpose(),
            );
            let (Ok(input_tokens), Ok(cached_input_tokens), Ok(output_tokens)) = counts else {
                return Err(inference_unavailable(String::from(
                    "reported usage is missing a token count",
                )));
            };
            let (Some(input_tokens), Some(cached_input_tokens), Some(output_tokens)) =
                (input_tokens, cached_input_tokens, output_tokens)
            else {
                return Err(inference_unavailable(String::from(
                    "reported usage is missing a token count",
                )));
            };
            if cached_input_tokens > input_tokens {
                return Err(inference_unavailable(String::from(
                    "cached tokens are not an input subset",
                )));
            }
            let snapshot = match raw.usage_pricing.as_deref() {
                None => None,
                Some(reference_text) => {
                    let reference =
                        decode_pricing_reference(reference_text).map_err(inference_unavailable)?;
                    let pricing = raw.pricing.clone().ok_or_else(|| {
                        inference_unavailable(String::from(
                            "usage fact references a missing pricing snapshot",
                        ))
                    })?;
                    let snapshot = decode_pricing(pricing)?;
                    if snapshot.reference() != reference {
                        return Err(inference_unavailable(String::from(
                            "stored pricing snapshot does not match its reference",
                        )));
                    }
                    Some(snapshot)
                }
            };
            let fact = UsageFact {
                ticket,
                provider: raw.provider.clone(),
                model: raw.model.clone(),
                input_tokens: Some(input_tokens),
                cached_input_tokens: Some(cached_input_tokens),
                output_tokens: Some(output_tokens),
                source: UsageSource::Reported,
            };
            let cost = match project_cost(&fact, snapshot.as_ref()) {
                Ok(cost) => cost,
                // The provider call already ran; an unrepresentable amount is
                // an unknown cost, never a reason to hide the usage row.
                Err(CostProjectionError::AmountOverflow) => UsageCostFact::Unknown {
                    pricing: snapshot.as_ref().map(PricingSnapshot::reference),
                },
                Err(error) => {
                    return Err(InferenceTechnicalError::CostProjectionFailed {
                        reason: error.to_string(),
                    });
                }
            };
            // The cap accounting of a reported settlement is the committed
            // total; a row whose committed amount disagrees with the
            // projected cost cannot be displayed as both.
            if let (UsageCostFact::Reported(projected), Some(currency), Some(micros)) = (
                &cost,
                raw.committed_currency.as_deref(),
                raw.committed_micros,
            ) {
                let committed = Money::from_micros(
                    decode_currency(currency).map_err(inference_unavailable)?,
                    decode_u64(micros).map_err(inference_unavailable)?,
                );
                if committed != projected.total {
                    return Err(inference_unavailable(String::from(
                        "committed reservation disagrees with the projected cost",
                    )));
                }
            }
            Ok(UsageSummaryRow {
                ticket,
                provider: raw.provider.clone(),
                model: raw.model.clone(),
                consumer,
                purpose,
                status: UsageSummaryStatus::Reported,
                tokens: Some(ReportedTokenUsage {
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                }),
                cost: Some(cost),
                reserved: match state {
                    Some(UsageReservationState::CommittedUnknown) => upper_bound,
                    _ => None,
                },
                started_at,
            })
        }
        Some(UsageSource::Unknown) => {
            if !matches!(state, None | Some(UsageReservationState::CommittedUnknown)) {
                return Err(inference_unavailable(String::from(
                    "unknown usage carries a non-unknown reservation",
                )));
            }
            if raw.input_tokens.is_some()
                || raw.cached_input_tokens.is_some()
                || raw.output_tokens.is_some()
            {
                return Err(inference_unavailable(String::from(
                    "unknown usage carries token counts",
                )));
            }
            let pricing = match raw.usage_pricing.as_deref() {
                None => None,
                Some(reference_text) => {
                    let reference =
                        decode_pricing_reference(reference_text).map_err(inference_unavailable)?;
                    let stored = raw.pricing.clone().ok_or_else(|| {
                        inference_unavailable(String::from(
                            "usage fact references a missing pricing snapshot",
                        ))
                    })?;
                    let snapshot = decode_pricing(stored)?;
                    if snapshot.reference() != reference {
                        return Err(inference_unavailable(String::from(
                            "stored pricing snapshot does not match its reference",
                        )));
                    }
                    if snapshot.provider != raw.provider || snapshot.model != raw.model {
                        return Err(inference_unavailable(String::from(
                            "pricing snapshot route disagrees with the usage attribution",
                        )));
                    }
                    Some(reference)
                }
            };
            Ok(UsageSummaryRow {
                ticket,
                provider: raw.provider.clone(),
                model: raw.model.clone(),
                consumer,
                purpose,
                status: UsageSummaryStatus::Unknown,
                tokens: None,
                cost: Some(UsageCostFact::Unknown { pricing }),
                reserved: upper_bound,
                started_at,
            })
        }
        None => {
            if !matches!(state, None | Some(UsageReservationState::Reserved)) {
                return Err(inference_unavailable(String::from(
                    "reserved usage carries a settled reservation",
                )));
            }
            if raw.input_tokens.is_some()
                || raw.cached_input_tokens.is_some()
                || raw.output_tokens.is_some()
            {
                return Err(inference_unavailable(String::from(
                    "reserved usage carries token counts",
                )));
            }
            Ok(UsageSummaryRow {
                ticket,
                provider: raw.provider.clone(),
                model: raw.model.clone(),
                consumer,
                purpose,
                status: UsageSummaryStatus::Reserved,
                tokens: None,
                cost: None,
                reserved: upper_bound,
                started_at,
            })
        }
    }
}

impl UsageSummaryRepository for Store {
    async fn query_usage_summary(
        &self,
        query: UsageSummaryQuery,
    ) -> Result<Vec<UsageSummaryRow>, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            if query.effective_range_is_empty() {
                return Ok(Vec::new());
            }
            let limit =
                encode_u64(u64::from(query.effective_limit())).map_err(inference_unavailable)?;
            let from = encode_wall_clock(query.effective_from());
            let to = encode_wall_clock(query.to);
            let provider = query.provider.as_deref();
            let model = query.model.as_deref();
            let consumer = query.consumer.map(ConsumerKind::as_str);
            let purpose = query.purpose.map(PurposeKind::as_str);
            let status = query.status.map(UsageSummaryStatus::as_str);
            let (after_at, after_ticket) = match &query.after {
                None => (None, None),
                Some(cursor) => (
                    Some(encode_wall_clock(cursor.started_at)),
                    Some(encode_id(cursor.ticket.0)),
                ),
            };
            let guard = lock_shared(&conn);
            let mut statement = guard
                .prepare(SQL_SELECT_USAGE_SUMMARY)
                .map_err(|error| inference_unavailable(error.to_string()))?;
            let rows: Vec<RawUsageSummaryRow> = statement
                .query_map(
                    params![
                        from,
                        to,
                        provider,
                        model,
                        consumer,
                        purpose,
                        status,
                        after_at,
                        after_ticket,
                        limit
                    ],
                    raw_usage_summary_row,
                )
                .map_err(|error| inference_unavailable(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| inference_unavailable(error.to_string()))?;
            rows.iter().map(decode_usage_summary).collect()
        })
        .await
    }
}
