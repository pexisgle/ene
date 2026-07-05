// Memory Writer: deterministic/LLM extraction + Memory Arbiter.

pub mod arbiter;
pub mod candidate;
pub mod deterministic;
pub mod forgetting;
pub mod llm;

pub use arbiter::{
    AppliedDecision, ArbiterAction, ArbiterContext, ArbiterOptions, ArbiterReason,
    ArbiterReasonCode, CandidateDecision, CandidateProvenance, MemoryArbiter, SemanticMatch,
};
pub use forgetting::{ForgettingContext, ForgettingLifecycle, ForgettingReport};

use std::collections::HashMap;

use chrono::Utc;
use ene_memory::MemoryStore;

use crate::config::CognitionConfig;
use crate::error::CognitionError;
use crate::lifecycle::PostTurnInput;

/// Memory Writer orchestrator.
#[derive(Debug, Default)]
pub struct MemoryWriter;

impl MemoryWriter {
    /// Extract and persist memories from the turn when enabled.
    pub async fn write_memories(
        store: &MemoryStore,
        config: &CognitionConfig,
        input: &PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        if !config.memory.write_every_turn {
            return Ok(());
        }

        let candidates = deterministic::extract(
            &input.turn,
            candidate::Locale::En,
            config.memory.min_confidence_to_persist as f32,
        )?;

        if candidates.is_empty() {
            return Ok(());
        }

        let options = ArbiterOptions {
            min_confidence: config.memory.min_confidence_to_persist as f32,
            ..Default::default()
        };
        let ctx = ArbiterContext {
            turn: candidate::TurnInput {
                user_message: input.turn.user_message,
                assistant_message: input.turn.assistant_message,
                tool_results: input.turn.tool_results,
            },
            character_id: input.character_id,
            user_id: input.user_id,
            source_ref: None,
            provenance: CandidateProvenance::Deterministic,
            options,
            semantic_matches: HashMap::new(),
        };
        MemoryArbiter::arbitrate_and_apply(store, &candidates, &ctx).await?;
        Ok(())
    }

    /// Apply forgetting lifecycle and persist affect state.
    pub async fn finalize_turn(
        store: &MemoryStore,
        config: &CognitionConfig,
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

        Ok(())
    }

    /// Extract, arbitrate, persist memories, apply forgetting, and update affect.
    pub async fn after_turn(
        store: &MemoryStore,
        config: &CognitionConfig,
        input: PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        Self::write_memories(store, config, &input).await?;
        Self::finalize_turn(store, config, &input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::TurnInput;
    use ene_memory::{AffectState, MemoryStore};

    #[tokio::test]
    async fn finalize_turn_runs_when_write_every_turn_disabled() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut config = CognitionConfig::default();
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
