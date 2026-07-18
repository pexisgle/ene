//! `CCv3` character memory index synchronization (#83 / #84).

use std::collections::HashSet;
use std::sync::Arc;

use ene_ai::{EmbeddingKind, EmbeddingProvider};
use ene_config::CharacterCardV3;
use ene_store::{MemoryItem, MemoryStore, NewMemoryItem};

use crate::config::CharacterMemoryConfig;
use crate::error::CognitionError;

use super::lorebook::{LOREBOOK_SOURCE_PREFIX, LorebookIndexer};
use super::style::{STYLE_SOURCE_PREFIX, StyleExampleSelector};

/// Result of synchronizing CCv3-derived typed memories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharacterMemorySyncReport {
    /// Lorebook rows inserted.
    pub lorebook_inserted: usize,
    /// Lorebook rows superseded after content change.
    pub lorebook_updated: usize,
    /// Style example rows inserted.
    pub style_inserted: usize,
    /// Style example rows superseded after content change.
    pub style_updated: usize,
    /// Stale `CCv3` rows archived.
    pub archived: usize,
    /// Whether sync was skipped (unchanged card hash or disabled).
    pub skipped: bool,
}

/// Synchronize lorebook and style indices for a character card.
pub async fn sync_character_memories(
    store: &MemoryStore,
    embedder: &Arc<dyn EmbeddingProvider>,
    character_id: &str,
    user_name: &str,
    card: &CharacterCardV3,
    _config: &CharacterMemoryConfig,
    previous_card_memory_hash: Option<u64>,
) -> Result<(CharacterMemorySyncReport, u64), CognitionError> {
    let hash = compute_card_memory_hash(card);

    if previous_card_memory_hash == Some(hash) {
        return Ok((
            CharacterMemorySyncReport {
                skipped: true,
                ..Default::default()
            },
            hash,
        ));
    }

    let mut desired: Vec<NewMemoryItem> = Vec::new();
    desired.extend(LorebookIndexer::compile_entries(card, user_name));
    desired.extend(StyleExampleSelector::compile_items(card, user_name));

    let desired_refs: HashSet<String> = desired
        .iter()
        .filter_map(|item| item.source_ref.clone())
        .collect();

    let archived = store
        .archive_typed_memories_by_source_prefixes(
            character_id,
            &[LOREBOOK_SOURCE_PREFIX, STYLE_SOURCE_PREFIX],
            &desired_refs,
        )
        .await
        .map_err(CognitionError::Memory)?;

    let mut lorebook_inserted = 0usize;
    let mut lorebook_updated = 0usize;
    let mut style_inserted = 0usize;
    let mut style_updated = 0usize;

    for item in desired {
        let is_style = item
            .source_ref
            .as_deref()
            .is_some_and(|r| r.starts_with(STYLE_SOURCE_PREFIX));
        let is_lore = item
            .source_ref
            .as_deref()
            .is_some_and(|r| r.starts_with(LOREBOOK_SOURCE_PREFIX));

        let content = item.content.clone();
        let embedding = ene_ai::embed(embedder.as_ref(), &content, EmbeddingKind::Summary)
            .await
            .map_err(|e| CognitionError::Other(format!("CCv3 memory embedding failed: {e}")))?;

        let memory_id = if let Some(ref source_ref) = item.source_ref {
            match store
                .get_active_typed_memory_by_source_ref(character_id, source_ref)
                .await
                .map_err(CognitionError::Memory)?
            {
                Some(existing) if ccv3_item_matches(&existing, &item) => continue,
                Some(existing) => {
                    let id = existing.id.ok_or_else(|| {
                        CognitionError::Other(format!(
                            "active CCv3 memory missing id for source_ref={source_ref}"
                        ))
                    })?;
                    let new_id = store
                        .supersede_typed_memory(&item, id)
                        .await
                        .map_err(CognitionError::Memory)?;
                    if is_lore {
                        lorebook_updated += 1;
                    } else if is_style {
                        style_updated += 1;
                    }
                    new_id
                }
                None => {
                    let id = store
                        .insert_typed_memory(&item)
                        .await
                        .map_err(CognitionError::Memory)?;
                    if is_lore {
                        lorebook_inserted += 1;
                    } else if is_style {
                        style_inserted += 1;
                    }
                    id
                }
            }
        } else {
            let id = store
                .insert_typed_memory(&item)
                .await
                .map_err(CognitionError::Memory)?;
            if is_lore {
                lorebook_inserted += 1;
            } else if is_style {
                style_inserted += 1;
            }
            id
        };

        store
            .upsert_memory_embedding(memory_id, embedder.model_name(), &content, &embedding)
            .await
            .map_err(CognitionError::Memory)?;
    }

    Ok((
        CharacterMemorySyncReport {
            lorebook_inserted,
            lorebook_updated,
            style_inserted,
            style_updated,
            archived,
            skipped: false,
        },
        hash,
    ))
}

/// Combined content hash for `CCv3` lorebook + style indices.
///
/// Used to skip per-turn sync when the session already holds a matching hash.
#[must_use]
pub fn compute_card_memory_hash(card: &CharacterCardV3) -> u64 {
    card_memory_hash_combined(
        LorebookIndexer::content_hash(card),
        style_content_hash(card),
    )
}

fn ccv3_item_matches(existing: &MemoryItem, desired: &NewMemoryItem) -> bool {
    existing.content == desired.content
        && existing.title == desired.title
        && existing.pinned == desired.pinned
        && (existing.salience.get() - desired.salience.get()).abs() < f32::EPSILON
        && existing.kind == desired.kind
}

fn style_content_hash(card: &CharacterCardV3) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    card.data.mes_example.hash(&mut hasher);
    hasher.finish()
}

const fn card_memory_hash_combined(lore: u64, style: u64) -> u64 {
    lore ^ style.rotate_left(17)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ene_ai::{EmbeddingKind, EmbeddingProvider};
    use ene_config::CharacterCardV3;

    use super::*;
    use crate::config::CharacterMemoryConfig;

    #[test]
    fn compute_card_memory_hash_is_stable_for_same_card() {
        let card = CharacterCardV3::default();
        let a = compute_card_memory_hash(&card);
        let b = compute_card_memory_hash(&card);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn sync_skips_when_previous_hash_matches() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
        let card = CharacterCardV3::default();
        let config = CharacterMemoryConfig::default();
        let hash = compute_card_memory_hash(&card);

        let (report, returned) =
            sync_character_memories(&store, &embedder, "ene", "user", &card, &config, Some(hash))
                .await
                .expect("sync");

        assert!(report.skipped);
        assert_eq!(returned, hash);
    }

    struct MockEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbedder {
        fn model_name(&self) -> &'static str {
            "mock"
        }

        fn dimensions(&self) -> usize {
            4
        }

        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }
    }
}
