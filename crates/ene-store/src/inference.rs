use std::sync::Arc;

use ene_inference::{
    AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository, InferenceTechnicalError,
    UsageFact, UsageRepository,
};
use ene_primitive::WallClockWithTz;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    SQL_SELECT_CONSENT, decode_u64, encode_id, encode_optional_count, encode_u64,
    encode_usage_source, inference_unavailable, lock_shared,
};
use crate::run_blocking;

const SQL_INSERT_ATTEMPT: &str = "INSERT INTO inference_attempt (ticket, consent_id, consent_rev, provider, model, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

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
                    && decode_u64(*rev)
                        .is_ok_and(|value| value == attempt.expected_consent.1.as_u64())
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
