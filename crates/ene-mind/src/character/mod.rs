//! Character processing: CCv3 compilation, Identity Kernel, lorebook indexing (#82–#84).

mod authors_note;
mod compiler;
mod kernel;
mod lorebook;
mod style;
mod sync;

pub use authors_note::{AuthorsNote, apply_authors_note};
pub use compiler::{CharacterCompiler, DEFAULT_IDENTITY_KERNEL_MAX_TOKENS};
pub use kernel::IdentityKernel;
pub use lorebook::{
    LOREBOOK_SOURCE_PREFIX, LorebookIndexer, build_lorebook_scan_text,
    compile_lorebook_regex_cache, entry_keys_match, entry_keys_match_with_cache, stable_entry_id,
};
pub use style::{
    STYLE_SOURCE_PREFIX, StyleExample, StyleExampleSelector, StyleIntent, infer_style_intent,
};
pub use sync::{CharacterMemorySyncReport, compute_card_memory_hash, sync_character_memories};

use std::sync::Arc;

use ene_ai::EmbeddingProvider;
use ene_config::CharacterCardV3;
use ene_store::MemoryStore;

use crate::config::CharacterMemoryConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;

/// Character processing: `CCv3` compilation, lorebook indexing, style retrieval.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharacterProcessor;

impl CharacterProcessor {
    /// Compile the identity kernel for a character card.
    pub fn compile_kernel(
        card: &CharacterCardV3,
        user_name: &str,
        max_tokens: usize,
    ) -> IdentityKernel {
        CharacterCompiler::compile(card, user_name, max_tokens)
    }

    /// Compile the identity kernel using default token budget.
    pub fn compile_kernel_default(card: &CharacterCardV3, user_name: &str) -> IdentityKernel {
        Self::compile_kernel(card, user_name, DEFAULT_IDENTITY_KERNEL_MAX_TOKENS)
    }

    /// Synchronize `CCv3` lorebook and style indices into typed memory.
    pub async fn sync_card_memories(
        store: &MemoryStore,
        embedder: &Arc<dyn EmbeddingProvider>,
        character_id: &str,
        user_name: &str,
        card: &CharacterCardV3,
        config: &CharacterMemoryConfig,
        previous_hash: Option<u64>,
    ) -> Result<(CharacterMemorySyncReport, u64), CognitionError> {
        sync_character_memories(
            store,
            embedder,
            character_id,
            user_name,
            card,
            config,
            previous_hash,
        )
        .await
    }

    /// Select style examples for the current turn.
    pub async fn select_style_examples(
        card: &CharacterCardV3,
        user_name: &str,
        user_input: &str,
        _history: &[HistoryEntry],
        store: Option<&MemoryStore>,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
        config: &CharacterMemoryConfig,
        max_examples: usize,
    ) -> Vec<StyleExample> {
        StyleExampleSelector::select(
            card,
            user_name,
            user_input,
            store,
            embedder,
            config,
            max_examples,
        )
        .await
    }
}
