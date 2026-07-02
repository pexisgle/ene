//! Companion Commitment Ledger — promises, tasks, and follow-ups.
//!
//! Commitments are stored in a dedicated `commitments` table in `ene-memory`,
//! linked to typed memories (`MemoryKind::Commitment`) via `source_memory_id`.
//! Active commitments are surfaced independently of vector recall similarity.

use ene_memory::{
    ActiveCommitmentPrompt, Commitment, CommitmentStatus, MemoryKind, MemoryStore, NewCommitment,
};
use tracing::debug;

use crate::error::CognitionError;
use crate::memory_writer::{AppliedDecision, ArbiterAction, ArbiterContext, MemoryArbiter};

/// Companion Commitment Ledger: promises, tasks, follow-ups.
#[derive(Debug, Default)]
pub struct CommitmentLedger;

/// Context required when syncing ledger rows from arbiter results.
#[derive(Debug, Clone)]
pub struct CommitmentSyncContext<'a> {
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: &'a str,
}

impl CommitmentLedger {
    /// Create ledger rows for commitment memories persisted by the arbiter.
    ///
    /// Skips decisions that did not insert a memory, non-commitment candidates,
    /// and rows that already exist for the same `source_memory_id`.
    pub async fn sync_from_applied_decisions(
        store: &MemoryStore,
        ctx: &CommitmentSyncContext<'_>,
        applied: &[AppliedDecision],
    ) -> Result<Vec<i64>, CognitionError> {
        let mut inserted = Vec::new();

        for app in applied {
            match &app.decision.action {
                ArbiterAction::MarkUserDeleted { memory_id } => {
                    Self::cancel_by_source_memory(store, *memory_id).await?;
                    continue;
                }
                ArbiterAction::MarkDisputed { memory_id } => {
                    Self::mark_stale_by_source_memory(store, *memory_id).await?;
                    continue;
                }
                _ => {}
            }

            let candidate = &app.decision.candidate;
            if candidate.kind != MemoryKind::Commitment {
                continue;
            }

            match &app.decision.action {
                ArbiterAction::Persist(_) | ArbiterAction::Supersede { .. } => {
                    let Some(memory_id) = app.inserted_id else {
                        continue;
                    };

                    if let ArbiterAction::Supersede { superseded_id, .. } = &app.decision.action {
                        Self::mark_stale_by_source_memory(store, *superseded_id).await?;
                    }

                    if store
                        .get_commitment_by_source_memory(memory_id)
                        .await
                        .map_err(CognitionError::Memory)?
                        .is_some()
                    {
                        debug!(
                            component = "CommitmentLedger",
                            source_memory_id = memory_id,
                            "commitment already exists for source memory, skipping"
                        );
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
                        source_memory_id: Some(memory_id),
                    };

                    let id = store
                        .insert_commitment(&new_item)
                        .await
                        .map_err(CognitionError::Memory)?;

                    debug!(
                        component = "CommitmentLedger",
                        commitment_id = id,
                        source_memory_id = memory_id,
                        title = %candidate.title,
                        "inserted active commitment"
                    );
                    inserted.push(id);
                }
                ArbiterAction::Ignore | ArbiterAction::AskConfirmationLater => {}
                ArbiterAction::MarkUserDeleted { .. } | ArbiterAction::MarkDisputed { .. } => {}
            }
        }

        Ok(inserted)
    }

    /// Run arbitration, apply decisions, and sync commitment ledger rows.
    pub async fn arbitrate_apply_and_sync(
        store: &MemoryStore,
        candidates: &[crate::memory_writer::candidate::MemoryCandidate],
        arbiter_ctx: &ArbiterContext<'_>,
        sync_ctx: &CommitmentSyncContext<'_>,
    ) -> Result<(Vec<AppliedDecision>, Vec<i64>), CognitionError> {
        let applied = MemoryArbiter::arbitrate_and_apply(store, candidates, arbiter_ctx).await?;
        let commitment_ids = Self::sync_from_applied_decisions(store, sync_ctx, &applied).await?;
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
    #[must_use]
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

    async fn mark_stale_by_source_memory(
        store: &MemoryStore,
        source_memory_id: i64,
    ) -> Result<(), CognitionError> {
        if let Some(old) = store
            .get_commitment_by_source_memory(source_memory_id)
            .await
            .map_err(CognitionError::Memory)?
            && let Some(id) = old.id
        {
            store
                .update_commitment_status(id, CommitmentStatus::Stale)
                .await
                .map_err(CognitionError::Memory)?;
        }
        Ok(())
    }

    async fn cancel_by_source_memory(
        store: &MemoryStore,
        source_memory_id: i64,
    ) -> Result<(), CognitionError> {
        if let Some(old) = store
            .get_commitment_by_source_memory(source_memory_id)
            .await
            .map_err(CognitionError::Memory)?
            && let Some(id) = old.id
        {
            store
                .cancel_commitment(id)
                .await
                .map_err(CognitionError::Memory)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::{MemoryCandidate, TurnInput};
    use crate::memory_writer::{
        ArbiterOptions, ArbiterReason, ArbiterReasonCode, CandidateDecision, CandidateProvenance,
    };
    use ene_memory::{
        MemoryConfidence, MemorySalience, MemoryScope, MemorySource, MemoryStatus, NewMemoryItem,
    };

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

    fn applied_persist(candidate: MemoryCandidate, memory_id: i64) -> AppliedDecision {
        AppliedDecision {
            decision: CandidateDecision {
                candidate,
                action: ArbiterAction::Persist(NewMemoryItem {
                    scope: MemoryScope::Character,
                    character_id: "ene".to_string(),
                    user_id: "user1".to_string(),
                    kind: MemoryKind::Commitment,
                    title: "discuss design".to_string(),
                    content: "Next time, let's discuss the design".to_string(),
                    source: MemorySource::Inferred,
                    source_ref: None,
                    confidence: MemoryConfidence::new(0.9),
                    salience: MemorySalience::new(0.7),
                    affect: ene_memory::AffectAnnotation::default(),
                    relationship_impact: 0.0,
                    valid_from: None,
                    valid_until: None,
                    status: MemoryStatus::Active,
                    supersedes_id: None,
                }),
                reason: ArbiterReason {
                    code: ArbiterReasonCode::ValidNewMemory,
                    detail: "test".to_string(),
                },
            },
            inserted_id: Some(memory_id),
            updated_existing: false,
        }
    }

    fn applied_supersede(
        candidate: MemoryCandidate,
        new_memory_id: i64,
        superseded_id: i64,
    ) -> AppliedDecision {
        AppliedDecision {
            decision: CandidateDecision {
                candidate,
                action: ArbiterAction::Supersede {
                    new_item: NewMemoryItem {
                        scope: MemoryScope::Character,
                        character_id: "ene".to_string(),
                        user_id: "user1".to_string(),
                        kind: MemoryKind::Commitment,
                        title: "discuss design v2".to_string(),
                        content: "Let's revisit the design next time".to_string(),
                        source: MemorySource::Inferred,
                        source_ref: None,
                        confidence: MemoryConfidence::new(0.9),
                        salience: MemorySalience::new(0.7),
                        affect: ene_memory::AffectAnnotation::default(),
                        relationship_impact: 0.0,
                        valid_from: None,
                        valid_until: None,
                        status: MemoryStatus::Active,
                        supersedes_id: Some(superseded_id),
                    },
                    superseded_id,
                },
                reason: ArbiterReason {
                    code: ArbiterReasonCode::ContradictionSupersede,
                    detail: "test".to_string(),
                },
            },
            inserted_id: Some(new_memory_id),
            updated_existing: true,
        }
    }

    fn applied_mark_user_deleted(candidate: MemoryCandidate, memory_id: i64) -> AppliedDecision {
        AppliedDecision {
            decision: CandidateDecision {
                candidate,
                action: ArbiterAction::MarkUserDeleted { memory_id },
                reason: ArbiterReason {
                    code: ArbiterReasonCode::DeletionRequest,
                    detail: "test".to_string(),
                },
            },
            inserted_id: None,
            updated_existing: true,
        }
    }

    fn applied_mark_disputed(candidate: MemoryCandidate, memory_id: i64) -> AppliedDecision {
        AppliedDecision {
            decision: CandidateDecision {
                candidate,
                action: ArbiterAction::MarkDisputed { memory_id },
                reason: ArbiterReason {
                    code: ArbiterReasonCode::ContradictionDisputed,
                    detail: "test".to_string(),
                },
            },
            inserted_id: None,
            updated_existing: true,
        }
    }

    async fn seed_commitment_memory(store: &MemoryStore, title: &str, content: &str) -> i64 {
        store
            .insert_typed_memory(&NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                kind: MemoryKind::Commitment,
                title: title.to_string(),
                content: content.to_string(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.9),
                salience: MemorySalience::new(0.7),
                affect: ene_memory::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn sync_creates_active_commitment_from_persist() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let memory_id = store
            .insert_typed_memory(&NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                kind: MemoryKind::Commitment,
                title: "discuss design".to_string(),
                content: "Next time, let's discuss the design".to_string(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.9),
                salience: MemorySalience::new(0.7),
                affect: ene_memory::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
            })
            .await
            .unwrap();

        let applied = vec![applied_persist(commitment_candidate(0.9), memory_id)];
        let ids = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();
        assert_eq!(ids.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, CommitmentStatus::Active);
        assert_eq!(active[0].source_memory_id, Some(memory_id));
        assert_eq!(active[0].due_label.as_deref(), Some("Next time"));

        let prompts = CommitmentLedger::active_prompt_candidates(&active);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "discuss design");
    }

    #[tokio::test]
    async fn sync_skips_duplicate_source_memory() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let memory_id = store
            .insert_typed_memory(&NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                kind: MemoryKind::Commitment,
                title: "discuss design".to_string(),
                content: "Next time, let's discuss the design".to_string(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.9),
                salience: MemorySalience::new(0.7),
                affect: ene_memory::AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
            })
            .await
            .unwrap();
        let applied = vec![applied_persist(commitment_candidate(0.9), memory_id)];

        let ids1 = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();
        assert_eq!(ids1.len(), 1);

        let ids2 = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();
        assert!(ids2.is_empty());

        let active = CommitmentLedger::list_active(&store, "ene", None, 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn sync_ignores_non_commitment_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut candidate = commitment_candidate(0.9);
        candidate.kind = MemoryKind::Semantic;
        let applied = vec![applied_persist(candidate, 1)];

        let ids = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn arbitrate_apply_and_sync_end_to_end() {
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

        assert_eq!(applied.len(), 1);
        assert!(applied[0].inserted_id.is_some());
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
                source_memory_id: None,
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
                source_memory_id: None,
            })
            .await
            .unwrap();
        assert!(CommitmentLedger::cancel(&store, id2).await.unwrap());
        let cancelled = store.get_commitment(id2).await.unwrap().unwrap();
        assert_eq!(cancelled.status, CommitmentStatus::Cancelled);
    }

    #[tokio::test]
    async fn sync_supersede_marks_old_stale_and_creates_new_active() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let old_memory_id = seed_commitment_memory(
            &store,
            "discuss design",
            "Next time, let's discuss the design",
        )
        .await;
        let new_memory_id = seed_commitment_memory(
            &store,
            "discuss design v2",
            "Let's revisit the design next time",
        )
        .await;

        let initial = vec![applied_persist(commitment_candidate(0.9), old_memory_id)];
        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &initial)
            .await
            .unwrap();

        let supersede = vec![applied_supersede(
            commitment_candidate(0.9),
            new_memory_id,
            old_memory_id,
        )];
        let ids = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &supersede)
            .await
            .unwrap();
        assert_eq!(ids.len(), 1);

        let old = store
            .get_commitment_by_source_memory(old_memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.status, CommitmentStatus::Stale);

        let new = store
            .get_commitment_by_source_memory(new_memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(new.status, CommitmentStatus::Active);
        assert_eq!(new.title, "discuss design");
    }

    #[tokio::test]
    async fn sync_mark_user_deleted_cancels_linked_commitment() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let memory_id = seed_commitment_memory(&store, "follow up", "Talk about project X").await;
        let initial = vec![applied_persist(commitment_candidate(0.9), memory_id)];
        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &initial)
            .await
            .unwrap();

        let mut deletion_candidate = commitment_candidate(0.9);
        deletion_candidate.kind = MemoryKind::Semantic;
        deletion_candidate.should_persist = false;
        deletion_candidate.deletion_target_key = Some("project X".to_string());
        let applied = vec![applied_mark_user_deleted(deletion_candidate, memory_id)];

        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();

        let cancelled = store
            .get_commitment_by_source_memory(memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, CommitmentStatus::Cancelled);
    }

    #[tokio::test]
    async fn sync_mark_disputed_marks_linked_commitment_stale() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let memory_id = seed_commitment_memory(&store, "follow up", "Talk about project X").await;
        let initial = vec![applied_persist(commitment_candidate(0.9), memory_id)];
        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &initial)
            .await
            .unwrap();

        let applied = vec![applied_mark_disputed(commitment_candidate(0.75), memory_id)];
        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &applied)
            .await
            .unwrap();

        let stale = store
            .get_commitment_by_source_memory(memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.status, CommitmentStatus::Stale);
    }

    #[tokio::test]
    async fn sync_supersede_does_not_overwrite_done_commitment() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let old_memory_id = seed_commitment_memory(
            &store,
            "discuss design",
            "Next time, let's discuss the design",
        )
        .await;
        let new_memory_id = seed_commitment_memory(
            &store,
            "discuss design v2",
            "Let's revisit the design next time",
        )
        .await;

        let initial = vec![applied_persist(commitment_candidate(0.9), old_memory_id)];
        let ids = CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &initial)
            .await
            .unwrap();
        assert!(CommitmentLedger::complete(&store, ids[0]).await.unwrap());

        let supersede = vec![applied_supersede(
            commitment_candidate(0.9),
            new_memory_id,
            old_memory_id,
        )];
        CommitmentLedger::sync_from_applied_decisions(&store, &sync_ctx(), &supersede)
            .await
            .unwrap();

        let old = store
            .get_commitment_by_source_memory(old_memory_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.status, CommitmentStatus::Done);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].source_memory_id, Some(new_memory_id));
    }
}
