use std::sync::Arc;

use ene_credential::{
    CredentialApprovalRepository, CredentialErasureOutcome, CredentialErasureRepository,
    CredentialIntentRepository, CredentialRef, CredentialRefRepository, CredentialSetRepository,
    CredentialSetRevision, CredentialStore, CredentialTechnicalError, DeviceId,
    DevicePairingRepository, DevicePairingStatus, DeviceRecord, PendingCredentialApproval,
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
use crate::preservation::condition_is_current;
use crate::run_blocking;

const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";

pub(crate) const SQL_SELECT_SET_REV: &str = "SELECT rev FROM credential_set WHERE id = 1";

const SQL_LIST_CREDENTIALS: &str = "SELECT id, provider, label FROM credential_ref ORDER BY id ASC";

const SQL_SELECT_PAIRED_BY_PENDING: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE pending_id = ?1";

const SQL_SELECT_PENDING_BY_ID: &str = "SELECT pending_id, descriptor, requested_at, origin_connection FROM pairing_pending WHERE pending_id = ?1";

const SQL_INSERT_PENDING: &str = "INSERT INTO pairing_pending (pending_id, descriptor, requested_at, origin_connection) VALUES (?1, ?2, ?3, ?4)";

const SQL_DELETE_PENDING: &str =
    "DELETE FROM pairing_pending WHERE pending_id = ?1 AND origin_connection = ?2";

const SQL_CLEAR_PENDING: &str = "DELETE FROM pairing_pending";

const SQL_INSERT_PAIRED: &str = "INSERT INTO paired_device (device_id, descriptor, paired_at, wire, pending_id) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_SELECT_DEVICE_BY_WIRE: &str =
    "SELECT device_id, descriptor, paired_at, wire FROM paired_device WHERE wire = ?1";

const SQL_LIST_PENDING_PAIRINGS: &str = "SELECT pending_id, descriptor, requested_at, origin_connection FROM pairing_pending ORDER BY rowid ASC";

const SQL_SELECT_CREDENTIAL_PENDING: &str = "SELECT provider, label, requested_at FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_DELETE_CREDENTIAL_PENDING: &str =
    "DELETE FROM credential_pending WHERE provider = ?1 AND label = ?2";

const SQL_LIST_CREDENTIAL_PENDING: &str =
    "SELECT provider, label, requested_at FROM credential_pending ORDER BY rowid ASC";

/// Secrets are never stored: the caller shows the returned string once on a
/// trusted surface and holds it only in memory afterwards.
fn fresh_pairing_secret() -> String {
    RawId::new().as_uuid().to_string()
}

impl Store {
    /// Approves one credential pair atomically: sweeps existing content,
    /// rebuilds the derived recall tokens from the swept text, makes the ref
    /// usable, and bumps the credential-set revision.
    ///
    /// One `Immediate` transaction owns all four effects, so a scrub premise
    /// taken before the commit is either covered by the sweep (content lands
    /// before) or refused by the revision (content lands after). The caller
    /// runs this while the bearer is borrowed inside
    /// [`ene_credential::CredentialStore::with_bearer`], so the value never
    /// leaves that scope. Returns `true` when the pair is usable after the
    /// call. Unknown pairs return `false` (the sweep still ran); a blank pair
    /// returns `false` without touching state.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// transaction cannot run or commit; the caller must then not make the
    /// credential usable.
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
            // A re-approval may accompany a value change (after a restart or
            // an explicit update), so every successful approval advances the
            // revision. The sweep above and this bump are one transaction, so
            // premises taken before the approval are refused.
            advance_credential_set(&tx)?;
        }
        tx.commit()
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(approved)
    }
}

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

/// Sweeps `bearer` out of durable content inside the caller's transaction.
///
/// Canonical text is swept first then the derived recall tokens are rebuilt
/// from the swept text in the same transaction, so the index can never keep
/// credential-derived fragments the replace cannot see. Only memories whose
/// pre-sweep content held the bearer are rebuilt; the common case stays one
/// cheap probe select.
fn sweep_registered_secret(
    tx: &rusqlite::Transaction<'_>,
    bearer: &str,
) -> Result<(), CredentialTechnicalError> {
    if bearer.is_empty() {
        return Ok(());
    }
    // Memories holding the bearer, collected before the sweep redacts them:
    // the rebuild below needs exactly this set, and nothing else changes.
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
    // only as a bound parameter.
    for (table, column) in SWEEP_TARGETS {
        let sql = format!(
            "UPDATE {table} SET {column} = replace({column}, ?1, ?2) \
             WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
        );
        tx.execute(&sql, params![bearer, REDACTED_CREDENTIAL])
            .map_err(|error| credential_unavailable(error.to_string()))?;
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

/// Advances the credential-set revision inside the caller's transaction.
fn advance_credential_set(tx: &rusqlite::Transaction<'_>) -> Result<(), CredentialTechnicalError> {
    let current: i64 = tx
        .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
        .map_err(|error| credential_unavailable(error.to_string()))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| credential_unavailable("credential set revision exhausted"))?;
    // Refuse a stored count that cannot be a revision (a corrupt negative
    // value) before it can persist; the converted value itself is unused.
    u64::try_from(next)
        .map_err(|_| credential_unavailable("credential set revision out of range"))?;
    tx.execute(
        "UPDATE credential_set SET rev = ?1 WHERE id = 1",
        params![next],
    )
    .map_err(|error| credential_unavailable(error.to_string()))?;
    Ok(())
}

/// Reads the durable credential-set revision inside a caller transaction.
///
/// The read shares the caller's transaction, so a writer can compare the
/// revision against a scrub premise in the same short window as the write it
/// admits: a set advanced between the premise's revision read and the commit
/// is observed here and refuses the write.
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
    /// Sweeps every registered value and advances the revision once.
    ///
    /// The Host startup and explicit update boundaries call this before any
    /// use: one short `Immediate` transaction replaces plaintext occurrences
    /// of the pinned values in durable content and advances the revision
    /// together, so a crash cannot leave the sweep and the generation apart.
    /// Every registered value must be readable: one unreadable value fails the
    /// whole boundary and rolls the transaction back, because replacing an
    /// unverifiable value cannot be proven and advancing past it would serve
    /// content prepared under an unknown set. An empty registry changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when a
    /// registered value cannot be read, the replacement cannot run, or the
    /// transaction cannot commit.
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
            // Fail closed before the revision advance: a value that cannot be
            // read cannot be swept, and `?` drops `tx` without committing, so
            // earlier replacements in this boundary roll back too.
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
        pending_id: Option<String>,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let requested = WallClockWithTz::now();
            let requested_text = requested.to_rfc3339();
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // A poll names the opaque pending identity issued earlier: an
            // approved id resolves to its unchanged paired record (the client
            // learns the device key after Owner approval) from any connection,
            // a waiting id with the same display body returns the stored entry
            // only on its origin connection, and an unknown id (stale after a
            // restart clear, or never issued) falls through to a fresh pending
            // below so the client converges on the new identity. A waiting id
            // polled from a new connection likewise falls through: the mapping
            // is kept only until the origin connection ends, so a new
            // connection always opens a new request (#1389). The descriptor is
            // display-only in every case: it is equality-checked on poll but
            // never a lookup key.
            if let Some(poll) = pending_id {
                let paired: Option<(String, String, String, Option<String>)> = tx
                    .query_row(SQL_SELECT_PAIRED_BY_PENDING, params![poll], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .optional()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                if let Some((device_text, stored_descriptor, paired_text, wire)) = paired {
                    let device =
                        decode_device_record(&device_text, stored_descriptor, &paired_text, wire)
                            .map_err(credential_unavailable)?;
                    return Ok(DevicePairingStatus::Paired { device });
                }
                let stored: Option<(String, String, String, String)> = tx
                    .query_row(SQL_SELECT_PENDING_BY_ID, params![poll], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .optional()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                if let Some((stored_id, stored_descriptor, stored_requested, stored_origin)) =
                    stored
                {
                    if stored_descriptor != descriptor {
                        return Err(credential_unavailable(String::from(
                            "pairing poll body mismatch; open a new request",
                        )));
                    }
                    if stored_origin == origin_connection {
                        let pending = decode_pending_pairing(
                            stored_id,
                            stored_descriptor,
                            &stored_requested,
                            stored_origin,
                        )
                        .map_err(credential_unavailable)?;
                        tx.commit()
                            .map_err(|error| credential_unavailable(error.to_string()))?;
                        return Ok(DevicePairingStatus::Pending { pending });
                    }
                    // Waiting id polled from a new connection: the origin
                    // connection ended (or this is another connection's id), so
                    // this poll opens a new request below. The stored row stays
                    // for the Owner decision, which names the recorded origin.
                }
                // Unknown poll id: stale (or never issued). Fall through and
                // mint a fresh pending below.
            }
            // Every new request mints its own opaque identity, even for an
            // identical descriptor: same-descriptor requests never share a
            // pending or a device (#1389).
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
            Ok(DevicePairingStatus::Pending { pending })
        })
        .await
    }

    async fn approve_pending(
        &self,
        pending_id: &str,
        origin_connection: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError> {
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
            // An already-approved id returns its unchanged record with a
            // freshly minted secret (rotation); no second device is minted
            // for one pending.
            let paired: Option<(String, String, String, Option<String>)> = tx
                .query_row(SQL_SELECT_PAIRED_BY_PENDING, params![pending_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if let Some((stored_text, stored_descriptor, stored_paired, wire)) = paired {
                let device =
                    decode_device_record(&stored_text, stored_descriptor, &stored_paired, wire)
                        .map_err(credential_unavailable)?;
                return Ok(Some((device, fresh_pairing_secret())));
            }
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
            // is the only device string that ever crosses the wire. The
            // originating pending id is recorded on the paired row so a later
            // poll for the same id resolves to this record.
            let wire = RawId::new().as_uuid().to_string();
            let deleted = tx
                .execute(SQL_DELETE_PENDING, params![stored_id, stored_origin])
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if deleted != 1 {
                return Ok(None);
            }
            tx.execute(
                SQL_INSERT_PAIRED,
                params![device_text, stored_descriptor, paired_text, wire, stored_id],
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
                // Loser of a cross-process race: roll back (never commit) and
                // let the caller answer from the journal.
                Some(_winner) => Ok(RegistrationApply::AlreadyDecided),
            }
        })
        .await
    }
}

/// Bounded rows deleted per table in one local erasure pass (lifecycle §9).
const ERASURE_BATCH_ROWS: i64 = 500;

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

/// Device keys, display descriptors, and the opaque wire/pending tokens are
/// the pairing rows' text surface. Host-stamped times are not caller text and
/// are never matched.
const SQL_ERASE_PAIRED_DEVICE: &str = "DELETE FROM paired_device
     WHERE rowid IN (
         SELECT rowid FROM paired_device
         WHERE instr(device_id, ?1) > 0 OR instr(descriptor, ?1) > 0
            OR instr(COALESCE(wire, ''), ?1) > 0
            OR instr(COALESCE(pending_id, ''), ?1) > 0
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
         OR instr(COALESCE(wire, ''), ?1) > 0
         OR instr(COALESCE(pending_id, ''), ?1) > 0)
   + (SELECT COUNT(*) FROM pairing_pending
      WHERE instr(pending_id, ?1) > 0 OR instr(descriptor, ?1) > 0
         OR instr(origin_connection, ?1) > 0)";

fn erasure_count(value: i64) -> Result<u64, CredentialTechnicalError> {
    u64::try_from(value).map_err(|_| credential_unavailable("count out of range"))
}

impl CredentialErasureRepository for Store {
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
    > + Send {
        let conn = Arc::clone(&self.conn);
        let target = target.to_owned();
        async move {
            run_blocking(move || {
                let mut guard = lock_shared(&conn);
                let tx = guard
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                // Stale-generation rejection and the mutation share one short
                // transaction: a superseded sweep or a completed operation
                // mutates nothing (lifecycle §6-§7/§9.1).
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
                    // The usable set changed: stale scrub premises must be
                    // refused, so the revision advances once in this same
                    // transaction. A duplicate pass deletes nothing and never
                    // advances it again.
                    advance_credential_set(&tx)?;
                }
                let remainder: i64 = tx
                    .query_row(
                        SQL_COUNT_CREDENTIAL_METADATA_TARGET,
                        params![target],
                        |row| row.get(0),
                    )
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                let erased = erasure_count(
                    i64::try_from(refs + pendings + devices + pairing)
                        .map_err(|_| credential_unavailable("count out of range"))?,
                )?;
                let remainder = erasure_count(remainder)?;
                tx.commit()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                Ok(CredentialErasureOutcome::Applied { erased, remainder })
            })
            .await
        }
    }
}
