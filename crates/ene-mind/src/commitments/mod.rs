//! Companion Commitment Ledger — promises, tasks, and follow-ups.
//!
//! The `commitments` table is the sole source of truth for commitment lifecycle
//! and prompt injection (#124). Typed `MemoryKind::Commitment` rows are optional
//! references (`typed_memories.commitment_id`) and are no longer dual-written
//! from arbiter persist → sync.

use ene_store::{
    ActiveCommitmentPrompt, Commitment, CommitmentStatus, MemoryKind, MemoryStore, NewCommitment,
};
use tracing::{debug, warn};

use crate::error::CognitionError;
use crate::memory_writer::candidate::MemoryCandidate;
use crate::memory_writer::{AppliedDecision, ArbiterContext, MemoryArbiter};

/// Maximum active commitments to consider for title-matching dedup / deletion.
/// Set to 4096 — well above any plausible concurrent active‑commitment count.
/// A warning is emitted when the cap is hit so operators can tune if needed.
const MAX_ACTIVE_MATCH_CHECK: usize = 4096;

/// Companion Commitment Ledger: promises, tasks, follow-ups.
#[derive(Debug, Default)]
pub struct CommitmentLedger;

/// Context required when writing ledger rows from commitment candidates.
#[derive(Debug, Clone)]
pub struct CommitmentSyncContext<'a> {
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: &'a str,
}

impl CommitmentLedger {
    /// Persist commitment candidates to the ledger first (sole `SoT`).
    ///
    /// Does **not** insert typed `MemoryKind::Commitment` bodies. Deletion-style
    /// candidates (`should_persist = false`) cancel matching active ledger rows
    /// by normalized title.
    pub async fn apply_commitment_candidates(
        store: &MemoryStore,
        ctx: &CommitmentSyncContext<'_>,
        candidates: &[MemoryCandidate],
    ) -> Result<Vec<i64>, CognitionError> {
        let mut inserted = Vec::new();

        for candidate in candidates {
            if candidate.kind != MemoryKind::Commitment {
                continue;
            }

            if !candidate.should_persist {
                Self::cancel_matching_by_title(store, ctx, &candidate.title).await?;
                continue;
            }

            let title_key = normalize_title(&candidate.title);
            let active = store
                .list_active_commitments(
                    ctx.character_id,
                    Some(ctx.user_id),
                    MAX_ACTIVE_MATCH_CHECK,
                )
                .await
                .map_err(CognitionError::Memory)?;
            if let Some(existing) = active
                .iter()
                .find(|c| normalize_title(&c.title) == title_key)
            {
                // Same title exists — supersede (update description/due_label) if content differs.
                let content_changed = existing.description != candidate.content;
                let due_changed =
                    existing.due_label.as_deref() != candidate.commitment_due.as_deref();
                if content_changed || due_changed {
                    if let Some(id) = existing.id {
                        store
                            .supersede_commitment(
                                id,
                                &candidate.content,
                                candidate.commitment_due.as_deref(),
                            )
                            .await
                            .map_err(CognitionError::Memory)?;
                        debug!(
                            component = "CommitmentLedger",
                            commitment_id = id,
                            title = %candidate.title,
                            "superseded active commitment (description/due_label updated)"
                        );
                        inserted.push(id);
                    }
                } else {
                    debug!(
                        component = "CommitmentLedger",
                        title = %candidate.title,
                        "active commitment unchanged, skipping"
                    );
                }
                continue;
            }

            let new_item = NewCommitment {
                character_id: ctx.character_id.to_string(),
                user_id: ctx.user_id.to_string(),
                title: candidate.title.clone(),
                description: candidate.content.clone(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: candidate.commitment_due.clone(),
            };

            let id = store
                .insert_commitment(&new_item)
                .await
                .map_err(CognitionError::Memory)?;

            debug!(
                component = "CommitmentLedger",
                commitment_id = id,
                title = %candidate.title,
                "inserted active commitment (ledger-first)"
            );
            inserted.push(id);
        }

        Ok(inserted)
    }

    /// Arbitrate non-commitment candidates; write commitments ledger-first.
    ///
    /// Replaces the former dual-write path (`arbitrate` → typed persist →
    /// `sync_from_applied_decisions`).
    pub async fn arbitrate_apply_and_sync(
        store: &MemoryStore,
        candidates: &[MemoryCandidate],
        arbiter_ctx: &ArbiterContext<'_>,
        sync_ctx: &CommitmentSyncContext<'_>,
    ) -> Result<(Vec<AppliedDecision>, Vec<i64>), CognitionError> {
        let (commitment_candidates, other_candidates): (Vec<_>, Vec<_>) = candidates
            .iter()
            .cloned()
            .partition(|c| c.kind == MemoryKind::Commitment);

        let commitment_ids =
            Self::apply_commitment_candidates(store, sync_ctx, &commitment_candidates).await?;

        let applied: Vec<AppliedDecision> = if other_candidates.is_empty() {
            Vec::new()
        } else {
            MemoryArbiter::arbitrate_and_apply(store, &other_candidates, arbiter_ctx).await?
        };

        Ok((applied, commitment_ids))
    }

    /// List active commitments for prompt injection (independent of vector recall).
    pub async fn list_active(
        store: &MemoryStore,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, CognitionError> {
        store
            .list_active_commitments(character_id, user_id, limit)
            .await
            .map_err(CognitionError::Memory)
    }

    /// Map active commitments to lightweight prompt DTOs.
    pub fn active_prompt_candidates(commitments: &[Commitment]) -> Vec<ActiveCommitmentPrompt> {
        commitments
            .iter()
            .filter_map(|c| {
                let id = c.id?;
                Some(ActiveCommitmentPrompt {
                    id,
                    title: c.title.clone(),
                    description: c.description.clone(),
                    due_label: c.due_label.clone(),
                    due_at: c.due_at,
                })
            })
            .collect()
    }

    /// Mark a commitment as done.
    pub async fn complete(store: &MemoryStore, id: i64) -> Result<bool, CognitionError> {
        store
            .complete_commitment(id)
            .await
            .map_err(CognitionError::Memory)
    }

    /// Mark a commitment as cancelled.
    pub async fn cancel(store: &MemoryStore, id: i64) -> Result<bool, CognitionError> {
        store
            .cancel_commitment(id)
            .await
            .map_err(CognitionError::Memory)
    }

    /// Mark overdue active commitments as stale.
    pub async fn mark_stale_overdue(
        store: &MemoryStore,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<usize, CognitionError> {
        store
            .mark_stale_commitments(now)
            .await
            .map_err(CognitionError::Memory)
    }

    async fn cancel_matching_by_title(
        store: &MemoryStore,
        ctx: &CommitmentSyncContext<'_>,
        title: &str,
    ) -> Result<(), CognitionError> {
        let key = normalize_title(title);
        let active = store
            .list_active_commitments(ctx.character_id, Some(ctx.user_id), MAX_ACTIVE_MATCH_CHECK)
            .await
            .map_err(CognitionError::Memory)?;

        if active.len() == MAX_ACTIVE_MATCH_CHECK {
            warn!(
                component = "CommitmentLedger",
                limit = MAX_ACTIVE_MATCH_CHECK,
                "list_active_commitments returned exactly the limit; results may be truncated"
            );
        }

        let matching: Vec<(i64, &str)> = active
            .iter()
            .filter_map(|row| row.id.map(|id| (id, row.title.as_str())))
            .filter(|(_, t)| normalize_title(t) == key)
            .collect();

        if matching.len() > 1 {
            warn!(
                component = "CommitmentLedger",
                title = %key,
                count = matching.len(),
                ids = ?matching.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                "cancel_matching_by_title matched multiple active commitments; ambiguity may cause unintended cancellation"
            );
        }

        // Accumulate errors across all rows so a single failure
        // does not leave the caller in a partially-cancelled state.
        let mut errors: Vec<String> = Vec::new();
        for (id, _) in &matching {
            if let Err(e) = store.cancel_commitment(*id).await {
                errors.push(format!("commitment {id}: {e}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(CognitionError::Aggregate(errors.join("; ")))
        }
    }
}

fn normalize_title(title: &str) -> String {
    title.trim().to_lowercase()
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    deprecated,
    reason = "explicit Default for test fixture clarity; deprecated API retained for migration"
)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::{MemoryCandidate, TurnInput};
    use crate::memory_writer::{ArbiterContext, ArbiterOptions, CandidateProvenance};

    fn sync_ctx<'a>() -> CommitmentSyncContext<'a> {
        CommitmentSyncContext {
            character_id: "ene",
            user_id: "user1",
        }
    }

    fn commitment_candidate(confidence: f32) -> MemoryCandidate {
        MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: "discuss design".to_string(),
            content: "Next time, let's discuss the design".to_string(),
            source_quote: "Next time, let's discuss the design".to_string(),
            confidence,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: Some("Next time".to_string()),
        }
    }

    #[tokio::test]
    async fn apply_creates_active_commitment_ledger_first() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let ids = CommitmentLedger::apply_commitment_candidates(
            &store,
            &sync_ctx(),
            &[commitment_candidate(0.9)],
        )
        .await
        .unwrap();
        assert_eq!(ids.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, CommitmentStatus::Active);
        assert_eq!(active[0].due_label.as_deref(), Some("Next time"));

        let prompts = CommitmentLedger::active_prompt_candidates(&active);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "discuss design");
    }

    #[tokio::test]
    async fn apply_skips_duplicate_title() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let candidates = [commitment_candidate(0.9)];
        let ids1 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert_eq!(ids1.len(), 1);

        let ids2 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert!(ids2.is_empty());

        let active = CommitmentLedger::list_active(&store, "ene", None, 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn apply_supersedes_existing_with_changed_content() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        // Insert the initial commitment
        let candidates = [commitment_candidate(0.9)];
        let ids1 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert_eq!(ids1.len(), 1);

        // Same title, different content and due_label → supersede
        let mut updated = commitment_candidate(0.9);
        updated.content = "Let's discuss the UI design instead".to_string();
        updated.commitment_due = Some("Tomorrow".to_string());

        let ids2 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[updated])
            .await
            .unwrap();
        assert_eq!(ids2.len(), 1);
        assert_eq!(ids2[0], ids1[0], "superseded row should keep the same id");

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].description, "Let's discuss the UI design instead");
        assert_eq!(active[0].due_label.as_deref(), Some("Tomorrow"));
    }

    #[tokio::test]
    async fn apply_ignores_non_commitment_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut candidate = commitment_candidate(0.9);
        candidate.kind = MemoryKind::Semantic;
        let ids = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[candidate])
            .await
            .unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn apply_deletion_cancels_matching_title() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        CommitmentLedger::apply_commitment_candidates(
            &store,
            &sync_ctx(),
            &[commitment_candidate(0.9)],
        )
        .await
        .unwrap();

        let mut deletion = commitment_candidate(0.9);
        deletion.should_persist = false;
        CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[deletion])
            .await
            .unwrap();

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn arbitrate_apply_and_sync_end_to_end_ledger_first() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let turn = TurnInput {
            user_message: "Next time, let's discuss the design",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ArbiterContext {
            turn,
            character_id: "ene",
            user_id: "user1",
            source_ref: Some("session-1"),
            provenance: CandidateProvenance::Deterministic,
            options: ArbiterOptions {
                min_confidence: 0.4,
                ..ArbiterOptions::default()
            },
            semantic_matches: Default::default(),
        };
        let candidate = commitment_candidate(0.9);

        let (applied, commitment_ids) = CommitmentLedger::arbitrate_apply_and_sync(
            &store,
            std::slice::from_ref(&candidate),
            &arbiter_ctx,
            &sync_ctx(),
        )
        .await
        .unwrap();

        // Commitment candidates bypass typed arbiter persist.
        assert!(applied.is_empty());
        assert_eq!(commitment_ids.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "discuss design");
    }

    #[tokio::test]
    async fn complete_and_cancel_lifecycle() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let id = store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                title: "follow up".to_string(),
                description: "Talk about project X".to_string(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();

        assert!(CommitmentLedger::complete(&store, id).await.unwrap());
        let done = store.get_commitment(id).await.unwrap().unwrap();
        assert_eq!(done.status, CommitmentStatus::Done);
        assert!(done.completed_at.is_some());

        let id2 = store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: String::new(),
                title: "other".to_string(),
                description: "other task".to_string(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();
        assert!(CommitmentLedger::cancel(&store, id2).await.unwrap());
        let cancelled = store.get_commitment(id2).await.unwrap().unwrap();
        assert_eq!(cancelled.status, CommitmentStatus::Cancelled);
    }
}
