use std::sync::Arc;

use ene_credential::{
    CredentialApprovalRepository, CredentialErasureOutcome, CredentialErasureRepository,
    CredentialIntentRepository, CredentialRef, CredentialRefRepository, CredentialSetRepository,
    CredentialSetRevision, CredentialStore, CredentialTechnicalError, DeviceId,
    DevicePairingRepository, DeviceRecord, PairingSecretMaterial, PendingCredentialApproval,
    PendingPairing, REDACTED_CREDENTIAL, RegistrationApply, RegistrationFingerprint,
    RegistrationState,
};
use ene_permission::{IntentFingerprint, IntentOutcome};
use ene_preservation::ErasureConditionRef;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    SQL_INSERT_CREDENTIAL_PENDING_IGNORE, SQL_SELECT_CREDENTIAL, credential_pair_is_blank,
    credential_unavailable, decode_device_record, decode_pending_credential,
    decode_pending_pairing, encode_id, insert_decided_row_tx, lock_shared, select_intent_row_tx,
};
use crate::erasure::{ERASURE_BATCH_ROWS, erasure_count};
use crate::preservation::condition_is_current;
use crate::run_blocking;

pub(crate) const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";

pub(crate) const SQL_SELECT_SET_REV: &str = "SELECT rev FROM credential_set WHERE id = 1";

const SQL_LIST_CREDENTIALS: &str = "SELECT id, provider, label FROM credential_ref ORDER BY id ASC";

const SQL_SELECT_PENDING_BY_ID: &str = "SELECT pending_id, descriptor, requested_at, origin_connection FROM pairing_pending WHERE pending_id = ?1";

const SQL_INSERT_PENDING: &str = "INSERT INTO pairing_pending (pending_id, descriptor, requested_at, origin_connection) VALUES (?1, ?2, ?3, ?4)";

const SQL_DELETE_PENDING: &str =
    "DELETE FROM pairing_pending WHERE pending_id = ?1 AND origin_connection = ?2";

const SQL_CLEAR_PENDING: &str = "DELETE FROM pairing_pending";

const SQL_ABANDON_PENDING_BY_ORIGIN: &str =
    "DELETE FROM pairing_pending WHERE origin_connection = ?1";

const SQL_INSERT_PAIRED: &str =
    "INSERT INTO paired_device (device_id, descriptor, paired_at, wire) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_DEVICE_BY_WIRE: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE wire = ?1";

const SQL_LIST_PENDING_PAIRINGS: &str = "SELECT pending_id, descriptor, requested_at, origin_connection FROM pairing_pending ORDER BY rowid ASC";

const SQL_SELECT_CREDENTIAL_PENDING: &str = "SELECT provider, label, requested_at FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_DELETE_CREDENTIAL_PENDING: &str =
    "DELETE FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_LIST_CREDENTIAL_PENDING: &str =
    "SELECT provider, label, requested_at FROM credential_pending ORDER BY rowid ASC";

fn fresh_pairing_secret() -> PairingSecretMaterial {
    PairingSecretMaterial::new(RawId::new().as_uuid().to_string())
}

impl Store {
    pub fn approve_credential_with_sweep(
        &self,
        provider: &str,
        label: &str,
        bearer: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        if credential_pair_is_blank(provider, label) {
            return Ok(false);
        }
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| credential_unavailable(error.to_string()))?;
        sweep_registered_secret(&tx, bearer)?;
        let pending = tx
            .query_row(
                SQL_SELECT_CREDENTIAL_PENDING,
                params![provider, label],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| credential_unavailable(error.to_string()))?
            .is_some();
        let approved = if pending {
            tx.execute(SQL_DELETE_CREDENTIAL_PENDING, params![provider, label])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_UPSERT_CREDENTIAL,
                params![format!("{provider}:{label}"), provider, label],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            true
        } else {
            tx.query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |_| Ok(()))
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?
                .is_some()
        };
        if approved {
            advance_credential_set(&tx)?;
        }
        tx.commit()
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(approved)
    }
}

/// Marker-language passes bounded before the fallback removal. A replacement
/// can re-form the bearer across the marker, and a bearer that is a substring
/// of the marker keeps re-matching, so the sweep repeats and then removes.
const SWEEP_PASS_BOUND: usize = 8;

/// Table and column pairs holding quarantined plaintext content.
///
/// The derived recall token index is deliberately absent: a registered value
/// is replaced as a whole string, while tokens hold its fragments, so a
/// replace would leave credential-derived pieces behind. Token rows are
/// rebuilt from the swept canonical text instead (see below).
///
/// Task and activity bodies are included because they are canonical sources
/// for the Task report, the management view, and undelivered excerpts: a
/// purpose, instruction activity, or final result recorded while the value
/// was still ordinary text must be redacted by the same boundary, or the
/// report/presentation would keep reading the raw value out of the owner row
/// after the value became a registered credential.
const SWEEP_TARGETS: &[(&str, &str)] = &[
    ("activity_record", "body"),
    ("history_message", "body"),
    ("learning_summary", "content"),
    ("learning_memory", "content"),
    ("learning_memory_revision", "content"),
    ("management_intent", "rationale_quote"),
    ("task", "purpose_text"),
    ("task_revision", "purpose_text"),
    ("task_result", "body"),
];

pub(crate) fn sweep_registered_secret(
    tx: &rusqlite::Transaction<'_>,
    bearer: &str,
) -> Result<(), CredentialTechnicalError> {
    if bearer.is_empty() {
        return Ok(());
    }
    let affected: Vec<(String, String)> = {
        let mut select = tx
            .prepare(
                "SELECT memory_id, companion_id FROM learning_memory WHERE instr(content, ?1) > 0",
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
        select
            .query_map(params![bearer], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| credential_unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| credential_unavailable(error.to_string()))?
    };
    // Table and column names are compile-time constants; the bearer travels
    // only as a bound parameter. The bounded marker passes and the removal
    // fallback mirror `erasure::redact_exact`: a single `replace` can re-form
    // the bearer across the marker (or reproduce a bearer that is a substring
    // of it), and removal strictly shortens the value so it reaches a clean
    // fixpoint. Every matched row is modified, so `changed == 0` proves no
    // row matches; no separate residual probe can find one.
    for (table, column) in SWEEP_TARGETS {
        let replace = format!(
            "UPDATE {table} SET {column} = replace({column}, ?1, ?2) \
             WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
        );
        for _ in 0..SWEEP_PASS_BOUND {
            let changed = tx
                .execute(&replace, params![bearer, REDACTED_CREDENTIAL])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if changed == 0 {
                break;
            }
        }
        let remove = format!(
            "UPDATE {table} SET {column} = replace({column}, ?1, '') \
             WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
        );
        loop {
            let changed = tx
                .execute(&remove, params![bearer])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if changed == 0 {
                break;
            }
        }
    }
    for (memory, companion) in &affected {
        let content: String = tx
            .query_row(
                "SELECT content FROM learning_memory WHERE memory_id = ?1",
                params![memory],
                |row| row.get(0),
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
        crate::learning::rebuild_memory_terms_tx(tx, memory, companion, &content)
            .map_err(|error| credential_unavailable(error.to_string()))?;
    }
    Ok(())
}

/// Advances the credential-set revision inside the caller's transaction and
/// returns the new revision.
pub(crate) fn advance_credential_set(
    tx: &rusqlite::Transaction<'_>,
) -> Result<u64, CredentialTechnicalError> {
    let current: i64 = tx
        .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
        .map_err(|error| credential_unavailable(error.to_string()))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| credential_unavailable("credential set revision exhausted"))?;
    // Refuse a stored count that cannot be a revision (a corrupt negative
    // value) before it can persist.
    let revision = u64::try_from(next)
        .map_err(|_| credential_unavailable("credential set revision out of range"))?;
    tx.execute(
        "UPDATE credential_set SET rev = ?1 WHERE id = 1",
        params![next],
    )
    .map_err(|error| credential_unavailable(error.to_string()))?;
    Ok(revision)
}

pub(crate) fn current_set_revision(
    conn: &rusqlite::Connection,
) -> Result<CredentialSetRevision, CredentialTechnicalError> {
    let stored_rev: i64 = conn
        .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
        .map_err(|error| credential_unavailable(error.to_string()))?;
    let revision = u64::try_from(stored_rev)
        .map_err(|_| credential_unavailable("credential set revision out of range"))?;
    Ok(CredentialSetRevision::from_u64(revision))
}

impl CredentialSetRepository for Store {
    async fn current_set_revision(
        &self,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            current_set_revision(&guard)
        })
        .await
    }
}

impl Store {
    pub fn sweep_registered_values<S: CredentialStore>(
        &self,
        refs: &[CredentialRef],
        values: &S,
    ) -> Result<(), CredentialTechnicalError> {
        if refs.is_empty() {
            return Ok(());
        }
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| credential_unavailable(error.to_string()))?;
        for cred in refs {
            values.with_bearer(cred, |bearer| sweep_registered_secret(&tx, bearer))??;
        }
        advance_credential_set(&tx)?;
        tx.commit()
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(())
    }
}

impl CredentialRefRepository for Store {
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

impl DevicePairingRepository for Store {
    async fn request_pairing(
        &self,
        descriptor: String,
        origin_connection: String,
    ) -> Result<PendingPairing, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let requested = WallClockWithTz::now();
            let requested_text = requested.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let fresh = RawId::new().as_uuid().to_string();
            tx.execute(
                SQL_INSERT_PENDING,
                params![fresh, descriptor, requested_text, origin_connection],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let stored: Option<(String, String, String, String)> = tx
                .query_row(SQL_SELECT_PENDING_BY_ID, params![fresh], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_id, stored_descriptor, stored_requested, stored_origin)) = stored
            else {
                return Err(credential_unavailable(String::from(
                    "pairing request vanished after insert",
                )));
            };
            let pending = decode_pending_pairing(
                stored_id,
                stored_descriptor,
                &stored_requested,
                stored_origin,
            )
            .map_err(credential_unavailable)?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(pending)
        })
        .await
    }

    async fn approve_pending(
        &self,
        pending_id: &str,
        origin_connection: &str,
    ) -> Result<Option<(DeviceRecord, PairingSecretMaterial)>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let pending_id = pending_id.to_owned();
        let origin_connection = origin_connection.to_owned();
        run_blocking(move || {
            let device_id = RawId::new();
            let device_text = encode_id(device_id);
            let paired_at = WallClockWithTz::now();
            let paired_text = paired_at.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let pending: Option<(String, String, String, String)> = tx
                .query_row(SQL_SELECT_PENDING_BY_ID, params![pending_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_id, stored_descriptor, _, stored_origin)) = pending else {
                return Ok(None);
            };
            if stored_origin != origin_connection {
                return Ok(None);
            }
            // The pending delete and the paired insert share one transaction
            // keyed on both columns (compare-and-swap), so an approval never
            // strands a pending in both tables or neither. The wire projection
            // is minted fresh here, unrelated to the device identity bytes: it
            // is the only device string that ever crosses the wire.
            let wire = RawId::new().as_uuid().to_string();
            let deleted = tx
                .execute(SQL_DELETE_PENDING, params![stored_id, stored_origin])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if deleted != 1 {
                return Ok(None);
            }
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

    async fn abandon_pending_by_origin(
        &self,
        origin_connection: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let origin_connection = origin_connection.to_owned();
        run_blocking(move || {
            lock_shared(&conn)
                .execute(SQL_ABANDON_PENDING_BY_ORIGIN, params![origin_connection])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
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
            let found: Option<(String, String, String, String)> = guard
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

    async fn clear_unapproved_pendings(&self) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            guard
                .execute(SQL_CLEAR_PENDING, ())
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
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
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let mut pending = Vec::new();
            for row in rows {
                let (pending_id, descriptor, requested_text, origin_connection) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                pending.push(
                    decode_pending_pairing(
                        pending_id,
                        descriptor,
                        &requested_text,
                        origin_connection,
                    )
                    .map_err(credential_unavailable)?,
                );
            }
            Ok(pending)
        })
        .await
    }
}

impl CredentialApprovalRepository for Store {
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

impl CredentialIntentRepository for Store {
    async fn request_registration_with_intent(
        &self,
        provider: String,
        label: String,
        fingerprint: RegistrationFingerprint,
    ) -> Result<RegistrationApply, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            if credential_pair_is_blank(&provider, &label) {
                return Err(credential_unavailable(String::from(
                    "blank credential pair",
                )));
            }
            let journal = IntentFingerprint {
                intent_id: fingerprint.intent_id,
                kind: fingerprint.kind,
                target: fingerprint.target,
                base: fingerprint.base,
                rationale_origin: fingerprint.rationale_origin,
                rationale_quote: fingerprint.rationale_quote,
            };
            let requested_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if select_intent_row_tx(&tx, &journal.intent_id)
                .map_err(credential_unavailable)?
                .is_some()
            {
                return Ok(RegistrationApply::AlreadyDecided);
            }
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let (state, outcome) = if usable.is_some() {
                (
                    RegistrationState::AppliedAsOneTime,
                    IntentOutcome::AppliedAsOneTime,
                )
            } else {
                tx.execute(
                    SQL_INSERT_CREDENTIAL_PENDING_IGNORE,
                    params![provider, label, requested_text],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
                (
                    RegistrationState::HeldByOperation,
                    IntentOutcome::HeldByOperation,
                )
            };
            insert_decided_row_tx(&tx, &journal, &outcome).map_err(credential_unavailable)?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(RegistrationApply::Decided(state))
        })
        .await
    }
}

/// Deletes usable refs whose derived identity, provider, or label carries the
/// target. Deleting the ref is the local erasure: the derived
/// `provider:label` identity can never be redacted without breaking the ref
/// grammar, and the pair must not stay usable under a textless identity. The
/// protected bearer value is not touched here (K-C); the pair simply stops
/// being resolvable, and the set revision advances below.
const SQL_ERASE_CREDENTIAL_REF: &str = "DELETE FROM credential_ref
     WHERE id IN (
         SELECT id FROM credential_ref
         WHERE instr(id, ?1) > 0 OR instr(provider, ?1) > 0 OR instr(label, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_CREDENTIAL_PENDING: &str = "DELETE FROM credential_pending
     WHERE rowid IN (
         SELECT rowid FROM credential_pending
         WHERE instr(provider, ?1) > 0 OR instr(label, ?1) > 0
         LIMIT ?2
     )";

const SQL_ERASE_PAIRED_DEVICE: &str = "DELETE FROM paired_device
     WHERE rowid IN (
         SELECT rowid FROM paired_device
          WHERE instr(device_id, ?1) > 0 OR instr(descriptor, ?1) > 0
             OR instr(COALESCE(wire, ''), ?1) > 0
          LIMIT ?2
     )";

const SQL_ERASE_PAIRING_PENDING: &str = "DELETE FROM pairing_pending
     WHERE rowid IN (
         SELECT rowid FROM pairing_pending
         WHERE instr(pending_id, ?1) > 0 OR instr(descriptor, ?1) > 0
            OR instr(origin_connection, ?1) > 0
         LIMIT ?2
     )";

const SQL_COUNT_CREDENTIAL_METADATA_TARGET: &str = "SELECT
     (SELECT COUNT(*) FROM credential_ref
      WHERE instr(id, ?1) > 0 OR instr(provider, ?1) > 0 OR instr(label, ?1) > 0)
   + (SELECT COUNT(*) FROM credential_pending
      WHERE instr(provider, ?1) > 0 OR instr(label, ?1) > 0)
   + (SELECT COUNT(*) FROM paired_device
       WHERE instr(device_id, ?1) > 0 OR instr(descriptor, ?1) > 0
          OR instr(COALESCE(wire, ''), ?1) > 0)
   + (SELECT COUNT(*) FROM pairing_pending
      WHERE instr(pending_id, ?1) > 0 OR instr(descriptor, ?1) > 0
         OR instr(origin_connection, ?1) > 0)";

impl CredentialErasureRepository for Store {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
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
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                if !condition_is_current(&tx, condition)
                    .map_err(|error| credential_unavailable(error.to_string()))?
                {
                    return Ok(CredentialErasureOutcome::NotCurrent);
                }
                let refs = tx
                    .execute(
                        SQL_ERASE_CREDENTIAL_REF,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                let pendings = tx
                    .execute(
                        SQL_ERASE_CREDENTIAL_PENDING,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                let devices = tx
                    .execute(SQL_ERASE_PAIRED_DEVICE, params![target, ERASURE_BATCH_ROWS])
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                let pairing = tx
                    .execute(
                        SQL_ERASE_PAIRING_PENDING,
                        params![target, ERASURE_BATCH_ROWS],
                    )
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                if refs > 0 {
                    advance_credential_set(&tx)?;
                }
                let remainder: i64 = tx
                    .query_row(
                        SQL_COUNT_CREDENTIAL_METADATA_TARGET,
                        params![target],
                        |row| row.get(0),
                    )
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                let erased = erasure_count(refs + pendings + devices + pairing)
                    .map_err(credential_unavailable)?;
                let remainder = erasure_count(remainder).map_err(credential_unavailable)?;
                tx.commit()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                Ok(CredentialErasureOutcome::Applied { erased, remainder })
            })
            .await
        }
    }

    fn condition_is_current(
        &self,
        condition: ErasureConditionRef,
    ) -> impl std::future::Future<Output = Result<bool, CredentialTechnicalError>> + Send {
        let conn = Arc::clone(&self.conn);
        async move {
            run_blocking(move || {
                let guard = lock_shared(&conn);
                condition_is_current(&guard, condition)
                    .map_err(|error| credential_unavailable(error.to_string()))
            })
            .await
        }
    }

    fn before_device_auth_file_erase(&self) -> impl std::future::Future<Output = ()> + Send {
        #[cfg(any(test, feature = "test-support"))]
        {
            let parks = Arc::clone(&self.test_parks);
            async move {
                parks.device_auth_file.pause_if_armed().await;
            }
        }
        #[cfg(not(any(test, feature = "test-support")))]
        std::future::ready(())
    }
}
