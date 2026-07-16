//! Memory store usage example.
//!
//! Demonstrates the current typed-memory API: inserting memories via
//! `NewMemoryItem`, searching via `Query`, and lifecycle management.
//! The legacy `search_summaries` / `get_all_keyfacts` APIs have been
//! retired from the product path (#125).

use chrono::Utc;
use ene_store::{
    AffectAnnotation, HybridSearchWeights, MemoryConfidence, MemoryKind, MemorySalience,
    MemoryScope, MemorySource, MemoryStatus, MemoryStore, NewMemoryItem, Query,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open an in-memory store with 4-dimensional embeddings
    let store = MemoryStore::open_in_memory(4).await?;

    // Insert a typed memory (Semantic kind — the modern persistence path).
    let memory = NewMemoryItem {
        scope: MemoryScope::Shared,
        character_id: "Ene".to_string(),
        user_id: "user1".to_string(),
        kind: MemoryKind::Semantic,
        title: "user_name".to_string(),
        content: "Alice is a designer who loves the color blue.".to_string(),
        source: MemorySource::Conversation,
        source_ref: Some("session-001".to_string()),
        confidence: MemoryConfidence::new(0.9),
        salience: MemorySalience::new(0.7),
        affect: AffectAnnotation {
            valence: 0.5,
            arousal: 0.0,
        },
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let id = store.insert_typed_memory(&memory).await?;
    println!("Inserted typed memory with ID: {id}");

    // Search with a Query (no embedding — lexical/recency-only match).
    let results = store
        .search(&Query {
            query_text: "Alice loves blue",
            embedding: None,
            character_id: "Ene",
            user_id: Some("user1"),
            model_name: "test",
            limit: 5,
            similarity_threshold: 0.0,
            candidate_pool_size: 16,
            query_affect: None,
            weights: HybridSearchWeights::default(),
            decay_half_life_days: 30.0,
            now: Utc::now(),
            min_score: 0.0,
            commitment_boost: 0.0,
            recent_fallback_limit: 5,
        })
        .await?;

    println!("\nSearch results (lexical-only):");
    for (i, scored) in results.iter().enumerate() {
        println!(
            "  {}. [total: {:.3}] {}",
            i.saturating_add(1),
            scored.breakdown.total,
            scored.item.content
        );
    }

    // Mark the memory as faded (lifecycle transition).
    store
        .update_typed_memory_status(id, MemoryStatus::Faded)
        .await?;
    println!("\nMemory {id} marked as faded.");

    Ok(())
}
