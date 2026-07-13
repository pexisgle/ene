//! Map legacy conversation summaries to typed Episodic memories.

use crate::typed_memory::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    MemoryStatus, NewMemoryItem,
};

/// Build a typed Episodic memory from a legacy summary row.
#[must_use]
pub fn summary_to_typed_memory(
    card_name: &str,
    user_id: &str,
    summary_text: &str,
    legacy_id: i64,
) -> NewMemoryItem {
    let title = summary_text
        .lines()
        .next()
        .unwrap_or(summary_text)
        .chars()
        .take(80)
        .collect::<String>();

    NewMemoryItem {
        scope: MemoryScope::Shared,
        character_id: card_name.to_string(),
        user_id: user_id.to_string(),
        kind: MemoryKind::Episodic,
        title,
        content: summary_text.to_string(),
        source: MemorySource::Imported,
        source_ref: Some(format!("legacy:summary:{legacy_id}")),
        confidence: MemoryConfidence::new(0.7),
        salience: MemorySalience::new(0.5),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
    }
}
