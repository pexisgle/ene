//! Durable Client body-delivery evidence (lifecycle §8.1).
//!
//! The Host composition's Client delivery tracking used to be process memory
//! only: a Host crash after the Client received a target-bearing body lost the
//! knowledge that the incarnation may hold a local copy, and a later admission
//! could complete without ever demanding that Client's local erasure. These
//! methods persist the same body-free evidence in the canonical database.
//!
//! Evidence contract (enforced here, documented for callers):
//!
//! - **Create on delivery.** [`Store::note_client_delivery_evidence`] inserts
//!   the incarnation row, or advances its `delivery_seq`, in one statement.
//!   The caller must commit this before handing the body to the transport:
//!   write-ahead is what makes a crash between "Client received" and "Host
//!   recorded" impossible. Only a Host-minted incarnation identity that an
//!   authenticated connection pinned is ever recorded, so the Host never
//!   invents an owner it cannot name.
//! - **Clear only on verified full-class erasure.**
//!   [`Store::clear_client_delivery_evidence`] deletes the row only when the
//!   caller observed `expected_seq` when the demand went on the wire and no
//!   later delivery advanced it. That compare-and-delete is the ordering
//!   guarantee: a body delivered after the wipe leaves a higher sequence and
//!   the row survives. Disconnect, connection replacement, ACK timeout, and
//!   Host restart never call this method.
//! - **Restart retains.** Nothing here expires, sweeps, or rebuilds a row;
//!   opening a database only migrates the schema.
//! - **A new incarnation never inherits.** Rows are keyed by the exact
//!   Host-minted `(counter, random)` identity; the required-participant read
//!   returns those identities verbatim and a different boot is a different
//!   row.
//!
//! The row is deliberately body-free: no target body, no reversible encoding,
//! no body hash/fingerprint, no deletion matcher/search token, and no
//! presentation copy is stored or derivable from it.

use std::sync::Arc;

use ene_preservation::PreservationTechnicalError;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::codec::{decode_id, encode_id, lock_shared};
use crate::{Store, run_blocking};

fn storage(_: rusqlite::Error) -> PreservationTechnicalError {
    PreservationTechnicalError::StorageUnavailable
}

fn corrupt() -> PreservationTechnicalError {
    PreservationTechnicalError::CorruptState
}

impl Store {
    /// Records that body-bearing material is being handed to `incarnation`.
    ///
    /// One durable statement; the caller must await it before the body reaches
    /// the transport. A repeat delivery advances `delivery_seq` instead of
    /// creating a second row, so the sequence is the currentness premise a
    /// verified erasure clears with.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the evidence
    /// cannot be committed. The caller must then withhold the body: a
    /// delivery the Host cannot account for must not leave the Host.
    pub async fn note_client_delivery_evidence(
        &self,
        incarnation: RawId,
    ) -> Result<(), PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let at = WallClockWithTz::now().to_rfc3339();
            tx.execute(
                "INSERT INTO client_delivery_evidence
                     (incarnation_id,delivery_seq,first_delivered_at,last_delivered_at)
                 VALUES (?1,1,?2,?2)
                 ON CONFLICT(incarnation_id) DO UPDATE SET
                     delivery_seq=delivery_seq+1,
                     last_delivered_at=?2",
                params![encode_id(incarnation), at],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)
        })
        .await
    }

    /// The current durable delivery sequence of one incarnation, `None` when
    /// the Host holds no uncleared evidence for it.
    ///
    /// The caller reads this when a local-erasure demand goes on the wire: the
    /// value is the compare-and-delete premise the verified answer must match.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the read fails;
    /// the caller must then fail closed (deliver no demand and claim nothing).
    pub async fn client_delivery_evidence_seq(
        &self,
        incarnation: RawId,
    ) -> Result<Option<u64>, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let stored: Option<i64> = guard
                .query_row(
                    "SELECT delivery_seq FROM client_delivery_evidence WHERE incarnation_id=?1",
                    [encode_id(incarnation)],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            match stored {
                None => Ok(None),
                Some(seq) if seq > 0 => Ok(Some(seq as u64)),
                Some(_) => Err(corrupt()),
            }
        })
        .await
    }

    /// Clears one incarnation's evidence only when no delivery superseded the
    /// wipe: the delete matches `expected_seq` exactly, so a row advanced by a
    /// concurrent or later delivery survives.
    ///
    /// Returns whether the row was cleared. A `false` answer is not an error:
    /// it means the evidence was superseded and must stay.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the delete
    /// cannot commit. The row then stays, which is the conservative direction.
    pub async fn clear_client_delivery_evidence(
        &self,
        incarnation: RawId,
        expected_seq: u64,
    ) -> Result<bool, PreservationTechnicalError> {
        let expected = i64::try_from(expected_seq).map_err(|_| corrupt())?;
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let cleared = tx
                .execute(
                    "DELETE FROM client_delivery_evidence
                     WHERE incarnation_id=?1 AND delivery_seq=?2",
                    params![encode_id(incarnation), expected],
                )
                .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(cleared == 1)
        })
        .await
    }

    /// Fixed-size keyset page of incarnations with uncleared evidence, ordered
    /// by canonical identity text. `limit` is 1..=100.
    ///
    /// Admission walks the pages to completion; it must never truncate the set
    /// (a dropped incarnation would be a missing required participant), so the
    /// page exists to bound each upstream read, not to cap the result.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::InvalidLimit`] for a limit outside the
    /// contract, and [`PreservationTechnicalError::CorruptState`] when a
    /// stored identity is not a canonical id.
    pub async fn client_delivery_evidence_incarnations(
        &self,
        after: Option<RawId>,
        limit: u32,
    ) -> Result<Vec<RawId>, PreservationTechnicalError> {
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let after = after.map(encode_id).unwrap_or_default();
            let mut statement = guard
                .prepare(
                    "SELECT incarnation_id FROM client_delivery_evidence
                     WHERE incarnation_id>?1 ORDER BY incarnation_id LIMIT ?2",
                )
                .map_err(storage)?;
            let rows: Vec<String> = statement
                .query_map(params![after, limit], |row| row.get(0))
                .map_err(storage)?
                .collect::<Result<_, _>>()
                .map_err(storage)?;
            rows.into_iter()
                .map(|text| decode_id(&text).map_err(|_| corrupt()))
                .collect()
        })
        .await
    }
}
