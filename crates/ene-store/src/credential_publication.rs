use std::sync::Arc;

use ene_credential::{
    ActivationOutcome, CredentialPublicationRepository, CredentialRef, CredentialTechnicalError,
    MutationKind, MutationOutcome, MutationPhase, RetiredCredentialVersion, SecretVersionId,
    UncommittedMutationOutcome, VersionedCredentialStore,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{credential_unavailable, lock_shared};
use crate::credential::{
    SQL_UPSERT_CREDENTIAL, advance_credential_set, current_set_revision, sweep_registered_secret,
};
use crate::run_blocking;

const SQL_INSERT_MUTATION: &str = "INSERT INTO credential_mutation (mutation_id, op, provider, label, expected_revision, candidate_version, phase, decided_outcome, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)";

const SQL_SELECT_MUTATION: &str = "SELECT op, provider, label, expected_revision, candidate_version, phase, decided_outcome FROM credential_mutation WHERE mutation_id = ?1";

const SQL_MARK_STAGED: &str = "UPDATE credential_mutation SET phase = ?3 WHERE mutation_id = ?1 AND candidate_version = ?2 AND phase = ?4 AND decided_outcome IS NULL";

const SQL_DECIDE_MUTATION: &str = "UPDATE credential_mutation SET phase = ?2, decided_outcome = ?3 WHERE mutation_id = ?1 AND decided_outcome IS NULL";

const SQL_UPSERT_ACTIVE: &str = "INSERT INTO credential_active (provider, label, active_version) VALUES (?1, ?2, ?3) ON CONFLICT (provider, label) DO UPDATE SET active_version = excluded.active_version";

const SQL_SELECT_ACTIVE: &str =
    "SELECT active_version FROM credential_active WHERE provider = ?1 AND label = ?2";

fn stored_active_version(
    conn: &rusqlite::Connection,
    provider: &str,
    label: &str,
) -> Result<Option<u64>, CredentialTechnicalError> {
    let active: Option<Option<i64>> = conn
        .query_row(SQL_SELECT_ACTIVE, params![provider, label], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| credential_unavailable(error.to_string()))?;
    Ok(active.flatten().and_then(|value| u64::try_from(value).ok()))
}

const SQL_ENQUEUE_RETIRED: &str = "INSERT INTO credential_retired (provider, label, version, mutation_id, retired_at) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_PENDING_RETIRED: &str = "SELECT provider, label, version, mutation_id FROM credential_retired ORDER BY rowid ASC LIMIT ?1";

const SQL_DELETE_RETIRED: &str =
    "DELETE FROM credential_retired WHERE provider = ?1 AND label = ?2 AND version = ?3";

const SQL_COMPLETE_RETIRED_MUTATION: &str = "UPDATE credential_mutation SET phase = ?2 WHERE mutation_id = ?1 AND phase = ?3 AND decided_outcome IS NOT NULL AND NOT EXISTS (SELECT 1 FROM credential_retired WHERE mutation_id = ?1)";

fn outcome_text(outcome: &MutationOutcome) -> String {
    match outcome {
        MutationOutcome::Activated { revision } => format!("activated:{revision}"),
        MutationOutcome::Revoked { revision } => format!("revoked:{revision}"),
        MutationOutcome::Stale => String::from("stale"),
        MutationOutcome::Rejected => String::from("rejected"),
        MutationOutcome::Refused => String::from("refused"),
        MutationOutcome::Unknown => String::from("unknown"),
    }
}

fn uncommitted_outcome(outcome: UncommittedMutationOutcome) -> MutationOutcome {
    match outcome {
        UncommittedMutationOutcome::Stale => MutationOutcome::Stale,
        UncommittedMutationOutcome::Rejected => MutationOutcome::Rejected,
        UncommittedMutationOutcome::Refused => MutationOutcome::Refused,
        UncommittedMutationOutcome::Unknown => MutationOutcome::Unknown,
    }
}

fn parse_outcome(text: &str) -> Option<MutationOutcome> {
    match text {
        "stale" => Some(MutationOutcome::Stale),
        "rejected" => Some(MutationOutcome::Rejected),
        "refused" => Some(MutationOutcome::Refused),
        "unknown" => Some(MutationOutcome::Unknown),
        other => {
            let (kind, revision) = other.split_once(':')?;
            let revision = revision.parse::<u64>().ok()?;
            match kind {
                "activated" => Some(MutationOutcome::Activated { revision }),
                "revoked" => Some(MutationOutcome::Revoked { revision }),
                _ => None,
            }
        }
    }
}

fn mutation_from_row(
    mutation_id: String,
    row: &rusqlite::Row<'_>,
) -> Result<Option<ene_credential::CredentialMutation>, rusqlite::Error> {
    let op: String = row.get(0)?;
    let provider: String = row.get(1)?;
    let label: String = row.get(2)?;
    let expected_revision: Option<i64> = row.get(3)?;
    let candidate_version: Option<i64> = row.get(4)?;
    let phase: String = row.get(5)?;
    let decided_outcome: Option<String> = row.get(6)?;
    let (Some(kind), Some(phase)) = (MutationKind::parse(&op), MutationPhase::parse(&phase)) else {
        return Ok(None);
    };
    let outcome = match decided_outcome.as_deref().map(parse_outcome) {
        None => None,
        Some(None) => return Ok(None),
        Some(Some(outcome)) => Some(outcome),
    };
    if expected_revision.is_some_and(|value| value < 0)
        || candidate_version.is_some_and(|value| value < 0)
    {
        return Ok(None);
    }
    Ok(Some(ene_credential::CredentialMutation {
        mutation_id,
        kind,
        provider,
        label,
        expected_revision: expected_revision.and_then(|value| u64::try_from(value).ok()),
        candidate_version: candidate_version
            .and_then(|value| u64::try_from(value).ok())
            .map(SecretVersionId::from_u64),
        phase,
        outcome,
    }))
}

fn load_mutation(
    conn: &rusqlite::Connection,
    mutation_id: &str,
) -> Result<Option<Option<ene_credential::CredentialMutation>>, CredentialTechnicalError> {
    conn.query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
        mutation_from_row(mutation_id.to_owned(), row)
    })
    .optional()
    .map_err(|error| credential_unavailable(error.to_string()))
}

fn abandon_stale(
    tx: rusqlite::Transaction<'_>,
    mutation_id: &str,
    candidate_bearer: Option<&str>,
    retired_bearer: Option<&str>,
) -> Result<(), CredentialTechnicalError> {
    if let Some(bearer) = candidate_bearer {
        sweep_registered_secret(&tx, bearer)?;
    }
    if let Some(retired) = retired_bearer {
        sweep_registered_secret(&tx, retired)?;
    }
    tx.execute(
        SQL_DECIDE_MUTATION,
        params![
            mutation_id,
            MutationPhase::Abandoned.as_str(),
            outcome_text(&MutationOutcome::Stale),
        ],
    )
    .map_err(|error| credential_unavailable(error.to_string()))?;
    tx.commit()
        .map_err(|error| credential_unavailable(error.to_string()))?;
    Ok(())
}

impl CredentialPublicationRepository for Store {
    async fn begin_credential_mutation(
        &self,
        mutation_id: String,
        kind: MutationKind,
        provider: String,
        label: String,
        expected_revision: Option<u64>,
        candidate_version: Option<SecretVersionId>,
    ) -> Result<ene_credential::CredentialMutation, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let stored = load_mutation(&tx, &mutation_id)?;
            if let Some(Some(stored)) = stored {
                if stored.kind != kind
                    || stored.provider != provider
                    || stored.label != label
                    || stored.expected_revision != expected_revision
                    || stored.candidate_version != candidate_version
                {
                    return Err(credential_unavailable(
                        "the mutation id is already bound to another credential premise",
                    ));
                }
                return Ok(stored);
            }
            if let Some(None) = stored {
                return Err(credential_unavailable(
                    "stored credential mutation is malformed",
                ));
            }
            tx.execute(
                SQL_INSERT_MUTATION,
                params![
                    mutation_id,
                    kind.as_str(),
                    provider,
                    label,
                    expected_revision.map(|value| value as i64),
                    candidate_version.map(|value| value.as_u64() as i64),
                    MutationPhase::Prepared.as_str(),
                    ene_primitive::WallClockWithTz::now().to_rfc3339(),
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let created = ene_credential::CredentialMutation {
                mutation_id,
                kind,
                provider,
                label,
                expected_revision,
                candidate_version,
                phase: MutationPhase::Prepared,
                outcome: None,
            };
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(created)
        })
        .await
    }

    async fn mark_credential_staged(
        &self,
        mutation_id: &str,
        candidate: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let changed = guard
                .execute(
                    SQL_MARK_STAGED,
                    params![
                        mutation_id,
                        candidate.as_u64() as i64,
                        MutationPhase::Staged.as_str(),
                        MutationPhase::Prepared.as_str(),
                    ],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            if changed == 0 {
                return Err(credential_unavailable(
                    "the mutation is already decided or unknown",
                ));
            }
            Ok(())
        })
        .await
    }

    async fn activate_credential(
        &self,
        mutation_id: &str,
        candidate_bearer: &str,
        retired_bearer: Option<&str>,
    ) -> Result<ActivationOutcome, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        let candidate_bearer = candidate_bearer.to_owned();
        let retired_bearer = retired_bearer.map(str::to_owned);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some(Some(mutation)) = load_mutation(&tx, &mutation_id)? else {
                return Ok(ActivationOutcome::Missing);
            };
            if let Some(outcome) = mutation.outcome {
                return Ok(ActivationOutcome::AlreadyDecided(outcome));
            }
            let Some(candidate) = mutation.candidate_version else {
                return Ok(ActivationOutcome::Missing);
            };
            if mutation.phase != MutationPhase::Staged {
                return Ok(ActivationOutcome::Missing);
            }
            let current = current_set_revision(&tx)?.as_u64();
            if let Some(expected) = mutation.expected_revision
                && expected != current
            {
                abandon_stale(
                    tx,
                    &mutation_id,
                    Some(candidate_bearer.as_str()),
                    retired_bearer.as_deref(),
                )?;
                return Ok(ActivationOutcome::Stale {
                    current_revision: current,
                });
            }
            sweep_registered_secret(&tx, &candidate_bearer)?;
            if let Some(retired) = retired_bearer.as_deref() {
                sweep_registered_secret(&tx, retired)?;
            }
            tx.execute(
                SQL_UPSERT_CREDENTIAL,
                params![
                    format!("{}:{}", mutation.provider, mutation.label),
                    mutation.provider,
                    mutation.label,
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let previous_active = stored_active_version(&tx, &mutation.provider, &mutation.label)?;
            let retired = previous_active.filter(|version| *version != candidate.as_u64());
            if let Some(version) = retired {
                tx.execute(
                    SQL_ENQUEUE_RETIRED,
                    params![
                        mutation.provider,
                        mutation.label,
                        version as i64,
                        mutation_id.as_str(),
                        ene_primitive::WallClockWithTz::now().to_rfc3339(),
                    ],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            }
            tx.execute(
                SQL_UPSERT_ACTIVE,
                params![mutation.provider, mutation.label, candidate.as_u64() as i64],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let next = advance_credential_set(&tx)?;
            let phase = if retired.is_some() {
                MutationPhase::CleanupPending
            } else {
                MutationPhase::Activated
            };
            tx.execute(
                SQL_DECIDE_MUTATION,
                params![
                    mutation_id,
                    phase.as_str(),
                    outcome_text(&MutationOutcome::Activated { revision: next }),
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(ActivationOutcome::Activated {
                revision: next,
                retired: retired.map(SecretVersionId::from_u64),
            })
        })
        .await
    }

    async fn revoke_credential(
        &self,
        mutation_id: &str,
        retired_bearer: Option<&str>,
    ) -> Result<ActivationOutcome, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        let retired_bearer = retired_bearer.map(str::to_owned);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some(Some(mutation)) = load_mutation(&tx, &mutation_id)? else {
                return Ok(ActivationOutcome::Missing);
            };
            if let Some(outcome) = mutation.outcome {
                return Ok(ActivationOutcome::AlreadyDecided(outcome));
            }
            if mutation.kind != MutationKind::Revoke || mutation.phase != MutationPhase::Prepared {
                return Ok(ActivationOutcome::Missing);
            }
            let current = current_set_revision(&tx)?.as_u64();
            if let Some(expected) = mutation.expected_revision
                && expected != current
            {
                abandon_stale(tx, &mutation_id, None, retired_bearer.as_deref())?;
                return Ok(ActivationOutcome::Stale {
                    current_revision: current,
                });
            }
            if let Some(retired) = retired_bearer.as_deref() {
                sweep_registered_secret(&tx, retired)?;
            }
            let previous_active = stored_active_version(&tx, &mutation.provider, &mutation.label)?;
            let retired = previous_active;
            if let Some(version) = retired {
                tx.execute(
                    SQL_ENQUEUE_RETIRED,
                    params![
                        mutation.provider,
                        mutation.label,
                        version as i64,
                        mutation_id.as_str(),
                        ene_primitive::WallClockWithTz::now().to_rfc3339(),
                    ],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            }
            tx.execute(
                SQL_UPSERT_ACTIVE,
                params![mutation.provider, mutation.label, Option::<i64>::None],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                "DELETE FROM credential_ref WHERE provider = ?1 AND label = ?2",
                params![mutation.provider, mutation.label],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            let next = advance_credential_set(&tx)?;
            let phase = if retired.is_some() {
                MutationPhase::CleanupPending
            } else {
                MutationPhase::Completed
            };
            tx.execute(
                SQL_DECIDE_MUTATION,
                params![
                    mutation_id,
                    phase.as_str(),
                    outcome_text(&MutationOutcome::Revoked { revision: next }),
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(ActivationOutcome::Activated {
                revision: next,
                retired: retired.map(SecretVersionId::from_u64),
            })
        })
        .await
    }

    async fn record_credential_mutation_outcome(
        &self,
        mutation_id: &str,
        outcome: UncommittedMutationOutcome,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            guard
                .execute(
                    SQL_DECIDE_MUTATION,
                    params![
                        mutation_id,
                        MutationPhase::Abandoned.as_str(),
                        outcome_text(&uncommitted_outcome(outcome)),
                    ],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }

    async fn credential_mutation(
        &self,
        mutation_id: &str,
    ) -> Result<Option<ene_credential::CredentialMutation>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            load_mutation(&guard, &mutation_id).map(Option::flatten)
        })
        .await
    }

    async fn active_credential_version(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<SecretVersionId>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            Ok(stored_active_version(&guard, &provider, &label)?.map(SecretVersionId::from_u64))
        })
        .await
    }
}

impl Store {
    pub fn sweep_retired_credentials<S: VersionedCredentialStore>(
        &self,
        values: &S,
        limit: u32,
    ) -> Result<u32, CredentialTechnicalError> {
        if limit == 0 {
            return Ok(0);
        }
        let pending: Vec<RetiredCredentialVersion> = {
            let guard = lock_shared(&self.conn);
            let mut query = guard
                .prepare(SQL_PENDING_RETIRED)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let rows = query
                .query_map(params![i64::from(limit)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let mut retired = Vec::new();
            for row in rows {
                let (provider, label, version, mutation_id) =
                    row.map_err(|error| credential_unavailable(error.to_string()))?;
                let version = u64::try_from(version)
                    .map_err(|_| credential_unavailable("retired version out of range"))?;
                retired.push(RetiredCredentialVersion {
                    provider,
                    label,
                    version: SecretVersionId::from_u64(version),
                    mutation_id,
                });
            }
            retired
        };
        let mut confirmed = Vec::new();
        for row in pending {
            let Ok(credential) = CredentialRef::new(row.provider.clone(), row.label.clone()) else {
                continue;
            };
            if values
                .delete_version(&credential, row.version.as_u64())
                .is_ok()
            {
                confirmed.push(row);
            }
        }
        if confirmed.is_empty() {
            return Ok(0);
        }
        let mut guard = lock_shared(&self.conn);
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| credential_unavailable(error.to_string()))?;
        for row in &confirmed {
            tx.execute(
                SQL_DELETE_RETIRED,
                params![row.provider, row.label, row.version.as_u64() as i64],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_COMPLETE_RETIRED_MUTATION,
                params![
                    row.mutation_id,
                    MutationPhase::Completed.as_str(),
                    MutationPhase::CleanupPending.as_str(),
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| credential_unavailable(error.to_string()))?;
        Ok(confirmed.len() as u32)
    }
}
