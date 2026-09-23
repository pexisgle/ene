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

    pub async fn client_delivery_evidence_incarnations(
        &self,
        after: Option<RawId>,
        limit: u32,
    ) -> Result<Vec<RawId>, PreservationTechnicalError> {
        crate::preservation::check_page_limit(limit)?;
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
