//! Post-turn natural memory decay orchestration (#76).
//!
//! User explicit forget flows through the Memory Arbiter (`MarkUserDeleted`).
//! This module handles time-based `Active → Faded → Archived` transitions only.

use chrono::{DateTime, Utc};
use ene_store::{MemoryStore, NaturalDecayReport};
use tracing::debug;

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
    /// When `true`, `decay_enabled` was off and no store work ran.
    pub skipped: bool,
    /// Memories transitioned to `faded`.
    pub faded_count: usize,
    /// Memories transitioned to `archived`.
    pub archived_count: usize,
}

impl From<NaturalDecayReport> for ForgettingReport {
    fn from(report: NaturalDecayReport) -> Self {
        Self {
            skipped: false,
            faded_count: report.faded_count,
            archived_count: report.archived_count,
        }
    }
}

/// Natural forgetting lifecycle worker.
#[derive(Debug, Default)]
pub struct ForgettingLifecycle;

impl ForgettingLifecycle {
    /// Maximum memories evaluated per lifecycle pass.
    const BATCH_LIMIT: usize = 256;

    /// Apply natural decay transitions for the given scope.
    pub async fn apply(
        store: &MemoryStore,
        ctx: &ForgettingContext<'_>,
        config: &MindMemoryConfig,
    ) -> Result<ForgettingReport, CognitionError> {
        if !config.decay_enabled {
            debug!(
                component = "ForgettingLifecycle",
                character_id = ctx.character_id,
                "Skipping natural decay because decay_enabled is false"
            );
            return Ok(ForgettingReport {
                skipped: true,
                ..ForgettingReport::default()
            });
        }

        let half_life = config.default_forgetting_half_life_days.max(0.0);
        let report = store
            .apply_natural_decay_batch(
                ctx.character_id,
                ctx.user_id,
                ctx.now,
                half_life,
                Self::BATCH_LIMIT,
            )
            .await
            .map_err(CognitionError::Memory)?;

        debug!(
            component = "ForgettingLifecycle",
            character_id = ctx.character_id,
            faded_count = report.faded_count,
            archived_count = report.archived_count,
            half_life_days = half_life,
            "Natural decay pass complete"
        );

        Ok(report.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_skips_when_decay_disabled() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let config = MindMemoryConfig {
            decay_enabled: false,
            ..MindMemoryConfig::default()
        };
        let ctx = ForgettingContext {
            character_id: "ene",
            user_id: Some("user1"),
            now: Utc::now(),
        };

        let report = ForgettingLifecycle::apply(&store, &ctx, &config)
            .await
            .unwrap();
        assert!(report.skipped);
        assert_eq!(report.faded_count, 0);
    }

    #[tokio::test]
    async fn apply_runs_natural_decay_when_enabled() {
        use ene_store::{
            AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
            MemorySource, MemoryStatus, NewMemoryItem,
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

        let config = MindMemoryConfig {
            decay_enabled: true,
            ..MindMemoryConfig::default()
        };
        let ctx = ForgettingContext {
            character_id: "ene",
            user_id: Some("user1"),
            now,
        };

        let report = ForgettingLifecycle::apply(&store, &ctx, &config)
            .await
            .unwrap();
        assert!(!report.skipped);
        assert!(report.faded_count >= 1);
    }
}
