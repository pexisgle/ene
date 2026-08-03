//! Lorebook recall boosts for constant and key-triggered entries.

use ene_config::{CharacterCardV3, LorebookEntry};
use ene_core::{MemoryItem, MemoryPort, MemoryScoreBreakdown, MemorySource};

use crate::character::{
    ActivationContext, EntryDecorators, LOREBOOK_SOURCE_PREFIX, build_lorebook_scan_text,
    compile_lorebook_regex_cache, entry_decorators_accept, entry_keys_match_with_cache,
    stable_entry_id,
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

        if lorebook_include(
            item,
            book,
            user_input,
            recent_turns,
            &scan_text,
            scan_depth,
            &regex_cache,
        ) {
            seen_ids.insert(id);
            boosted.push(recalled_memory_from_item(item.clone()));
        }
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
            if lorebook_include(
                item,
                book,
                user_input,
                recent_turns,
                &extended_scan,
                scan_depth,
                &regex_cache,
            ) {
                seen_ids.insert(id);
                boosted.push(recalled_memory_from_item(item.clone()));
            }
        }
    }

    let mut merged = boosted;
    merged.extend(recalled);
    Ok(merged)
}

/// Whether a lorebook memory row belongs in this turn's prompt: pinned
/// (constant) entries and key-matched entries pass the entry's `@@` decorator
/// gates. This recall path holds no previous-match state, so the sticky
/// decorators (`@@keep_activate_after_match` / `@@dont_activate_after_match`)
/// are inert here — the spec lets applications ignore them when previous
/// matches are unknowable; the guaranteed-injection path carries real state.
fn lorebook_include(
    item: &MemoryItem,
    book: &ene_config::Lorebook,
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    scan_text: &str,
    scan_depth: u32,
    regex_cache: &std::collections::HashMap<String, regex::Regex>,
) -> bool {
    let Some((entry, index, decorators)) = lorebook_entry_state(item, book) else {
        return item.pinned;
    };

    let entry_scan = entry_scan_text(user_input, recent_turns, scan_text, &decorators, scan_depth);
    let key_match = item.pinned
        || decorators.activate
        || entry_keys_match_with_cache(entry, index, &entry_scan, Some(regex_cache));
    let assistant_count = recent_turns
        .iter()
        .filter(|t| t.role == "assistant")
        .count() as u32;

    key_match
        && entry_decorators_accept(
            &decorators,
            entry,
            &entry_scan,
            &ActivationContext {
                assistant_message_count: assistant_count,
                ..ActivationContext::default()
            },
        )
}

/// Resolve a lorebook memory row back to its card entry and parsed decorators.
fn lorebook_entry_state<'a>(
    item: &MemoryItem,
    book: &'a ene_config::Lorebook,
) -> Option<(&'a LorebookEntry, usize, EntryDecorators)> {
    if item.source != MemorySource::Ccv3 {
        return None;
    }
    let source_ref = item.source_ref.as_deref()?;
    if !source_ref.starts_with(LOREBOOK_SOURCE_PREFIX) {
        return None;
    }
    let entry_id = source_ref.trim_start_matches(LOREBOOK_SOURCE_PREFIX);
    book.entries
        .iter()
        .enumerate()
        .find(|(idx, e)| e.enabled && stable_entry_id(e, *idx) == entry_id)
        .map(|(idx, entry)| {
            let (decorators, _) = EntryDecorators::parse(&entry.content);
            (entry, idx, decorators)
        })
}

/// Per-entry scan text: the `@@scan_depth` override rebuilds the scan window,
/// entries without the decorator share the book-level scan text.
fn entry_scan_text<'a>(
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    book_scan_text: &'a str,
    decorators: &EntryDecorators,
    book_scan_depth: u32,
) -> std::borrow::Cow<'a, str> {
    if decorators.scan_depth.is_none() {
        std::borrow::Cow::Borrowed(book_scan_text)
    } else {
        std::borrow::Cow::Owned(build_lorebook_scan_text(
            user_input,
            recent_turns,
            decorators.effective_scan_depth(book_scan_depth),
        ))
    }
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
            reflection_multiplier: 1.0,
            total: 1.0,
        },
        sources: vec![ene_core::MemoryCandidateSource::Lexical],
    }
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
    fn lorebook_include_resolves_card_entry_by_source_ref() {
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
            name: None,
            description: None,
            scan_depth: None,
            token_budget: None,
            recursive_scanning: None,
            extensions: Default::default(),
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
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        assert!(lorebook_include(
            &item,
            &book,
            "I saw a dragon",
            &[],
            "I saw a dragon",
            4,
            &regex_cache
        ));
    }

    #[test]
    fn decorator_gates_apply_in_recall_path() {
        let item = MemoryItem {
            id: Some(1),
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "Dragon lore".into(),
            content: "A dragon.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("ccv3:lorebook:gated".into()),
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
        let entry = LorebookEntry {
            keys: vec!["dragon".into()],
            content: "@@activate_only_after 3\n@@exclude_keys ancient\nA dragon.".into(),
            extensions: Default::default(),
            enabled: true,
            insertion_order: 1,
            case_sensitive: None,
            use_regex: false,
            constant: Some(false),
            name: Some("Dragon lore".into()),
            priority: None,
            id: Some(serde_json::json!("gated")),
            comment: None,
            selective: None,
            secondary_keys: None,
            position: None,
            not_keys: Vec::new(),
            sticky_turns: None,
            turns_since_match: None,
        };
        let book = ene_config::Lorebook {
            entries: vec![entry],
            ..Default::default()
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        let turns = [
            RecallTurn {
                role: "assistant",
                content: "hello",
            },
            RecallTurn {
                role: "user",
                content: "the ancient dragon flies",
            },
        ];
        // Two assistant messages: `activate_only_after 3` still gates.
        assert!(!lorebook_include(
            &item,
            &book,
            "the ancient dragon flies",
            &turns,
            "the ancient dragon flies",
            4,
            &regex_cache
        ));
        // The exclude key alone suppresses even with enough assistant turns.
        let many_turns = vec![
            RecallTurn {
                role: "assistant",
                content: "a",
            },
            RecallTurn {
                role: "user",
                content: "b",
            },
            RecallTurn {
                role: "assistant",
                content: "c",
            },
            RecallTurn {
                role: "user",
                content: "d",
            },
        ];
        assert!(!lorebook_include(
            &item,
            &book,
            "the ancient dragon flies",
            &many_turns,
            "the ancient dragon flies",
            4,
            &regex_cache
        ));
        // Without the exclude key and with enough turns, the entry passes.
        let clean_turns = vec![
            RecallTurn {
                role: "assistant",
                content: "a",
            },
            RecallTurn {
                role: "user",
                content: "b",
            },
            RecallTurn {
                role: "assistant",
                content: "c",
            },
            RecallTurn {
                role: "user",
                content: "d",
            },
            RecallTurn {
                role: "assistant",
                content: "e",
            },
            RecallTurn {
                role: "user",
                content: "the dragon roars",
            },
        ];
        assert!(lorebook_include(
            &item,
            &book,
            "the dragon roars",
            &clean_turns,
            "the dragon roars",
            4,
            &regex_cache
        ));
    }
}
