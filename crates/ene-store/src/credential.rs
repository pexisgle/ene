use std::sync::Arc;

use ene_credential::{
    CredentialApprovalRepository, CredentialIntentRepository, CredentialRef,
    CredentialRefRepository, CredentialTechnicalError, DeviceId, DevicePairingRepository,
    DevicePairingStatus, DeviceRecord, PendingCredentialApproval, PendingPairing,
    RegistrationApply, RegistrationFingerprint, RegistrationState,
};
use ene_permission::{IntentFingerprint, IntentOutcome};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    SQL_INSERT_CREDENTIAL_PENDING_IGNORE, SQL_SELECT_CREDENTIAL, credential_pair_is_blank,
    credential_unavailable, decode_device_record, decode_pending_credential,
    decode_pending_pairing, encode_id, insert_decided_row_tx, lock_shared, select_intent_row_tx,
};
use crate::run_blocking;

const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";

const SQL_LIST_CREDENTIALS: &str = "SELECT id, provider, label FROM credential_ref ORDER BY id ASC";

const SQL_SELECT_PAIRED_BY_DESCRIPTOR: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE descriptor = ?1";

const SQL_SELECT_DEVICE_BY_ID: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE device_id = ?1";

const SQL_SELECT_PENDING_BY_DESCRIPTOR: &str =
    "SELECT descriptor, requested_at FROM pairing_pending WHERE descriptor = ?1";

const SQL_INSERT_PENDING_IGNORE: &str =
    "INSERT OR IGNORE INTO pairing_pending (descriptor, requested_at) VALUES (?1, ?2)";

const SQL_DELETE_PENDING: &str = "DELETE FROM pairing_pending WHERE descriptor = ?1";

const SQL_INSERT_PAIRED: &str =
    "INSERT INTO paired_device (device_id, descriptor, paired_at, wire) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_DEVICE_BY_WIRE: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE wire = ?1";

const SQL_LIST_PENDING_PAIRINGS: &str =
    "SELECT descriptor, requested_at FROM pairing_pending ORDER BY rowid ASC";

const SQL_SELECT_CREDENTIAL_PENDING: &str = "SELECT provider, label, requested_at FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_DELETE_CREDENTIAL_PENDING: &str =
    "DELETE FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_LIST_CREDENTIAL_PENDING: &str =
    "SELECT provider, label, requested_at FROM credential_pending ORDER BY rowid ASC";

/// Mints a one-time pairing secret for display-once custody.
///
/// Secrets are never stored: the caller shows the returned string once on a
/// trusted surface and holds it only in memory afterwards.
fn fresh_pairing_secret() -> String {
    RawId::new().as_uuid().to_string()
}

impl CredentialRefRepository for Store {
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            guard
                .execute(
                    SQL_UPSERT_CREDENTIAL,
                    params![cred.id(), cred.provider(), cred.label()],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let found: Option<(String, String, String)> = guard
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((_, provider_name, label_name)) = found else {
                return Ok(None);
            };
            let cred = CredentialRef::new(provider_name, label_name)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(Some(cred))
        })
        .await
    }

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
    ) -> Result<DevicePairingStatus, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let requested = WallClockWithTz::now();
            let requested_text = requested.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // An already-paired descriptor short-circuits: re-requests leave the
            // stored record untouched.
            let paired: Option<(String, String, String, Option<String>)> = tx
                .query_row(
                    SQL_SELECT_PAIRED_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if let Some((device_text, stored_descriptor, paired_text, wire)) = paired {
                let device =
                    decode_device_record(&device_text, stored_descriptor, &paired_text, wire)
                        .map_err(credential_unavailable)?;
                return Ok(DevicePairingStatus::Paired { device });
            }
            // `INSERT OR IGNORE` keeps a previously stored pending entry: the
            // re-read below returns it unchanged instead of refreshing its time.
            tx.execute(
                SQL_INSERT_PENDING_IGNORE,
                params![descriptor, requested_text],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let stored: Option<(String, String)> = tx
                .query_row(
                    SQL_SELECT_PENDING_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_descriptor, stored_requested)) = stored else {
                return Err(credential_unavailable(String::from(
                    "pairing request vanished after insert",
                )));
            };
            let pending = decode_pending_pairing(stored_descriptor, &stored_requested)
                .map_err(credential_unavailable)?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(DevicePairingStatus::Pending { pending })
        })
        .await
    }

    async fn approve_pending(
        &self,
        descriptor: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let descriptor = descriptor.to_owned();
        run_blocking(move || {
            let device_id = RawId::new();
            let device_text = encode_id(device_id);
            let paired_at = WallClockWithTz::now();
            let paired_text = paired_at.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // Re-approving an already-paired descriptor returns the existing
            // record unchanged with a freshly minted secret (rotation); no
            // fresh identity is stored. Secrets are never stored: the caller
            // displays the returned string once on a trusted surface.
            let paired: Option<(String, String, String, Option<String>)> = tx
                .query_row(
                    SQL_SELECT_PAIRED_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if let Some((stored_text, stored_descriptor, stored_paired, wire)) = paired {
                let device =
                    decode_device_record(&stored_text, stored_descriptor, &stored_paired, wire)
                        .map_err(credential_unavailable)?;
                return Ok(Some((device, fresh_pairing_secret())));
            }
            let pending: Option<(String, String)> = tx
                .query_row(
                    SQL_SELECT_PENDING_BY_DESCRIPTOR,
                    params![descriptor],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some((stored_descriptor, _)) = pending else {
                return Ok(None);
            };
            // The pending delete and the paired insert share one transaction so
            // an approval never strands a descriptor in both tables or neither.
            // The wire projection is minted fresh here, unrelated to the device
            // identity bytes: it is the only device string that ever crosses
            // the wire.
            let wire = RawId::new().as_uuid().to_string();
            tx.execute(SQL_DELETE_PENDING, params![descriptor])
                .map_err(|error| credential_unavailable(error.to_string()))?;
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

    async fn find_device(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let id = *id;
        run_blocking(move || {
            let key = encode_id(id.0);
            let guard = lock_shared(&conn);
            let found: Option<(String, String, String, Option<String>)> = guard
                .query_row(SQL_SELECT_DEVICE_BY_ID, params![key], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            match found {
                Some((device_text, descriptor, paired_text, wire)) => {
                    let device = decode_device_record(&device_text, descriptor, &paired_text, wire)
                        .map_err(credential_unavailable)?;
                    Ok(Some(device))
                }
                None => Ok(None),
            }
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
            let found: Option<(String, String, String, Option<String>)> = guard
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

    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let mut query = guard
                .prepare(SQL_LIST_PENDING_PAIRINGS)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let rows = query
                .query_map((), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let mut pending = Vec::new();
            for row in rows {
                let (descriptor, requested_text) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                pending.push(
                    decode_pending_pairing(descriptor, &requested_text)
                        .map_err(credential_unavailable)?,
                );
            }
            Ok(pending)
        })
        .await
    }
}

impl CredentialApprovalRepository for Store {
    async fn request_approval(
        &self,
        provider: String,
        label: String,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store, so
            // they never become stored rows.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let requested_text = WallClockWithTz::now().to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // An already-usable pair short-circuits: re-requests record nothing.
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if usable.is_some() {
                return Ok(false);
            }
            // `INSERT OR IGNORE` keeps a previously stored pending entry: a
            // repeat request reports `false` instead of refreshing its time.
            let inserted = tx
                .execute(
                    SQL_INSERT_CREDENTIAL_PENDING_IGNORE,
                    params![provider, label, requested_text],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if inserted == 0 {
                return Ok(false);
            }
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(true)
        })
        .await
    }

    async fn approve_pending(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // A known pending entry is consumed first so a pair that is somehow
            // both pending and usable never strands its pending row. The delete
            // and the usable-ref upsert share this transaction: a crash between
            // them could otherwise strand an approval with no usable marker (or
            // vice versa), forcing the Owner to re-request and re-approve. The
            // ref id follows the same `provider:label` convention the Host uses
            // when it builds refs for assignment, so both paths name one row.
            let pending: Option<(String, String, String)> = tx
                .query_row(
                    SQL_SELECT_CREDENTIAL_PENDING,
                    params![provider, label],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if pending.is_some() {
                tx.execute(SQL_DELETE_CREDENTIAL_PENDING, params![provider, label])
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                tx.execute(
                    SQL_UPSERT_CREDENTIAL,
                    params![format!("{provider}:{label}"), provider, label,],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
                tx.commit()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                return Ok(true);
            }
            // Re-approving an already-usable pair is idempotent with no change.
            let usable: Option<(String, String, String)> = tx
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(usable.is_some())
        })
        .await
    }

    async fn is_approved(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            // Blank pairs are treated as absent before touching the store.
            if credential_pair_is_blank(&provider, &label) {
                return Ok(false);
            }
            let guard = lock_shared(&conn);
            // Pure load: one statement, no transaction. The `credential_ref`
            // row is the usable marker; pending-only pairs report `false`.
            let found: Option<(String, String, String)> = guard
                .query_row(SQL_SELECT_CREDENTIAL, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(found.is_some())
        })
        .await
    }

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
            // Journal columns stay shared with the consent intents; the
            // registration decision is credential-owned and maps onto them.
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
            // Write-once claim first: an existing row decides without
            // touching credential state; the caller answers from the
            // journal.
            if select_intent_row_tx(&tx, &journal.intent_id)
                .map_err(credential_unavailable)?
                .is_some()
            {
                return Ok(RegistrationApply::AlreadyDecided);
            }
            // One transaction: the pending insert (or usable recheck) plus
            // the replay row, so the decided state and the row that
            // describes it can never strand apart.
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
            match insert_decided_row_tx(&tx, &journal, &outcome).map_err(credential_unavailable)? {
                None => {
                    tx.commit()
                        .map_err(|error| credential_unavailable(error.to_string()))?;
                    Ok(RegistrationApply::Decided(state))
                }
                // Lost a cross-process race after deciding: roll back
                // (dropping `tx` without committing) so the loser changes
                // nothing, and let the caller answer from the journal.
                Some(_winner) => Ok(RegistrationApply::AlreadyDecided),
            }
        })
        .await
    }
}
