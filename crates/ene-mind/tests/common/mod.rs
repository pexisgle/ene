//! Shared fixtures for `ene-mind` integration tests.

#![expect(
    clippy::unwrap_used,
    reason = "shared test helper uses unwrap for concise fixture setup"
)]

use ene_core::{
    MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource, MemoryStatus,
};
use ene_store::{MemoryStore, NewMemoryItem};

/// Insert a typed memory with the standard test fixture shape (salience 0.8,
/// active, character `ene`, user `user`) and return its assigned ID.
pub async fn insert_memory(
    store: &MemoryStore,
    scope: MemoryScope,
    kind: MemoryKind,
    title: &str,
    content: &str,
    source: MemorySource,
    confidence: f32,
) -> i64 {
    store
        .insert_typed_memory(&NewMemoryItem {
            scope,
            character_id: "ene".into(),
            user_id: "user".into(),
            kind,
            title: title.into(),
            content: content.into(),
            source,
            source_ref: None,
            confidence: MemoryConfidence::new(confidence),
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
