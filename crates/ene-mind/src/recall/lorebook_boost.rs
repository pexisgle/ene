//! Lorebook recall boosts for constant and key-triggered entries.

use std::borrow::Cow;
use std::collections::HashMap;

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
///
/// `assistant_message_count` is the total assistant-message count of the chat
/// log (derived from the full history by the caller) — the spec counts the
/// whole log for `@@activate_only_after` / `@@activate_only_every`, never the
/// recent-turn window.
pub async fn merge_lorebook_recall(
    store: &dyn MemoryPort,
    character_id: &str,
    card: Option<&CharacterCardV3>,
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    assistant_message_count: u32,
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
    let decorators_by_id = compile_entry_decorators(book);

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
            assistant_message_count,
            &scan_text,
            "",
            scan_depth,
            &regex_cache,
            &decorators_by_id,
        ) {
            seen_ids.insert(id);
            boosted.push(recalled_memory_from_item(item.clone()));
        }
    }

    if book.recursive_scanning.unwrap_or(false) && !boosted.is_empty() {
        let recursion_content = boosted
            .iter()
            .map(|m| m.item.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut extended_scan = scan_text;
        extended_scan.push('\n');
        extended_scan.push_str(&recursion_content);
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
                assistant_message_count,
                &extended_scan,
                &recursion_content,
                scan_depth,
                &regex_cache,
                &decorators_by_id,
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
///
/// `recursion_content` carries the other entries' contents when the book
/// enables `recursive_scanning`; it is appended to the per-entry scan window so
/// an `@@scan_depth` override cannot defeat recursive matching (spec:
/// "regardless of `scan_depth`").
fn lorebook_include(
    item: &MemoryItem,
    book: &ene_config::Lorebook,
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    assistant_message_count: u32,
    scan_text: &str,
    recursion_content: &str,
    scan_depth: u32,
    regex_cache: &HashMap<String, regex::Regex>,
    decorators_by_id: &HashMap<String, (usize, EntryDecorators)>,
) -> bool {
    let Some((entry, index, decorators)) = lorebook_entry_state(item, book, decorators_by_id)
    else {
        return item.pinned;
    };

    let entry_scan = entry_scan_text(
        user_input,
        recent_turns,
        scan_text,
        decorators,
        scan_depth,
        recursion_content,
    );
    let key_match = item.pinned
        || decorators.activate
        || entry_keys_match_with_cache(entry, index, &entry_scan, Some(regex_cache));

    key_match
        && entry_decorators_accept(
            decorators,
            entry,
            index,
            &entry_scan,
            Some(regex_cache),
            &ActivationContext {
                assistant_message_count,
                ..ActivationContext::default()
            },
        )
}

/// Pre-parse every enabled entry's decorators once per merge; the memory rows
/// are looked up by their stable id instead of re-parsing per row per turn.
fn compile_entry_decorators(
    book: &ene_config::Lorebook,
) -> HashMap<String, (usize, EntryDecorators)> {
    book.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.enabled)
        .map(|(index, entry)| {
            let (decorators, _) = EntryDecorators::parse(&entry.content);
            (stable_entry_id(entry, index), (index, decorators))
        })
        .collect()
}

/// Resolve a lorebook memory row back to its card entry and parsed decorators.
fn lorebook_entry_state<'a>(
    item: &MemoryItem,
    book: &'a ene_config::Lorebook,
    decorators_by_id: &'a HashMap<String, (usize, EntryDecorators)>,
) -> Option<(&'a LorebookEntry, usize, &'a EntryDecorators)> {
    if item.source != MemorySource::Ccv3 {
        return None;
    }
    let source_ref = item.source_ref.as_deref()?;
    if !source_ref.starts_with(LOREBOOK_SOURCE_PREFIX) {
        return None;
    }
    let entry_id = source_ref.trim_start_matches(LOREBOOK_SOURCE_PREFIX);
    let (index, decorators) = decorators_by_id.get(entry_id)?;
    book.entries
        .get(*index)
        .map(|entry| (entry, *index, decorators))
}

/// Per-entry scan text: the `@@scan_depth` override rebuilds the scan window
/// (with the recursive-scan contents appended), entries without the decorator
/// share the book-level scan text.
fn entry_scan_text<'a>(
    user_input: &str,
    recent_turns: &[RecallTurn<'_>],
    book_scan_text: &'a str,
    decorators: &EntryDecorators,
    book_scan_depth: u32,
    recursion_content: &str,
) -> Cow<'a, str> {
    if decorators.scan_depth.is_none() {
        return Cow::Borrowed(book_scan_text);
    }
    let mut rebuilt = build_lorebook_scan_text(
        user_input,
        recent_turns,
        decorators.effective_scan_depth(book_scan_depth),
    );
    if !recursion_content.is_empty() {
        rebuilt.push('\n');
        rebuilt.push_str(recursion_content);
    }
    Cow::Owned(rebuilt)
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
            }],
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        let decorators_by_id = compile_entry_decorators(&book);
        assert!(lorebook_include(
            &item,
            &book,
            "I saw a dragon",
            &[],
            0,
            "I saw a dragon",
            "",
            4,
            &regex_cache,
            &decorators_by_id
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
        };
        let book = ene_config::Lorebook {
            entries: vec![entry],
            ..Default::default()
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        let decorators_by_id = compile_entry_decorators(&book);
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
        // The chat-log assistant count (2) still gates `activate_only_after 3`,
        // even though the window also holds only two assistant messages.
        assert!(!lorebook_include(
            &item,
            &book,
            "the ancient dragon flies",
            &turns,
            2,
            "the ancient dragon flies",
            "",
            4,
            &regex_cache,
            &decorators_by_id
        ));
        // The count comes from the full chat log, not the window: a window
        // with no assistant messages cannot gate an entry that already passed
        // the threshold in the log.
        assert!(lorebook_include(
            &item,
            &book,
            "the dragon flies",
            &turns,
            5,
            "the dragon flies",
            "",
            4,
            &regex_cache,
            &decorators_by_id
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
            5,
            "the ancient dragon flies",
            "",
            4,
            &regex_cache,
            &decorators_by_id
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
            5,
            "the dragon roars",
            "",
            4,
            &regex_cache,
            &decorators_by_id
        ));
    }

    #[test]
    fn scan_depth_entry_matches_recursive_content() {
        let item = |id: i64, source_ref: &str| MemoryItem {
            id: Some(id),
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: String::new(),
            content: String::new(),
            source: MemorySource::Ccv3,
            source_ref: Some(source_ref.into()),
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
            recursive_scanning: Some(true),
            extensions: Default::default(),
            entries: vec![
                LorebookEntry {
                    keys: vec!["dragon".into()],
                    content: "The dragon's hoard holds the cursed ring.".into(),
                    extensions: Default::default(),
                    enabled: true,
                    insertion_order: 1,
                    case_sensitive: None,
                    use_regex: false,
                    constant: Some(false),
                    name: None,
                    priority: None,
                    id: Some(serde_json::json!("dragon")),
                    comment: None,
                    selective: None,
                    secondary_keys: None,
                    position: None,
                },
                LorebookEntry {
                    keys: vec!["cursed".into()],
                    content: "@@scan_depth 1\nThe ring brings misfortune.".into(),
                    extensions: Default::default(),
                    enabled: true,
                    insertion_order: 2,
                    case_sensitive: None,
                    use_regex: false,
                    constant: Some(false),
                    name: None,
                    priority: None,
                    id: Some(serde_json::json!("ring")),
                    comment: None,
                    selective: None,
                    secondary_keys: None,
                    position: None,
                },
            ],
        };
        let regex_cache = compile_lorebook_regex_cache(&book);
        let decorators_by_id = compile_entry_decorators(&book);
        // The `@@scan_depth 1` entry matches only because the recursion appends
        // the dragon entry's content ("...cursed ring.") to its rebuilt window.
        assert!(lorebook_include(
            &item(1, "ccv3:lorebook:dragon"),
            &book,
            "the dragon flies",
            &[],
            0,
            "the dragon flies",
            "The dragon's hoard holds the cursed ring.",
            4,
            &regex_cache,
            &decorators_by_id
        ));
        let mut truncated_scan = build_lorebook_scan_text("the dragon flies", &[], 1);
        truncated_scan.push('\n');
        truncated_scan.push_str("The dragon's hoard holds the cursed ring.");
        assert!(lorebook_include(
            &item(2, "ccv3:lorebook:ring"),
            &book,
            "the dragon flies",
            &[],
            0,
            &truncated_scan,
            "The dragon's hoard holds the cursed ring.",
            4,
            &regex_cache,
            &decorators_by_id
        ));
    }
}
