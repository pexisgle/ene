use std::sync::Arc;

use ene_permission::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
    IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository,
    IntentResolution, PermissionErasureRepository, PermissionTechnicalError, ShortcutIntentOutcome,
    consent_current_mark, consent_mark, parse_consent_mark,
};
use ene_preservation::{ErasureConditionRef, LocalErasurePass};
use rusqlite::{Transaction, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    encode_u64, insert_decided_row_tx, lock_shared, permission_unavailable, replay_or_conflict,
    select_consent, select_intent_row,
};
use crate::erasure::{ERASURE_BATCH_ROWS, erasure_count};
use crate::preservation::condition_is_current;
use crate::run_blocking;

const SQL_UPSERT_CONSENT: &str = "INSERT INTO consent_record (capability, id, rev, provider, model, credential_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (capability) DO UPDATE SET id = excluded.id, rev = excluded.rev, provider = excluded.provider, model = excluded.model, credential_id = excluded.credential_id";

/// Used by the intent-atomic assign so the premise check and the write
/// cannot drift apart. The row is selected and written under the record's
/// own capability, so a dialogue assignment can never overwrite or borrow
/// the learning assignment.
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
    tx.execute(
        SQL_UPSERT_CONSENT,
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
            // Write-once claim first: an existing row is never rewritten.
            if let Some(stored) = select_intent_row(&tx, &record.fingerprint.intent_id)
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
            select_intent_row(&guard, &intent_id).map_err(permission_unavailable)
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
                select_intent_row(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
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
                    current: consent_current_mark(record.capability, current.as_ref()),
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
                select_intent_row(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            let current =
                select_consent(&tx, CapabilityKind::Dialogue).map_err(permission_unavailable)?;
            let expected = parse_consent_mark(&expected_base, CapabilityKind::Dialogue);
            let current_state = current.as_ref().map(|record| record.rev.as_u64());
            let outcome = if expected != Some(current_state) {
                IntentOutcome::StaleBaseView {
                    current: consent_current_mark(CapabilityKind::Dialogue, current.as_ref()),
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
                select_intent_row(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
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

/// Redacts the caller-supplied text columns of the decision journal.
///
/// The journal row itself is never deleted: deleting a decided intent would
/// reopen its identity, so a retried id could re-execute a decision the Owner
/// already received. The erased span is removed (`''`, never a marker), so no
/// marker text can itself become a target match or a target-derived value.
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

impl PermissionErasureRepository for Store {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<Output = Result<LocalErasurePass, PermissionTechnicalError>> + Send
    {
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
                    return Ok(LocalErasurePass::NotCurrent);
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
                let erased = erasure_count(journal_redacted + consents_invalidated)
                    .map_err(permission_unavailable)?;
                let remainder = erasure_count(
                    journal_remainder
                        .checked_add(consent_remainder)
                        .ok_or_else(|| {
                            permission_unavailable(String::from("count out of range"))
                        })?,
                )
                .map_err(permission_unavailable)?;
                tx.commit()
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                Ok(LocalErasurePass::Applied { erased, remainder })
            })
            .await
        }
    }
}
