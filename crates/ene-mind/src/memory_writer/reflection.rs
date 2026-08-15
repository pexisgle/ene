//! Self-reflection pipeline.
//!
//! Periodically reviews the outcomes of persisted memories (rated by affect
//! valence) and generates `Reflection`-kind memories summarizing successful
//! and unsuccessful interaction strategies. These reflections are loaded
//! during recall and applied as a scoring signal — boosting or penalizing
//! similar memories — rather than surfacing as ordinary recall results:
//! the recall path excludes [`MemoryKind::Reflection`] from its search query
//! and calls [`load_reflection_memories`] + [`apply_reflection_adjustment`]
//! to close the feedback loop.
use crate::config::ReflectionConfig;
use crate::memory_writer::arbiter::AppliedDecision;
use ene_core::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemoryOutcome, MemoryPort, MemorySalience,
    MemoryScope, MemorySource, MemoryStatus, NewMemoryItem, OutcomeRatingSource, PendingCandidate,
};
use parking_lot::Mutex;

const OUTCOME_WINDOW_LIMIT: usize = 500;

#[derive(Debug)]
pub struct SelfReflectionPipeline {
    config: ReflectionConfig,
    state: Mutex<PipelineState>,
}

#[derive(Debug, Clone)]
struct PipelineState {
    turn_counter: usize,
    outcome_count: usize,
}

impl SelfReflectionPipeline {
    pub fn new(config: ReflectionConfig) -> Self {
        Self {
            config,
            state: Mutex::new(PipelineState {
                turn_counter: 0,
                outcome_count: 0,
            }),
        }
    }

    /// Only decisions with an `outcome_rating` and a valid `inserted_id` are
    /// recorded (neutral 0.0 ratings are stored but contribute to neither
    /// strategy bucket). The record is persisted through the store so the
    /// evaluation survives restarts; persistence failure is non-fatal (the
    /// memory itself is already committed) and only degrades the next
    /// reflection pass.
    pub async fn record_outcome(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
        user_id: &str,
        source_ref: Option<&str>,
        decision: &AppliedDecision,
    ) {
        let Some(rating) = decision.outcome_rating else {
            return;
        };
        let Some(memory_id) = decision.inserted_id else {
            return;
        };
        let outcome = MemoryOutcome {
            id: None,
            memory_id,
            memory_title: decision.decision.candidate.title.clone(),
            character_id: character_id.to_string(),
            user_id: user_id.to_string(),
            rating,
            source: OutcomeRatingSource::Affect,
            source_ref: source_ref.map(str::to_string),
            created_at: chrono::Utc::now(),
        };
        if persist_outcome_row(store, &outcome).await {
            let mut s = self.state.lock();
            s.turn_counter = s.turn_counter.saturating_add(1);
            s.outcome_count = s.outcome_count.saturating_add(1);
        }
    }

    pub fn should_reflect(&self) -> bool {
        let s = self.state.lock();
        s.turn_counter >= self.config.interval_turns && s.outcome_count >= self.config.min_outcomes
    }

    /// Check the local gate and then the durable queue so progress survives a
    /// new pipeline instance or a process restart. Each persisted outcome is
    /// one rated decision, which is the durable progress unit available at
    /// this boundary.
    pub async fn should_reflect_with_store(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
    ) -> bool {
        if self.should_reflect() {
            return true;
        }
        let threshold = self.config.interval_turns.max(self.config.min_outcomes);
        if threshold == 0 {
            return true;
        }
        match store
            .list_memory_outcomes(character_id, None, threshold)
            .await
        {
            Ok(rows) => rows.len() >= threshold,
            Err(error) => {
                tracing::warn!(
                    component = "SelfReflection",
                    error = %error,
                    character_id,
                    "Failed to load durable outcomes for reflection gate"
                );
                false
            }
        }
    }

    fn drain(&self) {
        let mut s = self.state.lock();
        s.turn_counter = 0;
        s.outcome_count = 0;
    }

    /// Returns the list of newly created [`NewMemoryItem`]s (already inserted
    /// into the store). The window is every unconsumed outcome for the
    /// character, so rows recorded by calls that did not meet the gate — and
    /// rows recorded before a restart — are aggregated by the next pass.
    /// Consumed rows are deleted, which keeps the table bounded and prevents
    /// double aggregation. Post-turn writes for one character are serialized
    /// by the caller, so the unconsumed pool is not split across sessions.
    pub async fn generate_reflection(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
        source_ref: Option<&str>,
        user_id: &str,
        _success_boost: f32,
        _failure_penalty: f32,
    ) -> Vec<NewMemoryItem> {
        let outcomes = store
            .list_memory_outcomes(character_id, None, OUTCOME_WINDOW_LIMIT)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(
                    component = "SelfReflection",
                    error = %error,
                    character_id,
                    "Failed to load memory outcomes for reflection generation"
                );
                Vec::new()
            });
        self.drain();
        if outcomes.is_empty() {
            return Vec::new();
        }
        if outcomes.len() == OUTCOME_WINDOW_LIMIT {
            tracing::warn!(
                component = "SelfReflection",
                character_id,
                limit = OUTCOME_WINDOW_LIMIT,
                "Outcome window limit reached; older evaluations wait for the next pass"
            );
        }
        let items = Self::build_reflections(&outcomes, character_id, source_ref, user_id);
        let mut all_inserted = true;
        for item in &items {
            if let Err(e) = store.insert_typed_memory(item).await {
                all_inserted = false;
                tracing::warn!(
                    component = "SelfReflection",
                    error = %e,
                    title = %item.title,
                    "Failed to persist reflection memory"
                );
            }
        }
        if all_inserted {
            let ids: Vec<i64> = outcomes.iter().filter_map(|o| o.id).collect();
            if let Err(error) = store.delete_memory_outcomes(character_id, &ids).await {
                tracing::warn!(
                    component = "SelfReflection",
                    error = %error,
                    character_id,
                    "Failed to consume outcome window; evaluations stay queued for the next pass"
                );
                return Vec::new();
            }
        }
        items
    }

    /// Outcomes with `rating > 0.3` contribute to "Successful strategies";
    /// outcomes with `rating < -0.3` contribute to "Strategies to avoid".
    pub fn build_reflections(
        outcomes: &[MemoryOutcome],
        character_id: &str,
        source_ref: Option<&str>,
        user_id: &str,
    ) -> Vec<NewMemoryItem> {
        let pos: Vec<_> = outcomes.iter().filter(|o| o.rating > 0.3).collect();
        let neg: Vec<_> = outcomes.iter().filter(|o| o.rating < -0.3).collect();
        let mut r = Vec::new();
        if !pos.is_empty() {
            let titles: Vec<&str> = pos.iter().map(|o| o.memory_title.as_str()).collect();
            r.push(NewMemoryItem {
                scope: MemoryScope::Shared,
                character_id: character_id.to_string(),
                user_id: user_id.to_string(),
                kind: MemoryKind::Reflection,
                title: "Successful strategies".into(),
                content: format!("Successful interaction strategies: {}", titles.join(", ")),
                source: MemorySource::Inferred,
                source_ref: source_ref.map(str::to_string),
                confidence: MemoryConfidence::new(0.7),
                salience: MemorySalience::new(0.6),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            });
        }
        if !neg.is_empty() {
            let titles: Vec<&str> = neg.iter().map(|o| o.memory_title.as_str()).collect();
            r.push(NewMemoryItem {
                scope: MemoryScope::Shared,
                character_id: character_id.to_string(),
                user_id: user_id.to_string(),
                kind: MemoryKind::Reflection,
                title: "Strategies to avoid".into(),
                content: format!(
                    "Less effective interaction strategies: {}",
                    titles.join(", ")
                ),
                source: MemorySource::Inferred,
                source_ref: source_ref.map(str::to_string),
                confidence: MemoryConfidence::new(0.7),
                salience: MemorySalience::new(0.4),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            });
        }
        r
    }
}

/// Persist one outcome row, warning instead of failing when the store write
/// errors (the evaluated memory is already committed; a missing row only
/// degrades the next reflection pass).
async fn persist_outcome_row(store: &dyn MemoryPort, outcome: &MemoryOutcome) -> bool {
    match store.record_memory_outcome(outcome).await {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(
                component = "SelfReflection",
                error = %error,
                memory_id = outcome.memory_id,
                "Failed to persist memory outcome; reflection pass will not see it"
            );
            false
        }
    }
}

/// Approval persists the candidate outside the arbiter (the store's
/// `approve_pending_candidate` path), so the rating carried on the pending
/// row is written back here; the row joins the character's unconsumed pool
/// and is aggregated by the next reflection pass. No-op when the candidate
/// carries no rating.
pub async fn record_approved_outcome(
    store: &dyn MemoryPort,
    candidate: &PendingCandidate,
    memory_id: i64,
) {
    let Some(rating) = candidate.outcome_rating else {
        return;
    };
    persist_outcome_row(
        store,
        &MemoryOutcome {
            id: None,
            memory_id,
            memory_title: candidate.title.clone(),
            character_id: candidate.character_id.clone(),
            user_id: candidate.user_id.clone(),
            rating,
            source: OutcomeRatingSource::Affect,
            source_ref: candidate.source_turn.clone(),
            created_at: chrono::Utc::now(),
        },
    )
    .await;
}

/// Only [`ene_core::MemoryStatus::Active`] reflections participate in the
/// recall adjustment: superseded rows are duplicates, and faded/archived
/// reflections are no longer the current strategy signal. Filtering here keeps
/// the boost/penalty applied by `apply_reflection_adjustment` consistent with
/// what the pipeline most recently persisted.
pub async fn load_reflection_memories(
    store: &dyn MemoryPort,
    character_id: &str,
) -> Vec<ene_core::MemoryItem> {
    match store
        .get_typed_memories_by_character(
            character_id,
            Some(MemoryKind::Reflection),
            None,
            None,
            50,
            0,
        )
        .await
    {
        Ok(items) => items
            .into_iter()
            .filter(|item| item.status == ene_core::MemoryStatus::Active)
            .collect(),
        Err(e) => {
            tracing::warn!(
                component = "SelfReflection",
                error = %e,
                "Failed to load reflection memories"
            );
            Vec::new()
        }
    }
}

/// Memories whose titles match "Successful strategies" content are boosted by
/// `success_boost`; those matching "Strategies to avoid" are penalized by
/// `failure_penalty`. The applied factor is recorded in
/// [`MemoryScoreBreakdown::reflection_multiplier`](ene_core::MemoryScoreBreakdown)
/// and `total` is scaled by it, so the explainable breakdown stays consistent
/// (the multiplier documents exactly how `total` was derived) rather than
/// silently overwriting `total`.
pub fn apply_reflection_adjustment(
    memories: &mut [ene_core::ScoredMemory],
    reflections: &[ene_core::MemoryItem],
    success_boost: f32,
    failure_penalty: f32,
) {
    let (succ, fail) = parse_strategies(reflections);
    if succ.is_empty() && fail.is_empty() {
        return;
    }
    for m in memories.iter_mut() {
        let t = m.item.title.to_lowercase();
        let multiplier = if succ.iter().any(|s| t.contains(s.as_str())) {
            success_boost
        } else if fail.iter().any(|s| t.contains(s.as_str())) {
            failure_penalty
        } else {
            continue;
        };
        m.breakdown.reflection_multiplier = multiplier;
        m.breakdown.total *= multiplier;
    }
}

fn parse_strategies(reflections: &[ene_core::MemoryItem]) -> (Vec<String>, Vec<String>) {
    let mut s = Vec::new();
    let mut f = Vec::new();
    for r in reflections {
        let c = r.content.to_lowercase();
        if r.title == "Successful strategies" {
            if let Some(st) = c.strip_prefix("successful interaction strategies: ") {
                s.extend(
                    st.split(", ")
                        .map(|t| t.trim().to_lowercase())
                        .filter(|t| !t.is_empty()),
                );
            }
        } else if r.title == "Strategies to avoid"
            && let Some(st) = c.strip_prefix("less effective interaction strategies: ")
        {
            f.extend(
                st.split(", ")
                    .map(|t| t.trim().to_lowercase())
                    .filter(|t| !t.is_empty()),
            );
        }
    }
    (s, f)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index into fixed-size fixture vectors"
    )]
    use super::*;
    use crate::config::ReflectionConfig;
    use crate::memory_writer::arbiter::{
        AppliedDecision, ArbiterAction, ArbiterReason, ArbiterReasonCode, CandidateDecision,
    };
    use crate::memory_writer::candidate::MemoryCandidate;
    use crate::memory_writer::test_support::InMemoryMemoryPort;
    use ene_core::PendingCandidateStatus;
    use ene_store::MemoryKind;

    fn config(interval: usize, min_outcomes: usize) -> ReflectionConfig {
        ReflectionConfig {
            enabled: true,
            interval_turns: interval,
            min_outcomes,
            success_boost: 1.2,
            failure_penalty: 0.8,
        }
    }

    fn applied_with_rating(title: &str, rating: f32) -> AppliedDecision {
        AppliedDecision {
            decision: CandidateDecision {
                candidate: MemoryCandidate {
                    kind: MemoryKind::Semantic,
                    title: title.to_string(),
                    content: String::new(),
                    source_quote: String::new(),
                    confidence: 0.9,
                    should_persist: true,
                    deletion_target_key: None,
                    commitment_due: None,
                    tags: Vec::new(),
                },
                action: ArbiterAction::Ignore,
                reason: ArbiterReason {
                    code: ArbiterReasonCode::ValidNewMemory,
                    detail: "test".to_string(),
                },
            },
            inserted_id: Some(1),
            updated_existing: false,
            outcome_rating: Some(rating),
        }
    }

    fn outcome(memory_id: i64, title: &str, rating: f32) -> MemoryOutcome {
        MemoryOutcome {
            id: Some(memory_id),
            memory_id,
            memory_title: title.to_string(),
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            rating,
            source: OutcomeRatingSource::Affect,
            source_ref: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn build_reflections_separates_positive_and_negative() {
        let outcomes = vec![
            outcome(1, "good strategy", 0.8),
            outcome(2, "bad strategy", -0.6),
            outcome(3, "neutral", 0.0),
        ];
        let items =
            SelfReflectionPipeline::build_reflections(&outcomes, "ene", Some("sess"), "user1");
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|i| i.title == "Successful strategies"));
        assert!(items.iter().any(|i| i.title == "Strategies to avoid"));
        assert!(items.iter().all(|i| i.kind == MemoryKind::Reflection));
    }

    #[tokio::test]
    async fn record_outcome_gates_on_interval_and_min_outcomes() {
        let port = InMemoryMemoryPort::new();
        let pipeline = SelfReflectionPipeline::new(config(2, 2));
        assert!(!pipeline.should_reflect());

        pipeline
            .record_outcome(&port, "ene", "user1", None, &applied_with_rating("a", 0.5))
            .await;
        assert!(!pipeline.should_reflect(), "only one turn recorded");

        pipeline
            .record_outcome(&port, "ene", "user1", None, &applied_with_rating("b", 0.5))
            .await;
        assert!(
            pipeline.should_reflect(),
            "two turns and two outcomes meet the gate"
        );
    }

    #[tokio::test]
    async fn durable_gate_spans_pipeline_instances() {
        let port = InMemoryMemoryPort::new();
        let first = SelfReflectionPipeline::new(config(2, 2));
        first
            .record_outcome(&port, "ene", "user1", None, &applied_with_rating("a", 0.5))
            .await;
        assert!(!first.should_reflect_with_store(&port, "ene").await);

        let second = SelfReflectionPipeline::new(config(2, 2));
        assert!(!second.should_reflect_with_store(&port, "ene").await);
        second
            .record_outcome(&port, "ene", "user1", None, &applied_with_rating("b", 0.5))
            .await;
        assert!(second.should_reflect_with_store(&port, "ene").await);
    }

    #[tokio::test]
    async fn record_outcome_ignores_missing_rating_or_id() {
        let port = InMemoryMemoryPort::new();
        let pipeline = SelfReflectionPipeline::new(config(1, 1));

        let mut no_rating = applied_with_rating("a", 0.5);
        no_rating.outcome_rating = None;
        pipeline
            .record_outcome(&port, "ene", "user1", None, &no_rating)
            .await;

        let mut no_id = applied_with_rating("b", 0.5);
        no_id.inserted_id = None;
        pipeline
            .record_outcome(&port, "ene", "user1", None, &no_id)
            .await;

        assert!(
            !pipeline.should_reflect(),
            "outcomes without rating or inserted id are not recorded"
        );
        assert!(
            port.outcomes().is_empty(),
            "no outcome row may be persisted for decisions without rating or id"
        );
    }

    #[tokio::test]
    async fn record_outcome_persists_durable_row() {
        let port = InMemoryMemoryPort::new();
        let pipeline = SelfReflectionPipeline::new(config(1, 1));

        pipeline
            .record_outcome(
                &port,
                "ene",
                "user1",
                Some("turn-7"),
                &applied_with_rating("warm greeting", 0.8),
            )
            .await;

        let rows = port.outcomes();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_title, "warm greeting");
        assert!((rows[0].rating - 0.8).abs() < f32::EPSILON);
        assert_eq!(rows[0].source, OutcomeRatingSource::Affect);
        assert_eq!(rows[0].source_ref.as_deref(), Some("turn-7"));
        assert_eq!(rows[0].character_id, "ene");
        assert_eq!(rows[0].user_id, "user1");
    }

    #[tokio::test]
    async fn generate_reflection_aggregates_across_calls_and_restarts() {
        let port = InMemoryMemoryPort::new();

        let first_call = SelfReflectionPipeline::new(config(1, 2));
        first_call
            .record_outcome(
                &port,
                "ene",
                "user1",
                Some("turn-1"),
                &applied_with_rating("good", 0.8),
            )
            .await;
        assert!(!first_call.should_reflect());

        let second_call = SelfReflectionPipeline::new(config(1, 2));
        second_call
            .record_outcome(
                &port,
                "ene",
                "user1",
                Some("turn-2"),
                &applied_with_rating("bad", -0.6),
            )
            .await;
        second_call
            .record_outcome(
                &port,
                "ene",
                "user1",
                Some("turn-2"),
                &applied_with_rating("worse", -0.8),
            )
            .await;
        assert!(second_call.should_reflect());

        let items = second_call
            .generate_reflection(&port, "ene", Some("turn-2"), "user1", 1.2, 0.8)
            .await;
        assert_eq!(items.len(), 2, "both calls' outcomes must be aggregated");
        assert!(
            port.outcomes().is_empty(),
            "consumed outcomes must be pruned"
        );
        let stored = port.all_items();
        assert_eq!(
            stored
                .iter()
                .filter(|i| i.kind == MemoryKind::Reflection)
                .count(),
            2,
            "both strategy reflections must be persisted"
        );
    }

    #[tokio::test]
    async fn record_approved_outcome_persists_rating_row() {
        let port = InMemoryMemoryPort::new();
        let candidate = PendingCandidate {
            id: 7,
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "warm greeting".into(),
            content: String::new(),
            kind: MemoryKind::Semantic,
            confidence: 0.8,
            reason_detail: String::new(),
            existing_memory_title: None,
            existing_memory_id: None,
            outcome_rating: Some(0.7),
            source_quote: String::new(),
            source_turn: Some("turn-9".into()),
            approval_parked: true,
            status: PendingCandidateStatus::Pending,
            created_at: chrono::Utc::now(),
            resolved_at: None,
        };

        record_approved_outcome(&port, &candidate, 42).await;

        let rows = port.outcomes();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_id, 42);
        assert_eq!(rows[0].memory_title, "warm greeting");
        assert!((rows[0].rating - 0.7).abs() < f32::EPSILON);
        assert_eq!(rows[0].character_id, "ene");
        assert_eq!(rows[0].user_id, "user1");
        assert_eq!(rows[0].source_ref.as_deref(), Some("turn-9"));
    }

    #[tokio::test]
    async fn record_approved_outcome_skips_unrated_candidates() {
        let port = InMemoryMemoryPort::new();
        let candidate = PendingCandidate {
            id: 8,
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "unrated".into(),
            content: String::new(),
            kind: MemoryKind::Semantic,
            confidence: 0.8,
            reason_detail: String::new(),
            existing_memory_title: None,
            existing_memory_id: None,
            outcome_rating: None,
            source_quote: String::new(),
            source_turn: None,
            approval_parked: true,
            status: PendingCandidateStatus::Pending,
            created_at: chrono::Utc::now(),
            resolved_at: None,
        };
        record_approved_outcome(&port, &candidate, 43).await;

        assert!(
            port.outcomes().is_empty(),
            "candidates without a rating must not write outcome rows"
        );
    }

    #[test]
    fn parse_strategies_extracts_titles_from_reflection_content() {
        use ene_store::{
            AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
            MemorySource, MemoryStatus,
        };

        fn reflection_item(title: &str, content: &str) -> MemoryItem {
            MemoryItem {
                id: Some(1),
                scope: MemoryScope::Shared,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: MemoryKind::Reflection,
                title: title.into(),
                content: content.into(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.7),
                salience: MemorySalience::new(0.6),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            }
        }

        let items = vec![
            reflection_item(
                "Successful strategies",
                "Successful interaction strategies: warm greeting, ask follow-up",
            ),
            reflection_item(
                "Strategies to avoid",
                "Less effective interaction strategies: long monologue",
            ),
        ];
        let (success, avoid) = parse_strategies(&items);
        assert!(success.iter().any(|s| s == "warm greeting"));
        assert!(success.iter().any(|s| s == "ask follow-up"));
        assert!(avoid.iter().any(|s| s == "long monologue"));
    }

    fn scored_memory(title: &str, total: f32) -> ene_core::ScoredMemory {
        use ene_store::{
            AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
            MemorySource, MemoryStatus,
        };

        ene_core::ScoredMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::Shared,
                character_id: "ene".into(),
                user_id: "user1".into(),
                kind: MemoryKind::Semantic,
                title: title.into(),
                content: String::new(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.7),
                salience: MemorySalience::new(0.6),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            },
            breakdown: ene_core::MemoryScoreBreakdown {
                total,
                ..ene_core::MemoryScoreBreakdown::default()
            },
            sources: Vec::new(),
        }
    }

    fn reflection_memory(title: &str, content: &str) -> ene_core::MemoryItem {
        use ene_store::{
            AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
            MemorySource, MemoryStatus,
        };

        MemoryItem {
            id: Some(1),
            scope: MemoryScope::Shared,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Reflection,
            title: title.into(),
            content: content.into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::new(0.6),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    #[test]
    fn apply_reflection_adjustment_records_multiplier_and_scales_total() {
        let reflections = vec![
            reflection_memory(
                "Successful strategies",
                "Successful interaction strategies: warm greeting",
            ),
            reflection_memory(
                "Strategies to avoid",
                "Less effective interaction strategies: long monologue",
            ),
        ];

        let mut memories = vec![
            scored_memory("warm greeting", 1.0),
            scored_memory("long monologue", 1.0),
            scored_memory("unrelated memory", 1.0),
        ];

        apply_reflection_adjustment(&mut memories, &reflections, 1.5, 0.5);

        let boosted = &memories[0];
        assert!((boosted.breakdown.reflection_multiplier - 1.5).abs() < f32::EPSILON);
        assert!((boosted.breakdown.total - 1.5).abs() < f32::EPSILON);

        let penalized = &memories[1];
        assert!((penalized.breakdown.reflection_multiplier - 0.5).abs() < f32::EPSILON);
        assert!((penalized.breakdown.total - 0.5).abs() < f32::EPSILON);

        let untouched = &memories[2];
        assert!((untouched.breakdown.reflection_multiplier - 1.0).abs() < f32::EPSILON);
        assert!((untouched.breakdown.total - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_reflection_adjustment_noop_without_strategies() {
        let mut memories = vec![scored_memory("warm greeting", 1.0)];
        apply_reflection_adjustment(&mut memories, &[], 1.5, 0.5);
        assert!((memories[0].breakdown.reflection_multiplier - 1.0).abs() < f32::EPSILON);
        assert!((memories[0].breakdown.total - 1.0).abs() < f32::EPSILON);
    }
}
