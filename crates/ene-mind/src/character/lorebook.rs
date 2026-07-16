//! `CCv3` lorebook → semantic memory compilation (#83).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ene_config::{CharacterCardV3, LorebookEntry, expand_cbs_macros};
use ene_store::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    MemoryStatus, NewMemoryItem,
};

/// Prefix for lorebook-derived `source_ref` values.
pub const LOREBOOK_SOURCE_PREFIX: &str = "ccv3:lorebook:";

/// Compiles lorebook entries into typed memory payloads.
#[derive(Debug, Default, Clone, Copy)]
pub struct LorebookIndexer;

impl LorebookIndexer {
    /// Compile enabled lorebook entries into new memory items (no DB writes).
    #[must_use]
    pub fn compile_entries(card: &CharacterCardV3, user_name: &str) -> Vec<NewMemoryItem> {
        let Some(book) = card.data.character_book.as_ref() else {
            return Vec::new();
        };

        let char_name = card.data.get_character_name();
        let character_id = card.data.get_character_name().to_string();

        book.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.enabled)
            .map(|(index, entry)| compile_entry(entry, &character_id, char_name, user_name, index))
            .collect()
    }

    /// Canonical hash of enabled lorebook entries for change detection.
    #[must_use]
    pub fn content_hash(card: &CharacterCardV3) -> u64 {
        let mut hasher = DefaultHasher::new();
        let Some(book) = card.data.character_book.as_ref() else {
            return 0;
        };
        for entry in book.entries.iter().filter(|e| e.enabled) {
            stable_entry_id(entry, 0).hash(&mut hasher);
            entry.keys.hash(&mut hasher);
            entry.content.hash(&mut hasher);
            entry.constant.hash(&mut hasher);
        }
        hasher.finish()
    }
}

fn compile_entry(
    entry: &LorebookEntry,
    character_id: &str,
    char_name: &str,
    user_name: &str,
    index: usize,
) -> NewMemoryItem {
    let content = expand_cbs_macros(entry.content.trim(), char_name, user_name);
    let trigger_line = if entry.keys.is_empty() {
        String::new()
    } else {
        format!("Triggers: {}\n\n", entry.keys.join(", "))
    };
    let title = entry
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| entry.keys.first().cloned())
        .unwrap_or_else(|| format!("Lore entry {}", entry.insertion_order));

    let priority = entry.priority.unwrap_or(entry.insertion_order);
    let salience_base = (0.5 + (priority as f32 / 200.0)).clamp(0.0, 1.0);
    let constant = entry.constant.unwrap_or(false);

    NewMemoryItem {
        scope: MemoryScope::Character,
        character_id: character_id.to_string(),
        user_id: String::new(),
        kind: MemoryKind::Semantic,
        title,
        content: format!("{trigger_line}{content}"),
        source: MemorySource::Ccv3,
        source_ref: Some(format!(
            "{LOREBOOK_SOURCE_PREFIX}{}",
            stable_entry_id(entry, index)
        )),
        confidence: MemoryConfidence::new(1.0),
        salience: MemorySalience::new(if constant { 1.0 } else { salience_base }),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: constant,
        created_at: None,
        commitment_id: None,
    }
}

/// Stable identifier for a lorebook entry across reindexes.
#[must_use]
pub fn stable_entry_id(entry: &LorebookEntry, _index: usize) -> String {
    if let Some(id) = &entry.id {
        let raw = id.to_string();
        let trimmed = raw.trim_matches('"');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let mut hasher = DefaultHasher::new();
    entry.insertion_order.hash(&mut hasher);
    entry.keys.hash(&mut hasher);
    entry.content.hash(&mut hasher);
    format!("{}:{:x}", entry.insertion_order, hasher.finish())
}

/// Match lorebook trigger keys against scan text (case-insensitive by default).
#[must_use]
pub fn entry_keys_match(entry: &LorebookEntry, scan_text: &str) -> bool {
    entry_keys_match_with_cache(entry, 0, scan_text, None)
}

/// Match lorebook trigger keys, optionally using a precompiled regex cache.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "HashMap key type is fixed to String in lorebook API"
)]
pub fn entry_keys_match_with_cache(
    entry: &LorebookEntry,
    entry_index: usize,
    scan_text: &str,
    regex_cache: Option<&std::collections::HashMap<String, regex::Regex>>,
) -> bool {
    if entry.constant.unwrap_or(false) {
        return true;
    }
    if entry.keys.is_empty() {
        return false;
    }
    let case_sensitive = entry.case_sensitive.unwrap_or(false);
    let haystack = if case_sensitive {
        scan_text.to_string()
    } else {
        scan_text.to_lowercase()
    };
    let entry_id = stable_entry_id(entry, entry_index);
    entry.keys.iter().any(|key| {
        if key.is_empty() {
            return false;
        }
        if entry.use_regex {
            let cache_key = format!("{entry_id}:{key}");
            if let Some(cache) = regex_cache
                && let Some(re) = cache.get(&cache_key)
            {
                return re.is_match(scan_text);
            }
            regex::RegexBuilder::new(key)
                .case_insensitive(!case_sensitive)
                .build()
                .is_ok_and(|re| re.is_match(scan_text))
        } else if case_sensitive {
            haystack.contains(key)
        } else {
            haystack.contains(&key.to_lowercase())
        }
    })
}

/// Precompile regex patterns for enabled lorebook entries.
#[must_use]
pub fn compile_lorebook_regex_cache(
    book: &ene_config::Lorebook,
) -> std::collections::HashMap<String, regex::Regex> {
    use std::collections::HashMap;

    let mut cache = HashMap::new();
    for (index, entry) in book.entries.iter().enumerate() {
        if !entry.enabled || !entry.use_regex {
            continue;
        }
        let entry_id = stable_entry_id(entry, index);
        let case_sensitive = entry.case_sensitive.unwrap_or(false);
        for key in &entry.keys {
            if key.is_empty() {
                continue;
            }
            let cache_key = format!("{entry_id}:{key}");
            if cache.contains_key(&cache_key) {
                continue;
            }
            match regex::RegexBuilder::new(key)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(re) => {
                    cache.insert(cache_key, re);
                }
                Err(error) => {
                    tracing::warn!(
                        component = "LorebookIndexer",
                        pattern = %key,
                        error = %error,
                        "Invalid lorebook regex pattern"
                    );
                }
            }
        }
    }
    cache
}

/// Build scan text from recent turns and the current user message.
#[must_use]
pub fn build_lorebook_scan_text(
    user_input: &str,
    recent_turns: &[crate::recall::RecallTurn<'_>],
    scan_depth: u32,
) -> String {
    let depth = scan_depth.max(1) as usize;
    let mut parts: Vec<String> = recent_turns
        .iter()
        .rev()
        .take(depth.saturating_mul(2))
        .map(|t| t.content.to_string())
        .collect();
    parts.reverse();
    parts.push(user_input.to_string());
    parts.join("\n")
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_config::Lorebook;

    fn sample_entry(keys: &[&str], content: &str, constant: bool) -> LorebookEntry {
        LorebookEntry {
            keys: keys.iter().map(|k| (*k).to_string()).collect(),
            content: content.into(),
            extensions: Default::default(),
            enabled: true,
            insertion_order: 100,
            case_sensitive: None,
            use_regex: false,
            constant: Some(constant),
            name: Some("Test Lore".into()),
            priority: None,
            id: None,
            comment: None,
            selective: None,
            secondary_keys: None,
            position: None,
        }
    }

    #[test]
    fn disabled_entries_skipped() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.character_book = Some(Lorebook {
            entries: vec![LorebookEntry {
                enabled: false,
                keys: vec!["magic".into()],
                content: "secret".into(),
                extensions: Default::default(),
                insertion_order: 1,
                use_regex: false,
                case_sensitive: None,
                constant: None,
                name: None,
                priority: None,
                id: None,
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
            }],
            ..Default::default()
        });
        assert!(LorebookIndexer::compile_entries(&card, "User").is_empty());
    }

    #[test]
    fn constant_entry_is_pinned() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.character_book = Some(Lorebook {
            entries: vec![sample_entry(&[], "Always true.", true)],
            ..Default::default()
        });
        let items = LorebookIndexer::compile_entries(&card, "User");
        assert_eq!(items.len(), 1);
        assert!(items[0].pinned);
        assert!(
            items[0]
                .source_ref
                .as_ref()
                .unwrap()
                .starts_with(LOREBOOK_SOURCE_PREFIX)
        );
    }

    #[test]
    fn key_match_is_case_insensitive_by_default() {
        let entry = sample_entry(&["Dragon"], "A dragon appears.", false);
        assert!(entry_keys_match(&entry, "I saw a dragon yesterday"));
    }
}
