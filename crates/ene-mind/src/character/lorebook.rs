//! `CCv3` lorebook → semantic memory compilation (#83).

use std::hash::{Hash, Hasher};

use ene_config::{CharacterCardV3, LorebookEntry, expand_cbs_macros};
use ene_core::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    MemoryStatus, NewMemoryItem,
};

/// Prefix for lorebook-derived `source_ref` values.
pub const LOREBOOK_SOURCE_PREFIX: &str = "ccv3:lorebook:";

/// Compiles lorebook entries into typed memory payloads.
#[derive(Debug, Default, Clone, Copy)]
pub struct LorebookIndexer;

/// Produce a stable `u64` hash from arbitrary bytes using `blake3`.
fn stable_hash_u64(data: &[u8]) -> u64 {
    let hash = blake3::hash(data);
    let bytes = hash.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// A `Hasher` that appends written bytes to a buffer for blake3 hashing.
struct StableHasher<'a>(&'a mut Vec<u8>);

impl Hasher for StableHasher<'_> {
    fn finish(&self) -> u64 {
        0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

impl LorebookIndexer {
    /// Compile enabled lorebook entries into new memory items (no DB writes).
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
    pub fn content_hash(card: &CharacterCardV3) -> u64 {
        let Some(book) = card.data.character_book.as_ref() else {
            return 0;
        };
        let mut buf = Vec::new();
        for (index, entry) in book.entries.iter().enumerate().filter(|(_, e)| e.enabled) {
            stable_entry_id(entry, index).hash(&mut StableHasher(&mut buf));
            entry.keys.hash(&mut StableHasher(&mut buf));
            entry.not_keys.hash(&mut StableHasher(&mut buf));
            entry.content.hash(&mut StableHasher(&mut buf));
            entry.constant.hash(&mut StableHasher(&mut buf));
            entry.selective.hash(&mut StableHasher(&mut buf));
            if let Some(ref sk) = entry.secondary_keys {
                sk.hash(&mut StableHasher(&mut buf));
            }
            entry.sticky_turns.hash(&mut StableHasher(&mut buf));
        }
        stable_hash_u64(&buf)
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
pub fn stable_entry_id(entry: &LorebookEntry, index: usize) -> String {
    if let Some(id) = &entry.id {
        let raw = id.to_string();
        let trimmed = raw.trim_matches('"');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let mut buf = Vec::new();
    index.hash(&mut StableHasher(&mut buf));
    entry.insertion_order.hash(&mut StableHasher(&mut buf));
    entry.keys.hash(&mut StableHasher(&mut buf));
    entry.content.hash(&mut StableHasher(&mut buf));
    format!("{}:{:x}", entry.insertion_order, stable_hash_u64(&buf))
}

/// Internal helper: check if a single key matches the scan text.
fn key_matches(
    key: &str,
    scan_text: &str,
    case_sensitive: bool,
    use_regex: bool,
    regex_cache: Option<&std::collections::HashMap<String, regex::Regex>>,
    entry_id: &str,
) -> bool {
    if key.is_empty() {
        return false;
    }
    if use_regex {
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
        scan_text.contains(key)
    } else {
        let haystack = scan_text.to_lowercase();
        haystack.contains(&key.to_lowercase())
    }
}

/// Match lorebook trigger keys against scan text (case-insensitive by default).
pub fn entry_keys_match(entry: &LorebookEntry, scan_text: &str) -> bool {
    entry_keys_match_with_cache(entry, 0, scan_text, None)
}

/// Match lorebook trigger keys, optionally using a precompiled regex cache.
///
/// Supports:
/// - **Constant entries** always match.
/// - **NOT keys** — if any NOT key matches, the entry is suppressed.
/// - **Selective mode** (`entry.selective == true`) — ALL keys must match (AND logic).
/// - **Secondary keys** — at least one primary AND at least one secondary must match.
/// - **Default** — any key match (OR logic).
///
/// When `entry.use_regex` is `true`, keys are treated as regular expressions
/// and the optional `regex_cache` is checked first for precompiled patterns.
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
    // Constant entries always match regardless of keys
    if entry.constant.unwrap_or(false) {
        return true;
    }

    // An entry with no keys and no constant flag cannot match
    if entry.keys.is_empty() && entry.not_keys.is_empty() {
        return false;
    }

    let case_sensitive = entry.case_sensitive.unwrap_or(false);
    let entry_id = stable_entry_id(entry, entry_index);

    // Check NOT keys first: if any NOT key matches, suppress entry
    for not_key in &entry.not_keys {
        if key_matches(
            not_key,
            scan_text,
            case_sensitive,
            entry.use_regex,
            regex_cache,
            &entry_id,
        ) {
            return false; // Suppressed by NOT key
        }
    }

    // If there are no positive keys to match, the entry passes (no NOT keys matched)
    if entry.keys.is_empty() {
        return true;
    }

    // Selective mode: ALL keys must match (AND logic)
    if entry.selective.unwrap_or(false) && !entry.keys.is_empty() {
        return entry.keys.iter().all(|key| {
            key_matches(
                key,
                scan_text,
                case_sensitive,
                entry.use_regex,
                regex_cache,
                &entry_id,
            )
        });
    }

    // Secondary keys mode: at least one primary AND at least one secondary must match
    if let Some(ref secondary_keys) = entry.secondary_keys
        && !secondary_keys.is_empty()
    {
        let primary_match = entry.keys.iter().any(|key| {
            key_matches(
                key,
                scan_text,
                case_sensitive,
                entry.use_regex,
                regex_cache,
                &entry_id,
            )
        });
        if !primary_match {
            return false;
        }
        let secondary_match = secondary_keys.iter().any(|key| {
            key_matches(
                key,
                scan_text,
                case_sensitive,
                entry.use_regex,
                regex_cache,
                &entry_id,
            )
        });
        return secondary_match;
    }

    // Default: any key matches (OR logic)
    entry.keys.iter().any(|key| {
        key_matches(
            key,
            scan_text,
            case_sensitive,
            entry.use_regex,
            regex_cache,
            &entry_id,
        )
    })
}

/// Precompile regex patterns for enabled lorebook entries.
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

        // Compile regexes for all key types: keys, not_keys, and secondary_keys
        let all_keys: Vec<&String> = entry
            .keys
            .iter()
            .chain(entry.not_keys.iter())
            .chain(
                entry
                    .secondary_keys
                    .as_ref()
                    .map(|v| v.iter())
                    .into_iter()
                    .flatten(),
            )
            .collect();

        for key in all_keys {
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
    clippy::indexing_slicing,
    reason = "explicit Default and fixed-index assertions for test fixture clarity"
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
            not_keys: Vec::new(),
            sticky_turns: None,
            turns_since_match: None,
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
                not_keys: Vec::new(),
                sticky_turns: None,
                turns_since_match: None,
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

    #[test]
    fn not_key_suppresses_entry() {
        let mut entry = sample_entry(&["sword"], "A shiny sword.", false);
        entry.not_keys = vec!["rusty".to_string()];
        // NOT key "rusty" matches the scan text, so the entry is suppressed
        assert!(!entry_keys_match(&entry, "There is a rusty sword here"));
        // Without the NOT key, it would match
        assert!(entry_keys_match(&entry, "There is a shiny sword here"));
    }

    #[test]
    fn selective_and_logic() {
        let mut entry = sample_entry(&["forest", "dark"], "You enter a dark forest.", false);
        entry.selective = Some(true);
        // Both keys must match
        assert!(entry_keys_match(&entry, "The dark forest is ahead"));
        // Only one key matches
        assert!(!entry_keys_match(&entry, "The forest is bright"));
        assert!(!entry_keys_match(&entry, "It is dark outside"));
    }

    #[test]
    fn secondary_keys_require_primary_and_secondary() {
        let mut entry = sample_entry(&["castle"], "Castle lore.", false);
        entry.secondary_keys = Some(vec!["king".to_string(), "queen".to_string()]);
        // Primary + secondary match
        assert!(entry_keys_match(&entry, "The king lives in the castle"));
        // Only primary matches
        assert!(!entry_keys_match(&entry, "The castle is empty"));
        // Only secondary matches
        assert!(!entry_keys_match(&entry, "The king is away"));
    }

    #[test]
    fn not_keys_checked_before_positive_match() {
        let mut entry = sample_entry(&["magic"], "Magic lore.", false);
        entry.not_keys = vec!["anti-magic".to_string()];
        // The scan text contains both "magic" and "anti-magic" - NOT key wins
        assert!(!entry_keys_match(
            &entry,
            "anti-magic field suppresses magic"
        ));
    }
}
