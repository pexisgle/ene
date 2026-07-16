//! Phase 5 character processing integration tests (#82–#84).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use ene_ai::{EmbeddingKind, EmbeddingProvider};
use ene_config::{CharacterCardV3, Lorebook, LorebookEntry};
use ene_mind::character::{
    CharacterProcessor, LorebookIndexer, StyleExampleSelector, sync_character_memories,
};
use ene_mind::config::CharacterMemoryConfig;
use ene_mind::recall::merge_lorebook_recall;
use ene_store::MemoryStore;

struct MockEmbedder;

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    fn model_name(&self) -> &'static str {
        "mock-4d"
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

fn lorebook_card() -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.character_book = Some(Lorebook {
        entries: vec![
            LorebookEntry {
                keys: vec!["dragon".into()],
                content: "Dragons guard the northern pass.".into(),
                extensions: Default::default(),
                enabled: true,
                insertion_order: 10,
                case_sensitive: None,
                use_regex: false,
                constant: Some(false),
                name: Some("Dragon lore".into()),
                priority: None,
                id: Some(serde_json::json!("dragon")),
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
            },
            LorebookEntry {
                keys: vec![],
                content: "The world is always sunny.".into(),
                extensions: Default::default(),
                enabled: true,
                insertion_order: 20,
                case_sensitive: None,
                use_regex: false,
                constant: Some(true),
                name: Some("World tone".into()),
                priority: None,
                id: Some(serde_json::json!("constant")),
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
            },
        ],
        ..Default::default()
    });
    card
}

#[tokio::test]
async fn lorebook_sync_and_constant_recall_merge() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let card = lorebook_card();
    let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(MockEmbedder);
    let config = CharacterMemoryConfig::default();

    let (report, hash) =
        sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
            .await
            .expect("sync");

    assert!(!report.skipped);
    assert!(report.lorebook_inserted >= 2);

    let merged = merge_lorebook_recall(
        &store,
        "Ene",
        Some(&card),
        "Tell me about the weather",
        &[],
        vec![],
    )
    .await
    .expect("merge");

    assert!(
        merged
            .iter()
            .any(|m| m.item.content.contains("always sunny")),
        "constant lorebook entry should be merged"
    );

    let (report2, hash2) =
        sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, Some(hash))
            .await
            .expect("resync");
    assert!(report2.skipped);
    assert_eq!(hash, hash2);

    let key_merged = merge_lorebook_recall(
        &store,
        "Ene",
        Some(&card),
        "I met a dragon today",
        &[],
        vec![],
    )
    .await
    .expect("key merge");
    assert!(
        key_merged
            .iter()
            .any(|m| m.item.content.contains("northern pass")),
        "key-triggered lorebook entry should merge"
    );
}

#[test]
fn style_selector_picks_greeting_example() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.mes_example =
        "<START>\n{{user}}: Hi\n{{char}}: Hey there!\n<START>\n{{user}}: explain\n{{char}}: Because science.".into();

    let chunks = StyleExampleSelector::chunk_mes_example(&card.data.mes_example, "Ene", "User");
    assert_eq!(chunks.len(), 2);

    let compiled = StyleExampleSelector::compile_items(&card, "User");
    assert_eq!(compiled.len(), 2);
    assert!(
        compiled[0]
            .source_ref
            .as_ref()
            .unwrap()
            .starts_with("ccv3:style:")
    );

    let kernel = CharacterProcessor::compile_kernel_default(&card, "User");
    assert!(kernel.text.contains("Hard instruction: remain Ene"));
}

#[test]
fn lorebook_compile_skips_disabled_entries() {
    let mut card = lorebook_card();
    card.data.character_book.as_mut().unwrap().entries[0].enabled = false;
    let items = LorebookIndexer::compile_entries(&card, "User");
    assert_eq!(items.len(), 1);
    assert!(items[0].pinned);
}

#[tokio::test]
async fn lorebook_content_update_supersedes_existing_row() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mut card = lorebook_card();
    let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(MockEmbedder);
    let config = CharacterMemoryConfig::default();

    sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
        .await
        .expect("initial sync");

    card.data.character_book.as_mut().unwrap().entries[0].content =
        "Dragons now guard the eastern gate.".into();

    let (report, _) =
        sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
            .await
            .expect("resync after edit");
    assert_eq!(report.lorebook_updated, 1);

    let merged = merge_lorebook_recall(
        &store,
        "Ene",
        Some(&card),
        "I met a dragon today",
        &[],
        vec![],
    )
    .await
    .expect("merge");
    assert!(
        merged
            .iter()
            .any(|m| m.item.content.contains("eastern gate")),
        "updated lorebook content should be recalled"
    );
}

#[tokio::test]
async fn style_content_update_supersedes_existing_row() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.mes_example = "<START>\n{{user}}: Hi\n{{char}}: Hey there!".into();
    let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(MockEmbedder);
    let config = CharacterMemoryConfig::default();

    sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
        .await
        .expect("initial sync");

    card.data.mes_example = "<START>\n{{user}}: Hi\n{{char}}: Hello friend!".into();

    let (report, _) =
        sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
            .await
            .expect("resync after edit");
    assert_eq!(report.style_updated, 1);

    let items = store
        .list_typed_memories_by_source_prefix("Ene", "ccv3:style:", 8)
        .await
        .expect("list style");
    assert!(items.iter().any(|m| m.content.contains("Hello friend!")));
}

#[tokio::test]
async fn disabled_lorebook_entry_is_archived_on_resync() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mut card = lorebook_card();
    let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(MockEmbedder);
    let config = CharacterMemoryConfig::default();

    sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
        .await
        .expect("initial sync");

    card.data.character_book.as_mut().unwrap().entries[0].enabled = false;

    let (report, _) =
        sync_character_memories(&store, &embedder, "Ene", "User", &card, &config, None)
            .await
            .expect("resync after disable");
    assert!(report.archived >= 1);

    let key_merged = merge_lorebook_recall(
        &store,
        "Ene",
        Some(&card),
        "I met a dragon today",
        &[],
        vec![],
    )
    .await
    .expect("merge");
    assert!(
        !key_merged
            .iter()
            .any(|m| m.item.content.contains("northern pass")),
        "disabled lorebook entry should not be recalled"
    );
}
