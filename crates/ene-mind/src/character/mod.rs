//! Character processing: CCv3 compilation, Identity Kernel, lorebook indexing.

mod authors_note;
mod compiler;
mod kernel;
mod lorebook;
mod style;
mod sync;

pub use authors_note::{AuthorsNote, apply_authors_note};
pub use compiler::{CharacterCompiler, identity_kernel_budget_tokens};
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
use ene_config::{CharacterCardV3, UserPersona};
use ene_core::MemoryPort;

use crate::config::CharacterMemoryConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;

/// Character processing: `CCv3` compilation, lorebook indexing, style retrieval.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharacterProcessor;

impl CharacterProcessor {
    /// Compile the identity kernel for a character card.
    ///
    /// `available_window` is the number of prompt tokens the model's context
    /// window leaves for this turn; the kernel budget is derived from it as a
    /// fraction so larger models carry a fuller definition.
    /// `user_persona`, when provided, expands the `{{user_persona}}` CBS macro
    /// in card-derived fields.
    ///
    /// `pick_seed`, when provided, keeps `{{pick}}` stable for the lifetime of
    /// the chat instead of re-rolling on each per-turn recompilation;
    /// derive it with [`ene_config::session_pick_seed`].
    pub fn compile_kernel(
        card: &CharacterCardV3,
        user_name: &str,
        user_persona: Option<&UserPersona>,
        pick_seed: Option<u64>,
        available_window: usize,
    ) -> IdentityKernel {
        CharacterCompiler::compile(card, user_name, user_persona, pick_seed, available_window)
    }

    /// Compile the identity kernel assuming the conservative default window.
    ///
    /// Used when no model window is known (tests and callers outside a live
    /// turn); the budget is still derived from a window rather than a fixed
    /// token count.
    pub fn compile_kernel_default(card: &CharacterCardV3, user_name: &str) -> IdentityKernel {
        let window = usize::try_from(ene_ai::DEFAULT_CONTEXT_WINDOW).unwrap_or(usize::MAX);
        Self::compile_kernel(card, user_name, None, None, window)
    }

    /// Synchronize `CCv3` lorebook and style indices into typed memory.
    pub async fn sync_card_memories(
        store: &dyn MemoryPort,
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
        store: Option<&dyn MemoryPort>,
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
