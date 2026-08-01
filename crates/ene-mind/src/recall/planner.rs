use chrono::{DateTime, Utc};
use ene_core::{
    ActiveCommitmentPrompt, AffectAnnotation, AffectState, MemoryKind, Query, ScoredMemory,
};

use super::input::RecallPlannerInput;
use super::intent::{RecallIntent, infer_intents, kinds_for_intents};
use super::plan::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};
use super::result::RecalledMemory;
use super::topic::{current_topic, normalize_text, recent_user_turn};
use crate::config::MindMemoryConfig;
use crate::error::CognitionError;

const DEFAULT_CANDIDATE_POOL_MULTIPLIER: usize = 4;

/// Recall Planner: deterministic recall-plan generation from turn context.
///
/// This type does not execute hybrid memory search or call embedding providers.
/// Downstream recall execution consumes the resulting [`RecallPlan`] and may call
/// [`RecallPlanner::to_memory_search_options`] for a single primary query.
#[derive(Debug, Default, Clone, Copy)]
pub struct RecallPlanner;

/// Options controlling deterministic recall-plan generation.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallPlannerOptions {
    /// Maximum number of memory results requested by the plan.
    pub result_limit: usize,
    /// Minimum vector similarity for vector-sourced candidates.
    pub similarity_threshold: f32,
    /// Minimum hybrid total score required to return a result.
    pub min_score: f32,
    /// Half-life in days for recency decay.
    pub decay_half_life_days: f64,
}

impl RecallPlannerOptions {
    /// Build recall-planner options from mind memory config.
    pub fn from_config(memory: &MindMemoryConfig) -> Self {
        Self {
            result_limit: memory.recall_result_limit.max(1),
            similarity_threshold: memory.recall_similarity_threshold.clamp(0.0, 1.0),
            min_score: memory.recall_min_score.clamp(0.0, 1.0),
            decay_half_life_days: memory.default_forgetting_half_life_days.max(0.0),
        }
    }
}

impl Default for RecallPlannerOptions {
    fn default() -> Self {
        Self::from_config(&MindMemoryConfig::default())
    }
}

impl RecallPlanner {
    /// Generate a recall plan from the current turn context.
    pub fn plan(
        input: &RecallPlannerInput<'_>,
        options: &RecallPlannerOptions,
    ) -> Result<RecallPlan, CognitionError> {
        let topic = current_topic(input.user_input, input.recent_turns, input.scene_summary)
            .ok_or_else(|| {
                CognitionError::RecallFailed("cannot plan recall without turn text".to_string())
            })?;
        let intents = infer_intents(&topic, input.affect, input.language);
        let has_commitments = !input.commitments.is_empty();
        let required_kinds = kinds_for_intents(&intents, has_commitments);

        let semantic_queries = semantic_queries(&topic, &intents, input.commitments);
        let episodic_queries = episodic_queries(&topic, input.recent_turns, &intents);
        let primary_query_text = semantic_queries
            .first()
            .cloned()
            .unwrap_or_else(|| topic.clone());

        Ok(RecallPlan {
            current_topic: topic,
            semantic_queries,
            episodic_queries,
            required_kinds,
            scope: RecallScopeFilter {
                character_id: input.character_id.to_string(),
                user_id: input.user_id.map(ToOwned::to_owned),
            },
            budget: RecallBudgetHints {
                result_limit: options.result_limit,
            },
            search: RecallSearchHints {
                primary_query_text,
                similarity_threshold: options.similarity_threshold,
                min_score: options.min_score,
                decay_half_life_days: options.decay_half_life_days,
                query_affect: input.affect.and_then(query_affect),
            },
        })
    }

    /// Convert a recall plan into a store [`Query`] for one primary query.
    ///
    /// Maps [`RecallSearchHints`] and scope fields onto [`Query`], filling
    /// hybrid weights / commitment boost from [`MindMemoryConfig`].
    /// Multi-query expansion and `required_kinds` filtering remain the
    /// responsibility of downstream recall execution.
    pub fn to_query<'a>(
        plan: &'a RecallPlan,
        embedding: Option<&'a [f32]>,
        model_name: &'a str,
        now: DateTime<Utc>,
        memory: &MindMemoryConfig,
    ) -> Query<'a> {
        let limit = plan.budget.result_limit.max(1);
        Query {
            query_text: &plan.search.primary_query_text,
            embedding,
            character_id: &plan.scope.character_id,
            user_id: plan.scope.user_id.as_deref(),
            model_name,
            limit,
            similarity_threshold: plan.search.similarity_threshold,
            candidate_pool_size: limit
                .saturating_mul(DEFAULT_CANDIDATE_POOL_MULTIPLIER)
                .max(limit)
                .max(16),
            query_affect: plan.search.query_affect,
            weights: memory.hybrid_weights,
            decay_half_life_days: plan.search.decay_half_life_days,
            access_boost_half_life_days: memory.access_boost_half_life_days.max(0.0),
            now,
            min_score: plan.search.min_score,
            commitment_boost: memory.commitment_boost,
            recent_fallback_limit: memory.recent_fallback_limit,
            time_range: None,
            // Reflection memories are a scoring signal applied separately by
            // the self-reflection pipeline, never ordinary recall
            // results — when the pipeline is enabled they are excluded from
            // every gather path so they cannot compete with normal memories or
            // leak into the LLM context. With the pipeline disabled the
            // exclusion is off too: reflections behave as ordinary recall
            // results, so enabling the pipeline is the only observable behavior
            // change. The set of excluded kinds comes from the per-kind policy
            // table (`MemoryKind::is_recall_eligible`); today it is exactly
            // Reflection.
            exclude_kinds: if memory.reflection.enabled {
                MemoryKind::ALL
                    .iter()
                    .copied()
                    .filter(|kind| !kind.is_recall_eligible())
                    .collect()
            } else {
                vec![]
            },
        }
    }

    /// Alias for [`Self::to_query`] kept for call-site clarity.
    pub fn to_memory_search_options<'a>(
        plan: &'a RecallPlan,
        query_embedding: &'a [f32],
        model_name: &'a str,
        now: DateTime<Utc>,
        memory: &MindMemoryConfig,
    ) -> Query<'a> {
        Self::to_query(plan, Some(query_embedding), model_name, now, memory)
    }

    /// Map hybrid search results into explainable recalled memories.
    ///
    /// Cognition-side entry point for attaching recall reasons after
    /// `MemoryStore::search`.
    pub fn explain_results(scored: Vec<ScoredMemory>) -> Vec<RecalledMemory> {
        super::executor::RecallResultMapper::map(scored)
    }
}

fn semantic_queries(
    topic: &str,
    intents: &[RecallIntent],
    commitments: &[ActiveCommitmentPrompt],
) -> Vec<String> {
    let mut queries = vec![topic.to_string()];
    let lower = topic.to_lowercase();

    if intents.contains(&RecallIntent::Preference) {
        push_query(&mut queries, &format!("{topic} preference"));
    }
    if intents.contains(&RecallIntent::Relationship) {
        push_query(&mut queries, &format!("{topic} relationship context"));
    }
    if intents.contains(&RecallIntent::Procedure) {
        push_query(&mut queries, &format!("{topic} procedure steps"));
    }
    if crate::contains_any(
        &lower,
        ["what do you know", "tell me about", "とは", "について"],
    ) {
        push_query(&mut queries, &format!("{topic} facts"));
    }

    for commitment in commitments.iter().take(3) {
        push_query(
            &mut queries,
            &format!("active commitment: {}", commitment.title),
        );
    }

    queries
}

fn episodic_queries(
    topic: &str,
    recent_turns: &[super::input::RecallTurn<'_>],
    intents: &[RecallIntent],
) -> Vec<String> {
    let mut queries = vec![topic.to_string()];

    if intents.contains(&RecallIntent::Episodic) {
        push_query(&mut queries, &format!("past conversation about {topic}"));
    }

    if let Some(recent) = recent_user_turn(recent_turns)
        && recent != topic
    {
        push_query(&mut queries, &recent);
    }

    queries
}

fn query_affect(state: &AffectState) -> Option<AffectAnnotation> {
    let valence = clamp_unit_signed(state.valence);
    let arousal = clamp_unit_signed(state.arousal);
    if valence.abs() < f32::EPSILON && arousal.abs() < f32::EPSILON {
        None
    } else {
        Some(AffectAnnotation { valence, arousal })
    }
}

const fn clamp_unit_signed(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn push_query(queries: &mut Vec<String>, query: &str) {
    if let Some(normalized) = normalize_text(query)
        && !queries.iter().any(|existing| existing == &normalized)
    {
        queries.push(normalized);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recall::RecallTurn;
    use ene_store::{ActiveCommitmentPrompt, DiscreteEmotion};

    fn input<'a>(
        user_input: &'a str,
        turns: &'a [RecallTurn<'a>],
        scene_summary: Option<&'a str>,
        affect: Option<&'a AffectState>,
        commitments: &'a [ActiveCommitmentPrompt],
    ) -> RecallPlannerInput<'a> {
        RecallPlannerInput {
            user_input,
            recent_turns: turns,
            scene_summary,
            affect,
            commitments,
            language: "en",
            character_id: "ene",
            user_id: Some("user1"),
        }
    }

    fn input_with_defaults<'a>(
        user_input: &'a str,
        turns: &'a [RecallTurn<'a>],
        affect: Option<&'a AffectState>,
        commitments: &'a [ActiveCommitmentPrompt],
    ) -> RecallPlannerInput<'a> {
        input(user_input, turns, None, affect, commitments)
    }

    fn commitment() -> ActiveCommitmentPrompt {
        ActiveCommitmentPrompt {
            id: 1,
            title: "discuss design".to_string(),
            description: "Talk about the runtime design next time".to_string(),
            due_label: Some("next time".to_string()),
            due_at: None,
        }
    }

    #[test]
    fn simple_input_generates_semantic_and_episodic_queries() {
        let plan = RecallPlanner::plan(
            &input_with_defaults("What do you know about Rust?", &[], None, &[]),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(!plan.semantic_queries.is_empty());
        assert!(!plan.episodic_queries.is_empty());
        assert!(
            plan.required_kinds
                .contains(&ene_store::MemoryKind::Semantic)
        );
        assert!(
            plan.required_kinds
                .contains(&ene_store::MemoryKind::Episodic)
        );
    }

    #[test]
    fn preference_input_biases_semantic_queries() {
        let plan = RecallPlanner::plan(
            &input_with_defaults("I like coffee in the morning", &[], None, &[]),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(
            plan.required_kinds
                .contains(&ene_store::MemoryKind::Preference)
        );
        assert!(
            plan.semantic_queries
                .iter()
                .any(|query| query.to_lowercase().contains("coffee"))
        );
    }

    #[test]
    fn active_commitments_require_commitment_kind() {
        let commitments = vec![commitment()];
        let plan = RecallPlanner::plan(
            &input_with_defaults("What should we talk about?", &[], None, &commitments),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(
            plan.required_kinds
                .contains(&ene_store::MemoryKind::Commitment)
        );
    }

    #[test]
    fn no_commitments_omits_commitment_kind() {
        let plan = RecallPlanner::plan(
            &input_with_defaults("What should we talk about?", &[], None, &[]),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(
            !plan
                .required_kinds
                .contains(&ene_store::MemoryKind::Commitment)
        );
    }

    #[test]
    fn affect_maps_to_query_affect() {
        let affect = AffectState {
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            valence: 0.7,
            arousal: -0.4,
            dominance: 0.0,
            trust: 0.0,
            affinity: 0.0,
            irritation: 0.0,
            curiosity: 0.0,
            fatigue: 0.0,
            mood_label: "warm".to_string(),
            last_expression: "smiling".to_string(),
            discrete_emotions: vec![DiscreteEmotion::new("joy", 0.7)],
            updated_at: None,
        };

        let plan = RecallPlanner::plan(
            &input_with_defaults("That made me happy", &[], Some(&affect), &[]),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert_eq!(
            plan.search.query_affect,
            Some(AffectAnnotation {
                valence: 0.7,
                arousal: -0.4
            })
        );
    }

    #[test]
    fn budget_hints_follow_memory_config() {
        let memory = MindMemoryConfig {
            recall_result_limit: 11,
            ..MindMemoryConfig::default()
        };
        let options = RecallPlannerOptions::from_config(&memory);

        let plan = RecallPlanner::plan(
            &input_with_defaults("Tell me about our project", &[], None, &[]),
            &options,
        )
        .expect("plan");

        assert_eq!(plan.budget.result_limit, 11);
    }

    #[test]
    fn to_memory_search_options_maps_scope_and_limits() {
        let memory = MindMemoryConfig {
            recall_result_limit: 3,
            recall_similarity_threshold: 0.42,
            recall_min_score: 0.21,
            reflection: crate::config::ReflectionConfig {
                enabled: true,
                ..crate::config::ReflectionConfig::default()
            },
            ..MindMemoryConfig::default()
        };
        let options = RecallPlannerOptions::from_config(&memory);
        let plan = RecallPlanner::plan(
            &input_with_defaults("Remember our design discussion", &[], None, &[]),
            &options,
        )
        .expect("plan");
        let embedding = [0.1, 0.2, 0.3];
        let now = Utc::now();

        let search =
            RecallPlanner::to_memory_search_options(&plan, &embedding, "model-a", now, &memory);

        assert_eq!(search.query_text, plan.search.primary_query_text.as_str());
        assert_eq!(search.character_id, "ene");
        assert_eq!(search.user_id, Some("user1"));
        assert_eq!(search.model_name, "model-a");
        assert_eq!(search.limit, 3);
        assert!((search.similarity_threshold - 0.42).abs() < f32::EPSILON);
        assert!((search.min_score - 0.21).abs() < f32::EPSILON);
        assert_eq!(
            search.exclude_kinds,
            vec![ene_store::MemoryKind::Reflection],
            "recall queries must exclude reflection memories while the pipeline is enabled (#347)"
        );

        // With the pipeline disabled the exclusion must be off too: reflections
        // keep behaving as ordinary recall results, so enabling the pipeline is
        // the only observable behavior change.
        let disabled_memory = MindMemoryConfig {
            reflection: crate::config::ReflectionConfig {
                enabled: false,
                ..crate::config::ReflectionConfig::default()
            },
            ..memory
        };
        let search = RecallPlanner::to_memory_search_options(
            &plan,
            &embedding,
            "model-a",
            now,
            &disabled_memory,
        );
        assert!(
            search.exclude_kinds.is_empty(),
            "reflection exclusion must be gated on the reflection pipeline"
        );
    }

    #[test]
    fn scene_summary_only_input_generates_plan() {
        let plan = RecallPlanner::plan(
            &input(
                "   ",
                &[],
                Some("Continuing the runtime design review"),
                None,
                &[],
            ),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert_eq!(plan.current_topic, "Continuing the runtime design review");
    }

    #[test]
    fn empty_context_returns_recall_failed() {
        let err = RecallPlanner::plan(
            &input_with_defaults("   ", &[], None, &[]),
            &RecallPlannerOptions::default(),
        )
        .expect_err("empty context should fail");

        assert!(matches!(err, CognitionError::RecallFailed(_)));
    }

    #[test]
    fn episodic_keyword_expands_episodic_queries() {
        let plan = RecallPlanner::plan(
            &input_with_defaults("Remember what we discussed last time", &[], None, &[]),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(
            plan.episodic_queries
                .iter()
                .any(|query| query.contains("past conversation about"))
        );
    }

    #[test]
    fn commitment_title_appears_in_semantic_queries() {
        let commitments = vec![commitment()];
        let plan = RecallPlanner::plan(
            &input_with_defaults("What should we talk about?", &[], None, &commitments),
            &RecallPlannerOptions::default(),
        )
        .expect("plan");

        assert!(
            plan.semantic_queries
                .iter()
                .any(|query| query.contains("active commitment: discuss design"))
        );
    }
}
