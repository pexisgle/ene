use std::sync::Arc;

use ene_credential::{
    ActivationOutcome, ActiveVersion, CredentialPublicationRepository, CredentialTechnicalError,
    MutationKind, MutationOutcome, MutationPhase, SecretVersionId,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{credential_unavailable, lock_shared};
use crate::credential::{SQL_SELECT_SET_REV, sweep_registered_secret};
use crate::run_blocking;

const SQL_INSERT_MUTATION: &str = "INSERT INTO credential_mutation (mutation_id, op, provider, label, expected_revision, candidate_version, phase, decided_outcome, decided_revision, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8)";

const SQL_SELECT_MUTATION: &str = "SELECT op, provider, label, expected_revision, candidate_version, phase, decided_outcome, decided_revision FROM credential_mutation WHERE mutation_id = ?1";

const SQL_MARK_STAGED: &str = "UPDATE credential_mutation SET phase = ?3 WHERE mutation_id = ?1 AND candidate_version = ?2 AND phase = ?4 AND decided_outcome IS NULL";

const SQL_DECIDE_MUTATION: &str = "UPDATE credential_mutation SET phase = ?2, decided_outcome = ?3, decided_revision = ?4 WHERE mutation_id = ?1 AND decided_outcome IS NULL";

const SQL_UPSERT_ACTIVE: &str = "INSERT INTO credential_active (provider, label, active_version, cleanup_version) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (provider, label) DO UPDATE SET active_version = excluded.active_version, cleanup_version = excluded.cleanup_version";

const SQL_SELECT_ACTIVE: &str = "SELECT active_version, cleanup_version FROM credential_active WHERE provider = ?1 AND label = ?2";

const SQL_CLEAR_CLEANUP: &str = "UPDATE credential_active SET cleanup_version = NULL WHERE provider = ?1 AND label = ?2 AND cleanup_version = ?3";

const SQL_UPSERT_CREDENTIAL: &str = "INSERT INTO credential_ref (id, provider, label) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET provider = excluded.provider, label = excluded.label";

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
    let decided_revision: Option<i64> = row.get(7)?;
    let (Some(kind), Some(phase)) = (MutationKind::parse(&op), MutationPhase::parse(&phase)) else {
        return Ok(None);
    };
    let outcome = match decided_outcome.as_deref().map(parse_outcome) {
        None => None,
        Some(None) => return Ok(None),
        Some(Some(outcome)) => Some(outcome),
    };
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
        decided_revision: decided_revision.and_then(|value| u64::try_from(value).ok()),
    }))
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
            let stored: Option<Option<ene_credential::CredentialMutation>> = tx
                .query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
                    mutation_from_row(mutation_id.clone(), row)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
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
            let created: Option<ene_credential::CredentialMutation> = tx
                .query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
                    mutation_from_row(mutation_id.clone(), row)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?
                .flatten();
            let created = created.ok_or_else(|| credential_unavailable("mutation read back"))?;
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
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let changed = tx
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
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
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
            let stored: Option<Option<ene_credential::CredentialMutation>> = tx
                .query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
                    mutation_from_row(mutation_id.clone(), row)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some(Some(mutation)) = stored else {
                return Ok(ActivationOutcome::Missing);
            };
            if let Some(outcome) = mutation.outcome {
                return Ok(ActivationOutcome::AlreadyDecided(outcome));
            }
            let Some(candidate) = mutation.candidate_version else {
                return Ok(ActivationOutcome::Missing);
            };
            let current: i64 = tx
                .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let current = u64::try_from(current)
                .map_err(|_| credential_unavailable("credential set revision out of range"))?;
            if let Some(expected) = mutation.expected_revision
                && expected != current
            {
                sweep_registered_secret(&tx, &candidate_bearer)?;
                if let Some(retired) = retired_bearer.as_deref() {
                    sweep_registered_secret(&tx, retired)?;
                }
                tx.execute(
                    SQL_DECIDE_MUTATION,
                    params![
                        mutation_id,
                        MutationPhase::Abandoned.as_str(),
                        outcome_text(&MutationOutcome::Stale),
                        Option::<i64>::None,
                    ],
                )
                .map_err(|error| credential_unavailable(error.to_string()))?;
                tx.commit()
                    .map_err(|error| credential_unavailable(error.to_string()))?;
                return Ok(ActivationOutcome::Stale {
                    current_revision: current,
                });
            }
            let next = current
                .checked_add(1)
                .ok_or_else(|| credential_unavailable("credential set revision exhausted"))?;
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
            let previous: Option<(Option<i64>, Option<i64>)> = tx
                .query_row(
                    SQL_SELECT_ACTIVE,
                    params![mutation.provider, mutation.label],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let (previous_active, previous_cleanup) = previous.unwrap_or((None, None));
            let retired = previous_cleanup.or(previous_active);
            tx.execute(
                SQL_UPSERT_ACTIVE,
                params![
                    mutation.provider,
                    mutation.label,
                    candidate.as_u64() as i64,
                    retired,
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                "UPDATE credential_set SET rev = ?1 WHERE id = 1",
                params![next as i64],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_DECIDE_MUTATION,
                params![
                    mutation_id,
                    MutationPhase::CleanupPending.as_str(),
                    outcome_text(&MutationOutcome::Activated { revision: next }),
                    next as i64,
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(ActivationOutcome::Activated {
                revision: next,
                retired: retired
                    .and_then(|value| u64::try_from(value).ok())
                    .map(SecretVersionId::from_u64),
            })
        })
        .await
    }

    async fn record_credential_mutation_outcome(
        &self,
        mutation_id: &str,
        outcome: MutationOutcome,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_DECIDE_MUTATION,
                params![
                    mutation_id,
                    MutationPhase::Abandoned.as_str(),
                    outcome_text(&outcome),
                    Option::<i64>::None,
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
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
            guard
                .query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
                    mutation_from_row(mutation_id.clone(), row)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))
                .map(Option::flatten)
        })
        .await
    }

    async fn active_credential_version(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<ActiveVersion, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let row: Option<(Option<i64>, Option<i64>)> = guard
                .query_row(SQL_SELECT_ACTIVE, params![provider, label], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let (active, cleanup) = row.unwrap_or((None, None));
            Ok(ActiveVersion {
                active: active
                    .and_then(|value| u64::try_from(value).ok())
                    .map(SecretVersionId::from_u64),
                cleanup: cleanup
                    .and_then(|value| u64::try_from(value).ok())
                    .map(SecretVersionId::from_u64),
            })
        })
        .await
    }

    async fn mark_credential_cleaned(
        &self,
        provider: &str,
        label: &str,
        version: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_CLEAR_CLEANUP,
                params![provider, label, version.as_u64() as i64],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }
}
