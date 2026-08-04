//! Post-turn natural memory decay orchestration.
//!
//! User explicit forget flows through the Memory Arbiter (`MarkUserDeleted`).
//! This module handles time-based `Active → Faded → Archived` transitions only.

use chrono::{DateTime, Utc};
use ene_core::{MemoryPort, NaturalDecayReport};
use tracing::{debug, info};

use crate::config::MindMemoryConfig;
use crate::error::CognitionError;

/// Scope for a forgetting lifecycle pass.
#[derive(Debug, Clone)]
pub struct ForgettingContext<'a> {
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: Option<&'a str>,
    /// Reference time for decay scoring.
    pub now: DateTime<Utc>,
}

/// Summary of a forgetting lifecycle run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForgettingReport {
    /// Memories transitioned to `faded`.
    pub faded_count: usize,
    /// Memories transitioned to `archived`.
    pub archived_count: usize,
    /// Pending memory candidates removed by the retention policy.
    pub pruned_candidates: usize,
}

impl From<NaturalDecayReport> for ForgettingReport {
    /// Build a report from a decay run alone.
    ///
    /// `pruned_candidates` is set to `0` here because a bare
    /// [`NaturalDecayReport`] carries no candidate-retention information;
    /// callers that also run the retention sweep (e.g.
    /// [`ForgettingLifecycle::apply`]) construct [`ForgettingReport`] directly
    /// so the count is not overwritten.
    fn from(report: NaturalDecayReport) -> Self {
        Self {
            faded_count: report.faded_count,
            archived_count: report.archived_count,
            pruned_candidates: 0,
        }
    }
}

/// Natural forgetting lifecycle worker.
#[derive(Debug, Default)]
pub struct ForgettingLifecycle;

impl ForgettingLifecycle {
    /// Apply natural decay transitions for the given scope.
    pub async fn apply(
        store: &dyn MemoryPort,
        ctx: &ForgettingContext<'_>,
        config: &MindMemoryConfig,
    ) -> Result<ForgettingReport, CognitionError> {
        let half_life = config.default_forgetting_half_life_days.max(0.0);
        let decay = store
            .apply_natural_decay_batch(
                ctx.character_id,
                ctx.user_id,
                ctx.now,
                half_life,
                config.fade_threshold,
                config.archive_threshold,
            )
            .await
            .map_err(CognitionError::MemoryPort)?;

        // Enforce the pending-candidate retention policy on the same batch
        // path: expire stale candidates and cap the live queue. Scoped
        // to the same (character, user) as the decay pass so one user's
        // candidates cannot evict another's on a multi-user database.
        let retention = &config.pending_candidate_retention;
        let pruned_candidates = store
            .prune_pending_candidates(
                ctx.character_id,
                ctx.user_id,
                retention.max_age_days,
                retention.max_per_character,
                ctx.now,
            )
            .await
            .map_err(CognitionError::MemoryPort)?;

        if pruned_candidates > 0 {
            info!(
                component = "ForgettingLifecycle",
                character_id = ctx.character_id,
                pruned_candidates,
                "Retention policy expired pending memory candidates"
            );
        }

        debug!(
            component = "ForgettingLifecycle",
            character_id = ctx.character_id,
            faded_count = decay.faded_count,
            archived_count = decay.archived_count,
            pruned_candidates,
            half_life_days = half_life,
            "Natural decay pass complete"
        );

        Ok(ForgettingReport {
            faded_count: decay.faded_count,
            archived_count: decay.archived_count,
            pruned_candidates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_runs_natural_decay() {
        use ene_store::{
            AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
            MemorySource, MemoryStatus, MemoryStore, NewMemoryItem,
        };

        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();
        let id = store
            .insert_typed_memory(&NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: MemoryKind::Semantic,
                title: "stale".into(),
                content: "old lifecycle content".into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::new(0.1),
                salience: MemorySalience::new(0.1),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .await
            .unwrap();
        store.test_backdate_typed_memory(id, 120).await.unwrap();

        let config = MindMemoryConfig::default();
        let ctx = ForgettingContext {
            character_id: "ene",
            user_id: Some("user1"),
            now,
        };

        let report = ForgettingLifecycle::apply(&store, &ctx, &config)
            .await
            .unwrap();
        assert!(report.faded_count >= 1);
    }

    /// The lifecycle reports how many pending candidates the retention sweep
    /// removed (surfaced on the mind side, not just the store).
    #[tokio::test]
    async fn apply_reports_pruned_candidates() {
        use ene_core::PendingCandidate;
        use ene_store::{MemoryKind, MemoryStore, PendingCandidateStatus};

        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();

        // A stale pending candidate (older than the default 14-day max age).
        let id = store
            .insert_pending_candidate(PendingCandidate {
                id: 0,
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: "stale candidate".into(),
                content: "old".into(),
                kind: MemoryKind::Preference,
                confidence: 0.8,
                reason_detail: String::new(),
                existing_memory_title: None,
                existing_memory_id: None,
                source_quote: String::new(),
                source_turn: None,
                approval_parked: false,
                status: PendingCandidateStatus::Pending,
                created_at: now,
                resolved_at: None,
            })
            .await
            .unwrap();
        store.test_backdate_pending_candidate(id, 30).await.unwrap();

        let config = MindMemoryConfig::default();
        let ctx = ForgettingContext {
            character_id: "ene",
            user_id: Some("user1"),
            now,
        };

        let report = ForgettingLifecycle::apply(&store, &ctx, &config)
            .await
            .unwrap();
        assert_eq!(report.pruned_candidates, 1);
    }
}
