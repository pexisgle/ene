// Memory Writer: deterministic/LLM extraction + Memory Arbiter.

pub mod arbiter;
pub mod candidate;
pub mod deterministic;
pub mod forgetting;
pub mod llm;
pub mod reflection;
#[cfg(test)]
pub mod test_support;
pub mod tool_grounding;

pub use arbiter::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, MemoryArbiter, SemanticMatch,
};
pub use candidate::ToolResultSummary;
pub use forgetting::{ForgettingContext, ForgettingLifecycle, ForgettingReport};
pub use reflection::{
    SelfReflectionPipeline, apply_reflection_adjustment, load_reflection_memories,
};

use std::collections::HashMap;

use chrono::Utc;
use ene_ai::{EmbeddingProvider, LlmProvider};
use ene_core::MemoryPort;

use crate::commitments::{CommitmentLedger, CommitmentSyncContext};
use crate::config::MindConfig;
use crate::error::CognitionError;
use crate::lifecycle::PostTurnInput;

/// Optional providers for LLM extraction and semantic deduplication.
pub struct MemoryWriteProviders<'a> {
    /// LLM provider for optional memory candidate extraction.
    pub llm: Option<&'a dyn LlmProvider>,
    /// Embedding provider for pre-arbitration semantic duplicate detection.
    pub embedder: Option<&'a dyn EmbeddingProvider>,
}

#[expect(
    clippy::derivable_impls,
    reason = "manual Default keeps non-default field ordering explicit"
)]
impl Default for MemoryWriteProviders<'_> {
    fn default() -> Self {
        Self {
            llm: None,
            embedder: None,
        }
    }
}

/// Memory Writer orchestrator.
#[derive(Debug, Default)]
pub struct MemoryWriter;

impl MemoryWriter {
    /// Extract and persist memories from the turn when enabled.
    ///
    /// When LLM extraction is enabled and returns candidates, those candidates
    /// are the sole conversation/tool judgment (remember patterns and tool
    /// grounding are hints / fallback only). Explicit forget patterns are always
    /// applied as a safety net, even when the LLM owns the turn. On LLM failure,
    /// empty result, or when disabled, remember patterns and configured
    /// tool-grounding fallbacks reach the arbiter.
    ///
    /// Returns the number of memory candidates deferred to the user-approval
    /// queue during this turn, so callers can notify the UI.
    pub async fn write_memories(
        store: &dyn MemoryPort,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<usize, CognitionError> {
        let locale = locale_from_classifier_language(&config.emotion.classifier_language);
        let turn = candidate::TurnInput {
            user_message: input.turn.user_message,
            assistant_message: input.turn.assistant_message,
            tool_results: input.turn.tool_results,
        };

        let deterministic = deterministic::extract_with_tool_grounding(
            &turn,
            locale,
            config.memory.min_confidence_to_persist as f32,
            &config.memory.tool_grounding,
        )?;

        let (tool_candidates, pattern_candidates): (
            Vec<candidate::MemoryCandidate>,
            Vec<candidate::MemoryCandidate>,
        ) = deterministic
            .into_iter()
            .partition(|c| c.source_quote.is_empty());

        tracing::debug!(
            component = "MemoryWriter",
            character_id = %input.character_id,
            user_id = %input.user_id,
            locale = ?locale,
            pattern_hint_count = pattern_candidates.len(),
            tool_candidate_count = tool_candidates.len(),
            "Post-turn memory extraction starting"
        );

        let mut batches: Vec<(Vec<candidate::MemoryCandidate>, CandidateProvenance)> = Vec::new();
        let mut used_llm = false;

        if let Some(provider) = providers.llm {
            // Soft tool hints include successes even when auto-persist is off.
            let mut llm_hints = pattern_candidates.clone();
            let tool_hint_cfg = crate::config::ToolGroundingConfig {
                persist_success_procedure: true,
                persist_failure_reflection: true,
                persist_user_visible_episodic: false,
                ..config.memory.tool_grounding.clone()
            };
            llm_hints.extend(tool_grounding::extract_tool_candidates(
                turn.tool_results,
                &tool_hint_cfg,
            ));

            match llm::extract_with_timeout(
                provider,
                &turn,
                locale,
                config.memory.extraction_timeout_secs,
                &llm_hints,
            )
            .await
            {
                Ok(llm_candidates) => {
                    tracing::debug!(
                        component = "MemoryWriter",
                        character_id = %input.character_id,
                        user_id = %input.user_id,
                        candidate_count = llm_candidates.len(),
                        pattern_hint_count = pattern_candidates.len(),
                        "LLM memory extraction completed"
                    );
                    if llm_candidates.is_empty() {
                        // Leave `used_llm` false so remember/forget
                        // safety-net patterns still reach the arbiter.
                        tracing::debug!(
                            component = "MemoryWriter",
                            character_id = %input.character_id,
                            "LLM returned no candidates; will use remember/forget patterns if any"
                        );
                    } else {
                        used_llm = true;
                        batches.push((llm_candidates, CandidateProvenance::LlmExtracted));
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        component = "MemoryWriter",
                        error = %error,
                        character_id = %input.character_id,
                        user_id = %input.user_id,
                        "LLM memory extraction failed; falling back to deterministic candidates"
                    );
                }
            }
        } else {
            tracing::warn!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                "No LLM provider available for memory extraction"
            );
        }

        // Remember/forget patterns fall back when LLM did not own the turn.
        // Forget requests are always applied even when LLM owned the turn so
        // explicit「忘れて」cannot be dropped if the model omits a deletion.
        let (forget_candidates, other_patterns): (
            Vec<candidate::MemoryCandidate>,
            Vec<candidate::MemoryCandidate>,
        ) = pattern_candidates
            .into_iter()
            .partition(|c| !c.should_persist);

        if !forget_candidates.is_empty() {
            tracing::debug!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                candidate_count = forget_candidates.len(),
                "Applying deterministic forget candidates (safety net)"
            );
            batches.push((forget_candidates, CandidateProvenance::Deterministic));
        }

        if !used_llm && !other_patterns.is_empty() {
            tracing::debug!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                candidate_count = other_patterns.len(),
                "Using deterministic remember candidates (LLM disabled, failed, or empty)"
            );
            batches.push((other_patterns, CandidateProvenance::Deterministic));
        } else if used_llm && !other_patterns.is_empty() {
            tracing::debug!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                hint_count = other_patterns.len(),
                "Deterministic remember patterns used as LLM hints only (not auto-persisted)"
            );
        }

        if !used_llm && !tool_candidates.is_empty() {
            tracing::debug!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                candidate_count = tool_candidates.len(),
                "Using tool-grounding fallback candidates (LLM disabled, failed, or empty)"
            );
            batches.push((tool_candidates, CandidateProvenance::Deterministic));
        } else if used_llm && !tool_candidates.is_empty() {
            tracing::debug!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                tool_candidate_count = tool_candidates.len(),
                "Tool-grounding candidates skipped; LLM owned tool-result judgment"
            );
        }

        if batches.is_empty() {
            return Ok(0);
        }

        // Tag every candidate from an interrupted (barge-in / cancelled) turn so
        // downstream consumers can distinguish partial episodes.
        if input.interrupted {
            for (candidates, _) in &mut batches {
                for candidate in candidates.iter_mut() {
                    if !candidate
                        .tags
                        .iter()
                        .any(|tag| tag == arbiter::INTERRUPTED_TAG)
                    {
                        candidate.tags.push(arbiter::INTERRUPTED_TAG.to_string());
                    }
                }
            }
        }

        let options = ArbiterOptions::from_config(&config.memory);
        let sync_ctx = CommitmentSyncContext {
            character_id: input.character_id,
            user_id: input.user_id,
            embedder: providers.embedder,
            title_similarity_threshold: config.memory.commitment_title_similarity_threshold,
        };
        let mut outcome_summary = ArbiterOutcomeSummary::default();
        let reflection = config
            .memory
            .reflection
            .enabled
            .then(|| reflection::SelfReflectionPipeline::new(config.memory.reflection.clone()));

        for (candidates, provenance) in batches {
            let semantic_matches = build_semantic_matches(
                store,
                providers.embedder,
                &config.memory,
                input.character_id,
                input.user_id,
                &candidates,
                options.semantic_similarity_threshold,
            )
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(
                    component = "MemoryWriter",
                    error = %error,
                    character_id = %input.character_id,
                    "Semantic dedup lookup failed; continuing without matches"
                );
                HashMap::new()
            });

            let base_ctx = ArbiterContext {
                turn: turn.clone(),
                character_id: input.character_id,
                user_id: input.user_id,
                source_ref: None,
                provenance,
                options: options.clone(),
                semantic_matches,
                affect_valence: input.affect.valence,
                embedder: providers.embedder,
            };

            let mut regular = Vec::new();

            for (idx, candidate) in candidates.into_iter().enumerate() {
                if candidate.source_quote.is_empty() {
                    let source_ref =
                        Some(format!("tool:{}:{}", sanitize_ref(&candidate.title), idx));
                    let ctx = ArbiterContext {
                        source_ref: source_ref.as_deref(),
                        ..base_ctx.clone()
                    };
                    let (applied, _synced_commitments) =
                        CommitmentLedger::arbitrate_apply_and_sync(
                            store,
                            &[candidate],
                            &ctx,
                            &sync_ctx,
                        )
                        .await?;
                    record_arbiter_outcomes(
                        input,
                        &applied,
                        &mut outcome_summary,
                        reflection.as_ref(),
                    );
                } else {
                    regular.push(candidate);
                }
            }

            if !regular.is_empty() {
                let (applied, _synced_commitments) = CommitmentLedger::arbitrate_apply_and_sync(
                    store, &regular, &base_ctx, &sync_ctx,
                )
                .await?;
                record_arbiter_outcomes(input, &applied, &mut outcome_summary, reflection.as_ref());
            }
        }

        tracing::info!(
            component = "MemoryWriter",
            character_id = %input.character_id,
            user_id = %input.user_id,
            persisted = outcome_summary.persisted,
            skipped = outcome_summary.skipped,
            deferred = outcome_summary.deferred,
            other = outcome_summary.other,
            "Post-turn memory arbitration complete"
        );

        if let Some(pipeline) = reflection
            && pipeline.should_reflect()
        {
            let reflections = pipeline
                .generate_reflection(
                    store,
                    input.character_id,
                    input.character_id,
                    input.user_id,
                    config.memory.reflection.success_boost,
                    config.memory.reflection.failure_penalty,
                )
                .await;
            tracing::info!(
                component = "MemoryWriter",
                character_id = %input.character_id,
                reflection_count = reflections.len(),
                "Self-reflection memories generated"
            );
        }

        Ok(outcome_summary.deferred)
    }

    /// Persist affect state only (forgetting runs on the deferred path).
    pub async fn finalize_turn(
        store: &dyn MemoryPort,
        _config: &MindConfig,
        input: &PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        store
            .upsert_affect_state(input.affect)
            .await
            .map_err(CognitionError::MemoryPort)?;
        tracing::debug!(
            component = "MemoryWriter",
            event = "affect state updated",
            character_id = %input.character_id,
            user_id = %input.user_id,
            mood = %input.affect.mood_label,
            valence = input.affect.valence,
            arousal = input.affect.arousal,
            "Affect state persisted"
        );

        Ok(())
    }

    /// Apply natural forgetting lifecycle for the turn scope.
    pub async fn apply_forgetting(
        store: &dyn MemoryPort,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        let forgetting_ctx = ForgettingContext {
            character_id: input.character_id,
            user_id: Some(input.user_id),
            now: Utc::now(),
        };
        let _report = ForgettingLifecycle::apply(store, &forgetting_ctx, &config.memory).await?;
        Ok(())
    }

    /// Extract, arbitrate, persist memories, apply forgetting, and update affect.
    pub async fn after_turn(
        store: &dyn MemoryPort,
        config: &MindConfig,
        input: PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError> {
        Self::write_memories(store, config, &input, providers).await?;
        Self::apply_forgetting(store, config, &input).await?;
        Self::finalize_turn(store, config, &input).await
    }
}

async fn build_semantic_matches(
    store: &dyn MemoryPort,
    embedder: Option<&dyn EmbeddingProvider>,
    config: &crate::config::MindMemoryConfig,
    character_id: &str,
    user_id: &str,
    candidates: &[candidate::MemoryCandidate],
    similarity_threshold: f32,
) -> Result<HashMap<usize, Vec<SemanticMatch>>, CognitionError> {
    let Some(embedder) = embedder else {
        return Ok(HashMap::new());
    };

    let mut matches = HashMap::new();
    let user_filter = if user_id.is_empty() {
        None
    } else {
        Some(user_id)
    };

    for (idx, candidate) in candidates.iter().enumerate() {
        let query_text = format!("{} {}", candidate.title, candidate.content);
        let query_embedding = ene_ai::embed_query(embedder, query_text.trim())
            .await
            .map_err(CognitionError::Embedding)?;
        let options = ene_core::Query {
            query_text: query_text.as_str(),
            embedding: Some(&query_embedding),
            character_id,
            user_id: user_filter,
            model_name: embedder.model_name(),
            limit: 5,
            similarity_threshold,
            candidate_pool_size: 8,
            query_affect: None,
            weights: config.hybrid_weights,
            decay_half_life_days: config.default_forgetting_half_life_days,
            now: Utc::now(),
            min_score: 0.0,
            commitment_boost: 0.0,
            recent_fallback_limit: 0,
            time_range: None,
            exclude_kinds: Vec::new(),
        };

        let gathered = store
            .search(&options)
            .await
            .map_err(CognitionError::MemoryPort)?;
        let scored = ene_rag::score_and_rank(&options, gathered);

        let semantic: Vec<SemanticMatch> = scored
            .into_iter()
            .filter(|hit| hit.breakdown.vector_similarity >= similarity_threshold)
            .map(|hit| SemanticMatch {
                memory_id: hit.item.id.unwrap_or_default(),
                similarity: hit.breakdown.vector_similarity,
                memory: hit.item,
            })
            .collect();

        if !semantic.is_empty() {
            matches.insert(idx, semantic);
        }
    }

    Ok(matches)
}

fn sanitize_ref(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    if out.is_empty() {
        "tool".to_string()
    } else {
        out
    }
}

const fn locale_from_classifier_language(lang: &str) -> candidate::Locale {
    if lang.eq_ignore_ascii_case("ja") {
        candidate::Locale::Ja
    } else {
        candidate::Locale::En
    }
}

#[derive(Default)]
struct ArbiterOutcomeSummary {
    persisted: usize,
    skipped: usize,
    deferred: usize,
    other: usize,
}

fn record_arbiter_outcomes(
    input: &PostTurnInput<'_>,
    applied: &[crate::memory_writer::AppliedDecision],
    summary: &mut ArbiterOutcomeSummary,
    reflection: Option<&reflection::SelfReflectionPipeline>,
) {
    for outcome in applied {
        if let Some(pipeline) = reflection {
            pipeline.record_outcome(outcome);
        }
        match &outcome.decision.action {
            crate::memory_writer::ArbiterAction::Persist(_)
            | crate::memory_writer::ArbiterAction::Supersede { .. } => {
                summary.persisted = summary.persisted.saturating_add(1);
                tracing::debug!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    user_id = %input.user_id,
                    kind = ?outcome.decision.candidate.kind,
                    title = %outcome.decision.candidate.title,
                    inserted_id = ?outcome.inserted_id,
                    reason_code = ?outcome.decision.reason.code,
                    "Memory candidate persisted"
                );
            }
            crate::memory_writer::ArbiterAction::AskConfirmationLater { .. } => {
                summary.deferred = summary.deferred.saturating_add(1);
                tracing::debug!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    user_id = %input.user_id,
                    kind = ?outcome.decision.candidate.kind,
                    title = %outcome.decision.candidate.title,
                    reason_code = ?outcome.decision.reason.code,
                    reason_detail = %outcome.decision.reason.detail,
                    "Memory candidate deferred to user-approval queue"
                );
            }
            crate::memory_writer::ArbiterAction::Ignore => {
                summary.skipped = summary.skipped.saturating_add(1);
                tracing::debug!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    user_id = %input.user_id,
                    kind = ?outcome.decision.candidate.kind,
                    title = %outcome.decision.candidate.title,
                    reason_code = ?outcome.decision.reason.code,
                    reason_detail = %outcome.decision.reason.detail,
                    "Memory candidate was not persisted"
                );
            }
            other => {
                summary.other = summary.other.saturating_add(1);
                tracing::debug!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    user_id = %input.user_id,
                    action = ?other,
                    reason_code = ?outcome.decision.reason.code,
                    "Memory arbiter applied non-insert action"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::TurnInput;
    use ene_ai::{LlmCompletion, LlmMessage};
    use ene_store::{
        AffectAnnotation, AffectState, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
        MemorySource, MemoryStatus, MemoryStore, NewMemoryItem,
    };

    type StreamResult = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                > + Send,
        >,
    >;

    struct MockProvider {
        response: String,
    }

    impl MockProvider {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "mock: not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, ene_ai::LlmProviderError> {
            Ok(LlmCompletion::text_only(self.response.clone()))
        }
    }

    struct FailingProvider;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for FailingProvider {
        fn name(&self) -> &'static str {
            "failing-mock"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "failing: not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "forced extraction failure".to_string(),
            ))
        }
    }

    #[test]
    fn locale_from_classifier_language_maps_ja() {
        assert_eq!(locale_from_classifier_language("ja"), candidate::Locale::Ja);
        assert_eq!(locale_from_classifier_language("JA"), candidate::Locale::Ja);
        assert_eq!(locale_from_classifier_language("en"), candidate::Locale::En);
    }

    #[tokio::test]
    async fn finalize_turn_runs() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let config = MindConfig::default();

        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "hello",
                assistant_message: Some("hi"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::finalize_turn(&store, &config, &post)
            .await
            .expect("finalize_turn");

        let loaded = store.get_affect_state("ene").await.unwrap();
        assert_eq!(loaded.character_id, affect.character_id);
    }

    #[tokio::test]
    async fn apply_forgetting_runs() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let config = MindConfig::default();

        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "hello",
                assistant_message: Some("hi"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::apply_forgetting(&store, &config, &post)
            .await
            .expect("apply_forgetting");
    }

    #[tokio::test]
    async fn llm_success_with_candidates_skips_deterministic_remember() {
        // Explicit remember would also match, but LLM returns its own candidate
        // → only the LLM item should be persisted (remember is hint-only).
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let json = r#"{
            "candidates": [{
                "kind": "Preference",
                "title": "likes mushrooms",
                "content": "User likes mushrooms",
                "source_quote": "Please remember that I like mushrooms",
                "confidence": 0.8,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "Please remember that I like mushrooms",
                assistant_message: Some("Nice!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let memories = store
            .get_typed_memories_by_character("ene", None, 10, 0)
            .await
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].title, "likes mushrooms");
        assert_eq!(memories[0].source, ene_store::MemorySource::LlmExtracted);
    }

    #[tokio::test]
    async fn llm_empty_falls_back_to_remember_pattern() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let provider = MockProvider::new(r#"{"candidates": []}"#);
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "Please remember that I like mushrooms",
                assistant_message: Some("Nice!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let count = store.count_typed_memories("ene", None).await.unwrap();
        assert!(
            count >= 1,
            "empty LLM result should fall back to explicit remember pattern"
        );
    }

    #[tokio::test]
    async fn llm_empty_does_not_persist_soft_signals_without_remember() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let provider = MockProvider::new(r#"{"candidates": []}"#);
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "I like mushrooms",
                assistant_message: Some("Nice!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let count = store.count_typed_memories("ene", None).await.unwrap();
        assert_eq!(
            count, 0,
            "soft preference without remember must not pattern-persist"
        );
    }

    #[tokio::test]
    async fn llm_success_persists_llm_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let json = r#"{
            "candidates": [{
                "kind": "Episodic",
                "title": "presentation today",
                "content": "User has a presentation today",
                "source_quote": "I have a presentation today",
                "confidence": 0.8,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "I have a presentation today",
                assistant_message: Some("Good luck!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let memories = store
            .get_typed_memories_by_character("ene", None, 10, 0)
            .await
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].title, "presentation today");
        assert_eq!(memories[0].kind, ene_store::MemoryKind::Episodic);
    }

    #[tokio::test]
    async fn llm_failure_falls_back_to_remember_pattern() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let provider = FailingProvider;
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "Please remember that I like mushrooms",
                assistant_message: Some("Nice!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let count = store.count_typed_memories("ene", None).await.unwrap();
        assert!(
            count >= 1,
            "deterministic remember should persist on LLM failure"
        );
    }

    #[tokio::test]
    async fn no_llm_provider_uses_remember_pattern() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "Please remember that I like mushrooms",
                assistant_message: Some("Nice!"),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(&store, &config, &post, MemoryWriteProviders::default())
            .await
            .expect("write_memories");

        let count = store.count_typed_memories("ene", None).await.unwrap();
        assert!(count >= 1);
    }

    #[tokio::test]
    async fn forget_safety_net_applies_when_llm_owns_turn() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.emotion.classifier_language = "en".into();

        let existing_id = store
            .insert_typed_memory(&NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: "ene".to_string(),
                user_id: "user".to_string(),
                kind: MemoryKind::Semantic,
                title: "project X".to_string(),
                content: "User is working on project X".to_string(),
                source: MemorySource::Inferred,
                source_ref: None,
                confidence: MemoryConfidence::new(0.8),
                salience: MemorySalience::default(),
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

        // LLM owns the turn with an unrelated candidate and omits deletion.
        let json = r#"{
            "candidates": [{
                "kind": "Semantic",
                "title": "ack",
                "content": "User asked to forget something",
                "source_quote": "Forget about project X",
                "confidence": 0.8,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "Forget about project X",
                assistant_message: Some("Okay, forgotten."),
                tool_results: &[],
            },
            affect: &affect,
            character_id: "ene",
            user_id: "user",
            interrupted: false,
            spoken_text: None,
        };

        MemoryWriter::write_memories(
            &store,
            &config,
            &post,
            MemoryWriteProviders {
                llm: Some(&provider),
                embedder: None,
            },
        )
        .await
        .expect("write_memories");

        let existing = store
            .get_typed_memory(existing_id)
            .await
            .unwrap()
            .expect("memory row");
        assert_eq!(
            existing.status,
            MemoryStatus::UserDeleted,
            "forget pattern must mark target deleted even when LLM owns the turn"
        );
    }
}
