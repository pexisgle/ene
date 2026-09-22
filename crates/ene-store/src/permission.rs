use std::sync::Arc;

use ene_permission::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
    IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository,
    IntentResolution, PermissionErasureOutcome, PermissionErasureRepository,
    PermissionTechnicalError, ShortcutIntentOutcome, consent_mark, parse_consent_mark,
};
use ene_preservation::ErasureConditionRef;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    IntentOutcomeRow, SQL_SELECT_INTENT_OUTCOME, decode_intent_outcome_row, encode_u64,
    insert_decided_row_tx, lock_shared, permission_unavailable, replay_or_conflict, select_consent,
    select_intent_row_tx,
};
use crate::preservation::condition_is_current;
use crate::run_blocking;

const SQL_INSERT_CONSENT: &str = "INSERT INTO consent_record (capability, id, rev, provider, model, credential_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

const SQL_UPDATE_CONSENT: &str = "UPDATE consent_record SET id = ?2, rev = ?3, provider = ?4, model = ?5, credential_id = ?6 WHERE capability = ?1";

fn current_mark(capability: CapabilityKind, current: Option<&ConsentRecord>) -> String {
    consent_mark(capability, current.map(|record| record.rev.as_u64()))
}

fn compare_and_save_row(
    tx: &Transaction<'_>,
    expected: Option<(&str, &ConsentRevision)>,
    record: &ConsentRecord,
) -> Result<ConsentCommitOutcome, String> {
    let rev_raw = encode_u64(record.rev.as_u64())?;
    let current = select_consent(tx, record.capability)?;
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
                record.capability.as_str(),
                record.id,
                rev_raw,
                record.provider,
                record.model,
                record.credential_id
            ],
        )
        .map_err(|error| error.to_string())?;
    } else {
        tx.execute(
            SQL_UPDATE_CONSENT,
            params![
                record.capability.as_str(),
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

impl ConsentRepository for Store {
    async fn load_current(
        &self,
        capability: CapabilityKind,
    ) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            select_consent(&guard, capability).map_err(permission_unavailable)
        })
        .await
    }
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
            if let Some(stored) = select_intent_row_tx(&tx, &record.fingerprint.intent_id)
                .map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &record.fingerprint));
            }
            insert_decided_row_tx(&tx, &record.fingerprint, &record.outcome)
                .map_err(permission_unavailable)?;
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(IntentResolution::Decided(()))
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
            let snapshot = match &outcome {
                ConsentCommitOutcome::Committed { record } => IntentOutcome::StoredAsRuleView {
                    revision: consent_mark(record.capability, Some(record.rev.as_u64())),
                },
                ConsentCommitOutcome::StaleCurrent { current } => IntentOutcome::StaleBaseView {
                    current: current_mark(record.capability, current.as_ref()),
                },
            };
            insert_decided_row_tx(&tx, &fingerprint, &snapshot).map_err(permission_unavailable)?;
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(IntentResolution::Decided(outcome))
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
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            let current =
                select_consent(&tx, CapabilityKind::Dialogue).map_err(permission_unavailable)?;
            let expected = parse_consent_mark(&expected_base, CapabilityKind::Dialogue);
            let current_state = current.as_ref().map(|record| record.rev.as_u64());
            let outcome = if expected != Some(current_state) {
                IntentOutcome::StaleBaseView {
                    current: current_mark(CapabilityKind::Dialogue, current.as_ref()),
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
            insert_decided_row_tx(&tx, &decided.fingerprint, &decided.outcome)
                .map_err(permission_unavailable)?;
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(IntentResolution::Decided(decided))
        })
        .await
    }

    async fn shortcut_with_intent(
        &self,
        capability: CapabilityKind,
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
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            let current = select_consent(&tx, capability).map_err(permission_unavailable)?;
            let record = match current {
                Some(record)
                    if record.provider == provider
                        && record.model == model
                        && record.credential_id == credential_id =>
                {
                    record
                }
                _ => return Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss)),
            };
            let snapshot = IntentOutcome::StoredAsRuleView {
                revision: consent_mark(capability, Some(record.rev.as_u64())),
            };
            insert_decided_row_tx(&tx, &fingerprint, &snapshot).map_err(permission_unavailable)?;
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(IntentResolution::Decided(ShortcutIntentOutcome::Hit {
                current: record,
            }))
        })
        .await
    }
}

const ERASURE_BATCH_ROWS: i64 = 500;

const SQL_REDACT_INTENT_JOURNAL: &str = "UPDATE management_intent
     SET target = replace(target, ?1, ''),
         rationale_quote = replace(rationale_quote, ?1, '')
     WHERE rowid IN (
         SELECT rowid FROM management_intent
         WHERE instr(target, ?1) > 0
            OR instr(COALESCE(rationale_quote, ''), ?1) > 0
         LIMIT ?2
     )";

const SQL_INVALIDATE_CONSENT: &str = "DELETE FROM consent_record
     WHERE capability IN (
         SELECT capability FROM consent_record
         WHERE instr(id, ?1) > 0 OR instr(provider, ?1) > 0
            OR instr(model, ?1) > 0 OR instr(credential_id, ?1) > 0
         LIMIT ?2
     )";

const SQL_COUNT_JOURNAL_TARGET: &str = "SELECT COUNT(*) FROM management_intent
     WHERE instr(target, ?1) > 0 OR instr(COALESCE(rationale_quote, ''), ?1) > 0";

const SQL_COUNT_CONSENT_TARGET: &str = "SELECT COUNT(*) FROM consent_record
     WHERE instr(id, ?1) > 0 OR instr(provider, ?1) > 0
        OR instr(model, ?1) > 0 OR instr(credential_id, ?1) > 0";

fn erasure_count(value: i64) -> Result<u64, PermissionTechnicalError> {
    u64::try_from(value).map_err(|_| permission_unavailable(String::from("count out of range")))
}

impl PermissionErasureRepository for Store {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<PermissionErasureOutcome, PermissionTechnicalError>,
    > + Send {
        #[cfg(any(test, feature = "test-support"))]
        let parks = Arc::clone(&self.test_parks);
        let conn = Arc::clone(&self.conn);
        let target = target.to_owned();
        async move {
            #[cfg(any(test, feature = "test-support"))]
            parks.erasure_mutation.pause_if_armed().await;
            run_blocking(move || {
                let mut guard = lock_shared(&conn);
                let tx = guard
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                if !condition_is_current(&tx, condition)
                    .map_err(|error| permission_unavailable(error.to_string()))?
                {
                    return Ok(PermissionErasureOutcome::NotCurrent);
                }
                let journal_redacted = tx
                    .execute(
                        SQL_REDACT_INTENT_JOURNAL,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                let consents_invalidated = tx
                    .execute(SQL_INVALIDATE_CONSENT, params![target, ERASURE_BATCH_ROWS])
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                let journal_remainder: i64 = tx
                    .query_row(SQL_COUNT_JOURNAL_TARGET, params![target], |row| row.get(0))
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                let consent_remainder: i64 = tx
                    .query_row(SQL_COUNT_CONSENT_TARGET, params![target], |row| row.get(0))
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                let erased = erasure_count(
                    i64::try_from(journal_redacted + consents_invalidated)
                        .map_err(|_| permission_unavailable(String::from("count out of range")))?,
                )?;
                let remainder = erasure_count(
                    journal_remainder
                        .checked_add(consent_remainder)
                        .ok_or_else(|| {
                            permission_unavailable(String::from("count out of range"))
                        })?,
                )?;
                tx.commit()
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                Ok(PermissionErasureOutcome::Applied { erased, remainder })
            })
            .await
        }
    }
}
