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
    SQL_SELECT_SET_REV, SQL_UPSERT_CREDENTIAL, advance_credential_set, sweep_registered_secret,
};
use crate::run_blocking;

const SQL_INSERT_MUTATION: &str = "INSERT INTO credential_mutation (mutation_id, op, provider, label, expected_revision, candidate_version, phase, decided_outcome, decided_revision, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8)";

const SQL_SELECT_MUTATION: &str = "SELECT op, provider, label, expected_revision, candidate_version, phase, decided_outcome, decided_revision FROM credential_mutation WHERE mutation_id = ?1";

const SQL_MARK_STAGED: &str = "UPDATE credential_mutation SET phase = ?3 WHERE mutation_id = ?1 AND candidate_version = ?2 AND phase = ?4 AND decided_outcome IS NULL";

const SQL_DECIDE_MUTATION: &str = "UPDATE credential_mutation SET phase = ?2, decided_outcome = ?3, decided_revision = ?4 WHERE mutation_id = ?1 AND decided_outcome IS NULL";

const SQL_UPSERT_ACTIVE: &str = "INSERT INTO credential_active (provider, label, active_version) VALUES (?1, ?2, ?3) ON CONFLICT (provider, label) DO UPDATE SET active_version = excluded.active_version";

const SQL_SELECT_ACTIVE: &str =
    "SELECT active_version FROM credential_active WHERE provider = ?1 AND label = ?2";

/// Enqueues one replaced version. The primary key makes a repeated retirement
/// of the same version a loud constraint failure, not a silent overwrite: a
/// version can only stop being active once.
const SQL_ENQUEUE_RETIRED: &str = "INSERT INTO credential_retired (provider, label, version, mutation_id, retired_at) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_PENDING_RETIRED: &str = "SELECT provider, label, version, mutation_id FROM credential_retired ORDER BY retired_at ASC, rowid ASC LIMIT ?1";

const SQL_DELETE_RETIRED: &str =
    "DELETE FROM credential_retired WHERE provider = ?1 AND label = ?2 AND version = ?3";

/// Completes the retiring mutation only when it committed an outcome and no
/// version it retired is still pending. The `phase` guard keeps a retry from
/// rewriting an already-completed mutation, and the `NOT EXISTS` probe makes
/// the transition depend on the whole retired set, not just this row.
const SQL_COMPLETE_RETIRED_MUTATION: &str = "UPDATE credential_mutation SET phase = ?2 WHERE mutation_id = ?1 AND phase = ?3 AND decided_outcome IS NOT NULL AND NOT EXISTS (SELECT 1 FROM credential_retired WHERE mutation_id = ?1)";

/// A stored outcome is written as its own durable text, so a later phase move
/// never rewrites what the Owner decided.
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

/// Widens a non-committing journal outcome to the full vocabulary written to
/// the durable `decided_outcome` column. The mapping is the only place the
/// uncommitted type meets a commit outcome, so `Activated`/`Revoked` can never
/// enter the non-committing path.
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
            // Design §3 step 4: the commit transaction re-verifies the
            // operation phase, so a mutation that was never staged through
            // the OS `put` is not activated here.
            if mutation.phase != MutationPhase::Staged {
                return Ok(ActivationOutcome::Missing);
            }
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
            // Sweep first: the new value and any value it replaces are removed
            // from stored content in this same transaction, so a premise taken
            // before the commit is covered by it.
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
            let previous: Option<Option<i64>> = tx
                .query_row(
                    SQL_SELECT_ACTIVE,
                    params![mutation.provider, mutation.label],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let previous_active = previous
                .flatten()
                .and_then(|value| u64::try_from(value).ok());
            // The replaced version is enqueued in this same transaction and a
            // later update never overwrites it. A candidate equal to the
            // current active version retires nothing: the item this commit
            // activates must never enter the cleanup set.
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
            // The active reference and revision commit here: the phase is
            // `Activated` when nothing was retired, and `CleanupPending` when
            // the version enqueued above still needs removal.
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
                    next as i64,
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
            let stored: Option<Option<ene_credential::CredentialMutation>> = tx
                .query_row(SQL_SELECT_MUTATION, params![mutation_id], |row| {
                    mutation_from_row(mutation_id.clone(), row)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let Some(Some(mutation)) = stored else {
                return Ok(ActivationOutcome::Missing);
            };
            // A decided mutation answers from its stored outcome, so a retry
            // never invalidates a newer reference.
            if let Some(outcome) = mutation.outcome {
                return Ok(ActivationOutcome::AlreadyDecided(outcome));
            }
            // Only an undecided `Revoke` recorded before any OS write has a
            // durable premise to invalidate; a registration mutation or an
            // unexpected phase is not revoked here.
            if mutation.kind != MutationKind::Revoke || mutation.phase != MutationPhase::Prepared {
                return Ok(ActivationOutcome::Missing);
            }
            let current: i64 = tx
                .query_row(SQL_SELECT_SET_REV, (), |row| row.get(0))
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let current = u64::try_from(current)
                .map_err(|_| credential_unavailable("credential set revision out of range"))?;
            if let Some(expected) = mutation.expected_revision
                && expected != current
            {
                // The premise moved: nothing is invalidated, and the value
                // named for retirement is swept so no plaintext of a
                // registered value survives the refused attempt.
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
            // Sweep the invalidated value in the same transaction that clears
            // the reference: a premise taken before the commit is covered by
            // the sweep or refused by the revision below.
            if let Some(retired) = retired_bearer.as_deref() {
                sweep_registered_secret(&tx, retired)?;
            }
            let previous: Option<Option<i64>> = tx
                .query_row(
                    SQL_SELECT_ACTIVE,
                    params![mutation.provider, mutation.label],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            let previous_active = previous
                .flatten()
                .and_then(|value| u64::try_from(value).ok());
            // The invalidated version is enqueued in this same transaction,
            // alongside any earlier pending retirement, and its item stays
            // addressable until the cleanup records the removal.
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
            // A revoked reference with a version still to remove stays
            // `CleanupPending`; one with nothing to remove is `Completed`.
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
                    next as i64,
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // `Activated` is the committed transaction answer for both
            // `activate_credential` and `revoke_credential`; the committed
            // outcome distinguishes the operation, and `retired` names the
            // version whose item still needs removal.
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
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.execute(
                SQL_DECIDE_MUTATION,
                params![
                    mutation_id,
                    MutationPhase::Abandoned.as_str(),
                    outcome_text(&uncommitted_outcome(outcome)),
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
    ) -> Result<Option<SecretVersionId>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let active: Option<Option<i64>> = guard
                .query_row(SQL_SELECT_ACTIVE, params![provider, label], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(active
                .flatten()
                .and_then(|value| u64::try_from(value).ok())
                .map(SecretVersionId::from_u64))
        })
        .await
    }

    async fn pending_credential_retirements(
        &self,
        limit: u32,
    ) -> Result<Vec<RetiredCredentialVersion>, CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
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
            Ok(retired)
        })
        .await
    }

    async fn mark_credential_cleaned(
        &self,
        mutation_id: &str,
        provider: &str,
        label: &str,
        version: SecretVersionId,
    ) -> Result<(), CredentialTechnicalError> {
        let conn = Arc::clone(&self.conn);
        let mutation_id = mutation_id.to_owned();
        let provider = provider.to_owned();
        let label = label.to_owned();
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| credential_unavailable(error.to_string()))?;
            // The row is the pending state: deleting it is the durable record
            // that the item removal was confirmed. A row already removed by a
            // concurrent pass deletes nothing, so the call is idempotent.
            tx.execute(
                SQL_DELETE_RETIRED,
                params![provider, label, version.as_u64() as i64],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            // Completing the mutation and removing the pending version commit
            // together, and only when the mutation's whole retired set is
            // gone: a cleanup whose transaction fails leaves both the row and
            // the `CleanupPending` phase so the item is retried.
            tx.execute(
                SQL_COMPLETE_RETIRED_MUTATION,
                params![
                    mutation_id,
                    MutationPhase::Completed.as_str(),
                    MutationPhase::CleanupPending.as_str(),
                ],
            )
            .map_err(|error| credential_unavailable(error.to_string()))?;
            tx.commit()
                .map_err(|error| credential_unavailable(error.to_string()))?;
            Ok(())
        })
        .await
    }
}

impl Store {
    /// One bounded retirement-cleanup pass.
    ///
    /// Reads at most `limit` pending retired versions (oldest first), removes
    /// each one's OS item through `values`, and records the confirmed removals
    /// in one short transaction. Recording a removal also completes the
    /// retiring mutation once no version it retired remains pending. The pass
    /// never reports a version swept whose item removal was not confirmed: a
    /// failed removal leaves its row pending for a later pass, and a crash
    /// after the removal but before the state write leaves the row so the
    /// removal is re-attempted (the backends treat a missing item as success).
    /// A row whose credential identity cannot be parsed stays pending as well.
    ///
    /// The batch bound is what keeps one pass from doing unbounded work; the
    /// durable set and the startup/publication cadence drain a backlog across
    /// passes.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// pending set cannot be read or the state transaction cannot commit.
    /// Per-version OS removal failures are not errors: those rows stay
    /// pending and are retried by a later pass.
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
        // OS I/O happens outside the connection lock and the state
        // transaction: a removal cannot hold the store's single connection.
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
