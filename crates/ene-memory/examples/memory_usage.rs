//! Memory store usage example.
//!
//! Demonstrates opening an in-memory store, inserting a summary with
//! key facts, searching by embedding similarity, and retrieving facts.

use chrono::Utc;
use ene_memory::{KeyFact, MemoryStore, RecalledSummary};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open an in-memory store with 4-dimensional embeddings
    let store = MemoryStore::open_in_memory(4).await?;

    let card_name = "Alicia";

    // Insert a conversation summary with key facts
    let embedding = vec![1.0_f32, 0.0, 0.0, 0.0];
    let key_facts = vec![
        KeyFact {
            key: "user_name".into(),
            value: "Alice".into(),
        },
        KeyFact {
            key: "favorite_color".into(),
            value: "blue".into(),
        },
    ];

    let summary_id = store
        .insert_summary(
            "session-001",
            card_name,
            "User Alice said she loves blue and works as a designer.",
            &key_facts,
            &embedding,
            Utc::now(),
        )
        .await?;

    println!("Inserted summary with ID: {summary_id}");

    // Search for related summaries
    let query_emb = vec![0.9_f32, 0.1, 0.0, 0.0];
    let results: Vec<RecalledSummary> = store
        .search_summaries(&query_emb, card_name, 5, 0.5)
        .await?;

    println!("\nFound {} related summaries:", results.len());
    for (i, rs) in results.iter().enumerate() {
        println!(
            "  {}. [score: {:.3}] {}",
            i + 1,
            rs.similarity,
            rs.entry.summary
        );
    }

    // Retrieve all key facts
    let facts = store.get_all_keyfacts(card_name).await?;
    println!("\nKey facts:");
    for fact in &facts {
        println!("  {} = {}", fact.key, fact.value);
    }

    // Upsert a fact
    store.upsert_keyfact(card_name, "favorite_color", "green").await?;
    let facts = store.get_all_keyfacts(card_name).await?;
    println!(
        "\nAfter upsert - favorite_color = {}",
        facts
            .iter()
            .find(|f| f.key == "favorite_color")
            .map_or("?", |f| f.value.as_str())
    );

    // Delete a fact
    store.delete_keyfact(card_name, "favorite_color").await?;
    let facts = store.get_all_keyfacts(card_name).await?;
    println!("\nAfter delete - {} facts remain", facts.len());

    Ok(())
}
