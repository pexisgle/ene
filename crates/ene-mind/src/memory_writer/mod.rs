// Memory Writer: deterministic/LLM extraction + Memory Arbiter.

pub mod arbiter;
pub mod candidate;
pub mod deterministic;
pub mod forgetting;
pub mod llm;
pub mod tool_grounding;

pub use arbiter::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, MemoryArbiter, SemanticMatch,
};
pub use candidate::ToolResultSummary;
pub use forgetting::{ForgettingContext, ForgettingLifecycle, ForgettingReport};

use std::collections::HashMap;

use chrono::Utc;
use ene_ai::{EmbeddingProvider, LlmProvider};
use ene_store::MemoryStore;

use crate::commitments::{CommitmentLedger, CommitmentSyncContext};
use crate::config::MindConfig;
use crate::error::CognitionError;
use crate::lifecycle::PostTurnInput;

/// Optional providers for LLM extraction and semantic deduplication.
pub struct MemoryWriteProviders<'a> {
    /// LLM provider for optional memory candidate extraction (#66).
    pub llm: Option<&'a dyn LlmProvider>,
    /// Embedding provider for pre-arbitration semantic duplicate detection (#75).
    pub embedder: Option<&'a dyn EmbeddingProvider>,
}

#[expect(clippy::derivable_impls)]
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
    pub async fn write_memories(
        store: &MemoryStore,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError> {
        if !config.memory.write_every_turn {
            return Ok(());
        }

        let turn = candidate::TurnInput {
            user_message: input.turn.user_message,
            assistant_message: input.turn.assistant_message,
            tool_results: input.turn.tool_results,
        };

        let mut batches: Vec<(Vec<candidate::MemoryCandidate>, CandidateProvenance)> = Vec::new();

        let deterministic = deterministic::extract_with_tool_grounding(
            &turn,
            candidate::Locale::En,
            config.memory.min_confidence_to_persist as f32,
            &config.memory.tool_grounding,
        )?;
        tracing::debug!(
            component = "MemoryWriter",
            event = "memory candidates extracted",
            character_id = %input.character_id,
            user_id = %input.user_id,
            turn_id = 0usize,
            candidate_count = deterministic.len(),
            provenance = "deterministic",
            "Deterministic memory candidates extracted"
        );
        if !deterministic.is_empty() {
            batches.push((deterministic, CandidateProvenance::Deterministic));
        }

        if config.memory.llm_extraction_enabled {
            if let Some(provider) = providers.llm {
                match llm::extract_with_timeout(
                    provider,
                    &turn,
                    candidate::Locale::En,
                    config.memory.extraction_timeout_secs,
                )
                .await
                {
                    Ok(llm_candidates) => {
                        tracing::debug!(
                            component = "MemoryWriter",
                            event = "memory candidates extracted",
                            character_id = %input.character_id,
                            user_id = %input.user_id,
                            turn_id = 0usize,
                            candidate_count = llm_candidates.len(),
                            provenance = "llm",
                            "LLM memory candidates extracted"
                        );
                        if !llm_candidates.is_empty() {
                            batches.push((llm_candidates, CandidateProvenance::LlmExtracted));
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            component = "MemoryWriter",
                            error = %error,
                            character_id = %input.character_id,
                            user_id = %input.user_id,
                            "LLM memory extraction failed; continuing with deterministic candidates"
                        );
                    }
                }
            } else {
                tracing::debug!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    "LLM memory extraction enabled but no provider available"
                );
            }
        }

        if batches.is_empty() {
            return Ok(());
        }

        let options = ArbiterOptions::from_config(&config.memory);
        let sync_ctx = CommitmentSyncContext {
            character_id: input.character_id,
            user_id: input.user_id,
        };

        for (candidates, provenance) in batches {
            let semantic_matches = if config.memory.semantic_dedup_enabled {
                build_semantic_matches(
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
                })
            } else {
                HashMap::new()
            };

            let base_ctx = ArbiterContext {
                turn: turn.clone(),
                character_id: input.character_id,
                user_id: input.user_id,
                source_ref: None,
                provenance,
                options: options.clone(),
                semantic_matches,
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
                    log_arbiter_outcomes(input, &applied);
                } else {
                    regular.push(candidate);
                }
            }

            if !regular.is_empty() {
                let (applied, _synced_commitments) = CommitmentLedger::arbitrate_apply_and_sync(
                    store, &regular, &base_ctx, &sync_ctx,
                )
                .await?;
                log_arbiter_outcomes(input, &applied);
            }
        }

        Ok(())
    }

    /// Apply forgetting lifecycle and persist affect state.
    pub async fn finalize_turn(
        store: &MemoryStore,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        let forgetting_ctx = ForgettingContext {
            character_id: input.character_id,
            user_id: Some(input.user_id),
            now: Utc::now(),
        };
        let _report = ForgettingLifecycle::apply(store, &forgetting_ctx, &config.memory).await?;

        store
            .upsert_affect_state(&input.affect)
            .await
            .map_err(CognitionError::Memory)?;
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

    /// Extract, arbitrate, persist memories, apply forgetting, and update affect.
    pub async fn after_turn(
        store: &MemoryStore,
        config: &MindConfig,
        input: PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError> {
        Self::write_memories(store, config, &input, providers).await?;
        Self::finalize_turn(store, config, &input).await
    }
}

async fn build_semantic_matches(
    store: &MemoryStore,
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
        let options = ene_store::Query {
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
        };

        let scored = store
            .search(&options)
            .await
            .map_err(CognitionError::Memory)?;

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

fn log_arbiter_outcomes(
    input: &PostTurnInput<'_>,
    applied: &[crate::memory_writer::AppliedDecision],
) {
    for outcome in applied {
        if matches!(
            outcome.decision.action,
            crate::memory_writer::ArbiterAction::Ignore
                | crate::memory_writer::ArbiterAction::AskConfirmationLater
        ) {
            tracing::debug!(
                component = "MemoryWriter",
                event = "memory candidate rejected",
                character_id = %input.character_id,
                user_id = %input.user_id,
                reason_code = ?outcome.decision.reason.code,
                reason_detail = %outcome.decision.reason.detail,
                "Memory candidate was not persisted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::TurnInput;
    use ene_store::{AffectState, MemoryStore};

    #[tokio::test]
    async fn finalize_turn_runs_when_write_every_turn_disabled() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = MindConfig::default();
        config.memory.write_every_turn = false;

        let affect = AffectState::neutral("ene");
        let post = PostTurnInput {
            turn: TurnInput {
                user_message: "hello",
                assistant_message: Some("hi"),
                tool_results: &[],
            },
            affect: affect.clone(),
            character_id: "ene",
            user_id: "user",
        };

        MemoryWriter::finalize_turn(&store, &config, &post)
            .await
            .expect("finalize_turn");

        let loaded = store.get_affect_state("ene").await.unwrap();
        assert_eq!(loaded.character_id, affect.character_id);
    }
}
