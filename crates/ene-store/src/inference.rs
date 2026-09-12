use std::sync::Arc;

use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRecord, InferenceAttemptRepository,
    InferenceTechnicalError, TaskAgentAttemptPremise, UsageFact, UsageRepository,
};
use ene_permission::{CapabilityKind, ConsumerKind};
use ene_primitive::{RevisionInner, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_consumer, decode_id, decode_purpose, decode_u64, encode_consumer, encode_id,
    encode_optional_count, encode_purpose, encode_u64, encode_usage_source, inference_unavailable,
    lock_shared, select_consent,
};
use crate::credential::SQL_SELECT_SET_REV;
use crate::run_blocking;

const SQL_INSERT_ATTEMPT: &str = "INSERT INTO inference_attempt (ticket, capability, consumer, purpose, consent_id, consent_rev, provider, model, started_at, delegation_id, task_id, task_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

const SQL_SELECT_ATTEMPT: &str = "SELECT capability, consumer, purpose, provider, model, delegation_id, task_id, task_revision FROM inference_attempt WHERE ticket = ?1";

const SQL_SELECT_DELEGATION_PREMISE: &str =
    "SELECT task_id, task_revision FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_TASK_REVISION_POINTER: &str = "SELECT revision FROM task WHERE task_id = ?1";

const SQL_INSERT_USAGE: &str = "INSERT INTO usage_fact (ticket, provider, model, input_tokens, output_tokens, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

impl InferenceAttemptRepository for Store {
    async fn begin_inference_attempt(
        &self,
        attempt: InferenceAttempt,
    ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let rev_raw =
                encode_u64(attempt.expected_consent.1.as_u64()).map_err(inference_unavailable)?;
            let ticket_text = encode_id(attempt.ticket.0);
            let correlation = encode_task_agent(attempt.task_agent)?;
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| inference_unavailable(error.to_string()))?;
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
            match check_task_agent_premise(&tx, attempt.task_agent)? {
                TaskPremiseCheck::Current => {}
                TaskPremiseCheck::Stale => {
                    return Ok(AttemptBeginOutcome::TaskPremiseStale);
                }
            }
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
                    attempt.provider,
                    attempt.model,
                    started_text,
                    correlation.delegation,
                    correlation.task,
                    correlation.task_revision,
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
                .map(|raw| decode_attempt_record(ticket, raw))
                .transpose()
        })
        .await
    }
}

/// The encoded Task Agent correlation group. The whole group is present or
/// absent together; a partial group can never be written.
struct EncodedCorrelation {
    delegation: Option<String>,
    task: Option<String>,
    task_revision: Option<i64>,
}

fn encode_task_agent(
    premise: Option<TaskAgentAttemptPremise>,
) -> Result<EncodedCorrelation, InferenceTechnicalError> {
    match premise {
        None => Ok(EncodedCorrelation {
            delegation: None,
            task: None,
            task_revision: None,
        }),
        Some(premise) => Ok(EncodedCorrelation {
            delegation: Some(encode_id(premise.delegation)),
            task: Some(encode_id(premise.task)),
            task_revision: Some(
                encode_u64(premise.task_revision.as_u64()).map_err(inference_unavailable)?,
            ),
        }),
    }
}

enum TaskPremiseCheck {
    Current,
    Stale,
}

/// Task Agent premise compare inside the claim transaction.
///
/// (1) the delegation row exists, (2) its stated `(task, revision)` equals
/// the premise, (3) the current Task row is still at the relied revision.
/// Missing rows and a moved revision are [`TaskPremiseCheck::Stale`] (domain
/// stale, no write, no send); a disagreement (2) or malformed stored values
/// are technical errors, never a fabricated stale.
fn check_task_agent_premise(
    tx: &rusqlite::Transaction<'_>,
    premise: Option<TaskAgentAttemptPremise>,
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
    let current: Option<i64> = tx
        .query_row(
            SQL_SELECT_TASK_REVISION_POINTER,
            params![encode_id(premise.task)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| inference_unavailable(error.to_string()))?;
    let Some(current_raw) = current else {
        return Ok(TaskPremiseCheck::Stale);
    };
    let current_revision = decode_u64(current_raw).map_err(inference_unavailable)?;
    if current_revision != premise.task_revision.as_u64() {
        return Ok(TaskPremiseCheck::Stale);
    }
    Ok(TaskPremiseCheck::Current)
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
    })
}

/// A claimed attempt is composed only when it is internally consistent:
/// known capability/consumer/purpose names, and a Task Agent consumer whose
/// correlation group is complete (or a non-Task-Agent consumer with no
/// group). Anything else is an unreadable row, never guessed.
fn decode_attempt_record(
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
    let task_agent = match (raw.delegation_id, raw.task_id, raw.task_revision) {
        (None, None, None) => None,
        (Some(delegation), Some(task), Some(revision_raw)) => Some(TaskAgentAttemptPremise {
            delegation: decode_id(&delegation).map_err(inference_unavailable)?,
            task: decode_id(&task).map_err(inference_unavailable)?,
            task_revision: RevisionInner::from_u64(
                decode_u64(revision_raw).map_err(inference_unavailable)?,
            ),
        }),
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
            let input_column =
                encode_optional_count(fact.input_tokens).map_err(inference_unavailable)?;
            let output_column =
                encode_optional_count(fact.output_tokens).map_err(inference_unavailable)?;
            let guard = lock_shared(&conn);
            // A duplicate ticket violates the primary key and maps to
            // `StorageUnavailable`, never a panic.
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
