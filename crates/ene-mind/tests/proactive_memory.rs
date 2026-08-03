//! Integration tests for proactive speech memory access: deterministic
//! suppression-condition injection into the decision and topic-hint recall
//! into the generation pre-turn.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::default_trait_access,
    reason = "integration tests use unwrap/expect/Default for concise fixtures"
)]

use ene_core::{MemoryKind, MemoryStatus};
use ene_mind::{CognitionEngine, MindConfig, TurnContext, load_proactive_memory_notes};
use ene_store::{MemoryStore, NewMemoryItem};

fn test_config() -> MindConfig {
    let mut config = MindConfig {
        language: "en".into(),
        ..MindConfig::default()
    };
    config.memory.recall_similarity_threshold = 0.0;
    config.memory.recall_min_score = 0.0;
    // Topic recall must be driven purely by lexical overlap; the recent
    // fallback would otherwise surface unrelated memories and muddy assertions.
    config.memory.recent_fallback_limit = 0;
    config
}

async fn insert_memory(store: &MemoryStore, kind: MemoryKind, title: &str, content: &str) -> i64 {
    use ene_core::{MemoryConfidence, MemorySalience, MemoryScope, MemorySource};
    store
        .insert_typed_memory(&NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user".into(),
            kind,
            title: title.into(),
            content: content.into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.9),
            salience: MemorySalience::new(0.8),
            affect: Default::default(),
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
        .unwrap()
}

fn turn_ctx<'a>(
    config: &'a MindConfig,
    card: &'a ene_config::CharacterCardV3,
    store: &'a MemoryStore,
    proactive_topic: Option<&'a str>,
) -> TurnContext<'a> {
    TurnContext {
        config,
        card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess",
        user_input: "",
        history: &[],
        greeting_index: None,
        store: Some(store),
        query_embedding: None,
        embedder: None,
        llm_provider: None,
        available_window: None,
        post_history_block: None,
        compression_pending: false,
        packing_budget_override: None,
        proactive_topic,
    }
}

#[tokio::test]
async fn memory_notes_inject_user_standing_rules_without_scoring() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    insert_memory(
        &store,
        MemoryKind::Preference,
        "Do not disturb",
        "don't talk while I work",
    )
    .await;
    insert_memory(
        &store,
        MemoryKind::UserProfile,
        "Night owl",
        "quiet at night",
    )
    .await;
    insert_memory(&store, MemoryKind::Semantic, "Fact", "the sky is blue").await;

    let notes = load_proactive_memory_notes(&store, "ene", "user", 12)
        .await
        .expect("load memory notes");
    assert_eq!(
        notes.len(),
        2,
        "only Preference/UserProfile kinds are injected"
    );
    assert!(notes.iter().any(|n| n.contains("don't talk while I work")));
    assert!(notes.iter().any(|n| n.contains("quiet at night")));
}

#[tokio::test]
async fn before_proactive_turn_recalls_memories_matching_the_topic() {
    let mut config = test_config();
    config.proactive.sources.memory = true;
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    insert_memory(
        &store,
        MemoryKind::Episodic,
        "Presentation day",
        "the user has a big presentation today at 3pm",
    )
    .await;
    // Unrelated memory must not surface for this topic.
    insert_memory(
        &store,
        MemoryKind::Preference,
        "Drinks",
        "user prefers matcha",
    )
    .await;

    let card = ene_config::CharacterCardV3::default();
    let ctx = turn_ctx(
        &config,
        &card,
        &store,
        Some("Ask how the presentation went"),
    );
    let pre = CognitionEngine::new()
        .before_proactive_turn(ctx)
        .await
        .expect("proactive pre-turn");

    assert_eq!(
        pre.recalled.len(),
        1,
        "only the topic-relevant memory is recalled"
    );
    assert!(pre.recalled[0].item.content.contains("presentation"));
}

#[tokio::test]
async fn before_proactive_turn_skips_recall_when_memory_source_disabled() {
    let mut config = test_config();
    config.proactive.sources.memory = false;
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    insert_memory(
        &store,
        MemoryKind::Episodic,
        "Presentation day",
        "the user has a big presentation today",
    )
    .await;

    let card = ene_config::CharacterCardV3::default();
    let ctx = turn_ctx(
        &config,
        &card,
        &store,
        Some("Ask how the presentation went"),
    );
    let pre = CognitionEngine::new()
        .before_proactive_turn(ctx)
        .await
        .expect("proactive pre-turn");
    assert!(
        pre.recalled.is_empty(),
        "disabled memory source must leave generation recall empty"
    );
}

#[tokio::test]
async fn before_proactive_turn_skips_recall_without_a_topic_hint() {
    let config = test_config();
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    insert_memory(
        &store,
        MemoryKind::Episodic,
        "Presentation day",
        "the user has a big presentation today",
    )
    .await;

    let card = ene_config::CharacterCardV3::default();
    let ctx = turn_ctx(&config, &card, &store, None);
    let pre = CognitionEngine::new()
        .before_proactive_turn(ctx)
        .await
        .expect("proactive pre-turn");
    assert!(
        pre.recalled.is_empty(),
        "no topic hint means nothing to recall against"
    );
}
