//! Lorebook recall boosts for constant and key-triggered entries (#83).

use ene_config::CharacterCardV3;
use ene_core::{MemoryItem, MemoryPort, MemoryScoreBreakdown, MemorySource};

use crate::character::{
    LOREBOOK_SOURCE_PREFIX, build_lorebook_scan_text, compile_lorebook_regex_cache,
    entry_keys_match_with_cache, stable_entry_id,
};
use crate::error::CognitionError;
use crate::recall::{RecallReason, RecallTurn, RecalledMemory};

/// Merge pinned and key-triggered lorebook memories into hybrid recall results.
pub async fn merge_lorebook_recall(
    store: &dyn MemoryPort,
    character_id: &str,
    card: Option<&CharacterCardV3>,
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    recalled: Vec<RecalledMemory>,
) -> Result<Vec<RecalledMemory>, CognitionError> {
    let Some(card) = card else {
        return Ok(recalled);
    };
    let Some(book) = card.data.character_book.as_ref() else {
        return Ok(recalled);
    };

    let scan_depth = book.scan_depth.unwrap_or(4);
    let scan_text = build_lorebook_scan_text(user_input, recent_turns, scan_depth);
    let regex_cache = compile_lorebook_regex_cache(book);

    let indexed = store
        .list_typed_memories_by_source_prefix(character_id, LOREBOOK_SOURCE_PREFIX, 128)
        .await
        .map_err(CognitionError::MemoryPort)?;

    let mut boosted: Vec<RecalledMemory> = Vec::new();
    let mut seen_ids = recalled
        .iter()
        .filter_map(|m| m.item.id)
        .collect::<std::collections::HashSet<_>>();

    for item in &indexed {
        let Some(id) = item.id else {
            continue;
        };
        if seen_ids.contains(&id) {
            continue;
        }

        let should_include =
            item.pinned || lorebook_entry_matches(item, book, &scan_text, &regex_cache);
        if !should_include {
            continue;
        }

        seen_ids.insert(id);
        boosted.push(recalled_memory_from_item(item.clone()));
    }

    if book.recursive_scanning.unwrap_or(false) && !boosted.is_empty() {
        let mut extended_scan = scan_text;
        for memory in &boosted {
            extended_scan.push('\n');
            extended_scan.push_str(&memory.item.content);
        }
        for item in &indexed {
            let Some(id) = item.id else {
                continue;
            };
            if seen_ids.contains(&id) || item.pinned {
                continue;
            }
            if lorebook_entry_matches(item, book, &extended_scan, &regex_cache) {
                seen_ids.insert(id);
                boosted.push(recalled_memory_from_item(item.clone()));
            }
        }
    }

    let mut merged = boosted;
    merged.extend(recalled);
    Ok(merged)
}

fn recalled_memory_from_item(item: MemoryItem) -> RecalledMemory {
    RecalledMemory {
        item,
        reason: RecallReason::CharacterLore,
        score_breakdown: MemoryScoreBreakdown {
            vector_similarity: 0.0,
            lexical_score: 1.0,
            recency_score: 1.0,
            salience: 1.0,
            confidence: 1.0,
            emotional_match: 0.0,
            relationship: 0.0,
            access_boost: 0.0,
            relevance: 1.0,
            quality_factor: 1.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total: 1.0,
        },
        sources: vec![ene_core::MemoryCandidateSource::Lexical],
    }
}

fn lorebook_entry_matches(
    item: &MemoryItem,
    book: &ene_config::Lorebook,
    scan_text: &str,
    regex_cache: &std::collections::HashMap<String, regex::Regex>,
) -> bool {
    if item.source != MemorySource::Ccv3 {
        return false;
    }
    let Some(source_ref) = &item.source_ref else {
        return false;
    };
    if !source_ref.starts_with(LOREBOOK_SOURCE_PREFIX) {
        return false;
    }
    let entry_id = source_ref.trim_start_matches(LOREBOOK_SOURCE_PREFIX);
    book.entries
        .iter()
        .enumerate()
        .find(|(idx, e)| e.enabled && stable_entry_id(e, *idx) == entry_id)
        .is_some_and(|(idx, entry)| {
            entry_keys_match_with_cache(entry, idx, scan_text, Some(regex_cache))
        })
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_config::LorebookEntry;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemoryStatus,
    };

    #[test]
    fn lorebook_entry_matches_resolves_card_entry_by_source_ref() {
        let item = MemoryItem {
            id: Some(1),
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "Dragon lore".into(),
            content: "A dragon.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("ccv3:lorebook:dragon".into()),
            confidence: MemoryConfidence::default(),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        };
        let book = ene_config::Lorebook {
            entries: vec![LorebookEntry {
                keys: vec!["dragon".into()],
                content: "A dragon.".into(),
                extensions: Default::default(),
                enabled: true,
                insertion_order: 1,
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
                not_keys: Vec::new(),
                sticky_turns: None,
                turns_since_match: None,
            }],
            ..Default::default()
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        assert!(lorebook_entry_matches(
            &item,
            &book,
            "I saw a dragon",
            &regex_cache
        ));
    }
}
