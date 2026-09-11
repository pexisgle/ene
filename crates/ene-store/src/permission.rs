use std::sync::Arc;

use ene_permission::{
    CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
    IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository,
    IntentResolution, PermissionTechnicalError, ShortcutIntentOutcome, consent_mark,
    parse_consent_mark,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    IntentOutcomeRow, SQL_SELECT_CONSENT, SQL_SELECT_INTENT_OUTCOME, decode_consent,
    decode_intent_outcome_row, encode_u64, insert_decided_row_tx, lock_shared,
    permission_unavailable, replay_or_conflict, select_intent_row_tx,
};
use crate::run_blocking;

const SQL_INSERT_CONSENT: &str = "INSERT INTO consent_record (capability, id, rev, provider, model, credential_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

const SQL_UPDATE_CONSENT: &str = "UPDATE consent_record SET id = ?2, rev = ?3, provider = ?4, model = ?5, credential_id = ?6 WHERE capability = ?1";

fn current_mark(capability: CapabilityKind, current: Option<&ConsentRecord>) -> String {
    consent_mark(capability, current.map(|record| record.rev.as_u64()))
}

/// Shared by [`ConsentRepository::compare_and_save`] and the intent-atomic
/// variant so the premise check and the write cannot drift apart between
/// the two entry points. The row is selected and written under the record's
/// own capability, so a dialogue assignment can never overwrite or borrow
/// the learning assignment.
fn compare_and_save_row(
    tx: &Transaction<'_>,
    expected: Option<(&str, &ConsentRevision)>,
    record: &ConsentRecord,
) -> Result<ConsentCommitOutcome, String> {
    let rev_raw = encode_u64(record.rev.as_u64())?;
    let found: Option<(String, i64, String, String, String)> = tx
        .query_row(
            SQL_SELECT_CONSENT,
            params![record.capability.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let current = match found {
        Some((id, stored_rev, provider, model, credential_id)) => Some(decode_consent(
            record.capability,
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
        // One logical row per capability: overwrite it.
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
            let found: Option<(String, i64, String, String, String)> = guard
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
                .map_err(|error| permission_unavailable(error.to_string()))?;
            match found {
                Some((id, rev_raw, provider, model, credential_id)) => {
                    let record =
                        decode_consent(capability, id, rev_raw, provider, model, credential_id)
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
            // so a retried id always observes the same answer.
            let snapshot = match &outcome {
                ConsentCommitOutcome::Committed { record } => IntentOutcome::StoredAsRuleView {
                    revision: consent_mark(record.capability, Some(record.rev.as_u64())),
                },
                ConsentCommitOutcome::StaleCurrent { current } => IntentOutcome::StaleBaseView {
                    current: current_mark(record.capability, current.as_ref()),
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
                Some(winner) => Ok(replay_or_conflict(winner, &fingerprint)),
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
            // the same answer; only store failures hold unrecorded. Setup
            // completion is a Stage 2 meaning: it names the dialogue consent
            // only, so a learning assignment can never complete or block it.
            let found: Option<(String, i64, String, String, String)> = tx
                .query_row(
                    SQL_SELECT_CONSENT,
                    params![CapabilityKind::Dialogue.as_str()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            let current = match found {
                Some((id, stored_rev, provider, model, credential_id)) => Some(
                    decode_consent(
                        CapabilityKind::Dialogue,
                        id,
                        stored_rev,
                        provider,
                        model,
                        credential_id,
                    )
                    .map_err(permission_unavailable)?,
                ),
                None => None,
            };
            let expected = parse_consent_mark(&expected_base, CapabilityKind::Dialogue);
            let current_state = current.as_ref().map(|record| record.rev.as_u64());
            let outcome = match expected {
                None => IntentOutcome::StaleBaseView {
                    current: current_mark(CapabilityKind::Dialogue, current.as_ref()),
                },
                Some(expected_state) if expected_state != current_state => {
                    IntentOutcome::StaleBaseView {
                        current: current_mark(CapabilityKind::Dialogue, current.as_ref()),
                    }
                }
                Some(_) if current.is_some() && bearer_present => IntentOutcome::AppliedAsOneTime,
                Some(_) => IntentOutcome::NeedsClarification,
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
                Some(winner) => Ok(replay_or_conflict(winner, &decided.fingerprint)),
            }
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
            // Write-once claim first: an existing row decides without reading
            // consent, so a concurrent same-id send can neither fork the answer
            // nor re-run the route check.
            if let Some(stored) =
                select_intent_row_tx(&tx, &fingerprint.intent_id).map_err(permission_unavailable)?
            {
                return Ok(replay_or_conflict(stored, &fingerprint));
            }
            // No consent state changes either way: the only write, on a hit,
            // records the replay row.
            let found: Option<(String, i64, String, String, String)> = tx
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
                .map_err(|error| permission_unavailable(error.to_string()))?;
            let current = match found {
                Some((id, stored_rev, provider, model, credential_id)) => Some(
                    decode_consent(capability, id, stored_rev, provider, model, credential_id)
                        .map_err(permission_unavailable)?,
                ),
                None => None,
            };
            let record = match current {
                Some(record)
                    if record.provider == provider
                        && record.model == model
                        && record.credential_id == credential_id =>
                {
                    record
                }
                current => {
                    return Ok(IntentResolution::Decided(ShortcutIntentOutcome::Miss {
                        current,
                    }));
                }
            };
            let snapshot = IntentOutcome::StoredAsRuleView {
                revision: consent_mark(capability, Some(record.rev.as_u64())),
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
                Some(winner) => Ok(replay_or_conflict(winner, &fingerprint)),
            }
        })
        .await
    }
}
