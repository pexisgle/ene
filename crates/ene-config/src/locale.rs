//! Localized character card diffs and their merge into a base card.
//!
//! A `character.{lang}.json` sidecar (or the embedded
//! `extensions.ene.locales` bag used by PNG distribution) carries only the
//! translatable subset of a `CCv3` card. [`merge_localized_fields`] overlays
//! such a diff onto the base card: every `Some` field replaces the base
//! value, every `None` keeps it, so untranslated fields fall back to the
//! base language. Lorebook entries are matched by `id` and only their `keys`
//! and `content` are replaced — `keys` must be translated because they are
//! matched against conversation text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::CharacterCardV3;

/// Translatable subset of a `CCv3` card, layered over `character.json`.
///
/// Language-independent data (`assets`, motion/expression definitions,
/// timestamps) is intentionally absent so a diff cannot drift from the base
/// card. `name` is also absent: it is the character's identity key used for
/// discovery and folder naming, so only the display-only `nickname` is
/// translatable.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedCharacterFields {
    /// Localized `data.description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Localized `data.personality`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// Localized `data.scenario`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    /// Localized `data.first_mes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_mes: Option<String>,
    /// Localized `data.alternate_greetings` (full list replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_greetings: Option<Vec<String>>,
    /// Localized `data.mes_example`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mes_example: Option<String>,
    /// Localized `data.system_prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Localized `data.post_history_instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_history_instructions: Option<String>,
    /// Localized `data.creator_notes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_notes: Option<String>,
    /// Localized display name (`data.nickname`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    /// Localized `data.tags` (full list replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Lorebook entry translations, matched against the base by entry `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_book: Option<LocalizedLorebook>,
}

/// Lorebook translations: only `keys` and `content` of existing entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedLorebook {
    /// Translated entries; each must reference a base entry `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<LocalizedLorebookEntry>,
}

/// A single translated lorebook entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedLorebookEntry {
    /// The base entry's `id`; entries without a matching base id are skipped
    /// with a warning instead of being appended.
    pub id: serde_json::Value,
    /// Translated trigger keys (matched against conversation text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,
    /// Translated entry content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Overlays `diff` onto `card`; `None` fields keep the base value.
pub(crate) fn merge_localized_fields(card: &mut CharacterCardV3, diff: &LocalizedCharacterFields) {
    let data = &mut card.data;
    if let Some(description) = &diff.description {
        data.description.clone_from(description);
    }
    if let Some(personality) = &diff.personality {
        data.personality.clone_from(personality);
    }
    if let Some(scenario) = &diff.scenario {
        data.scenario.clone_from(scenario);
    }
    if let Some(first_mes) = &diff.first_mes {
        data.first_mes.clone_from(first_mes);
    }
    if let Some(alternate_greetings) = &diff.alternate_greetings {
        data.alternate_greetings.clone_from(alternate_greetings);
    }
    if let Some(mes_example) = &diff.mes_example {
        data.mes_example.clone_from(mes_example);
    }
    if let Some(system_prompt) = &diff.system_prompt {
        data.system_prompt.clone_from(system_prompt);
    }
    if let Some(post_history_instructions) = &diff.post_history_instructions {
        data.post_history_instructions
            .clone_from(post_history_instructions);
    }
    if let Some(creator_notes) = &diff.creator_notes {
        data.creator_notes.clone_from(creator_notes);
    }
    if let Some(nickname) = &diff.nickname {
        data.nickname.clone_from(nickname);
    }
    if let Some(tags) = &diff.tags {
        data.tags.clone_from(tags);
    }
    if let Some(localized_book) = &diff.character_book {
        let Some(book) = data.character_book.as_mut() else {
            tracing::warn!(
                "Localized card diff has character_book entries but the base card has none; skipping them"
            );
            return;
        };
        for entry in &localized_book.entries {
            let Some(base_entry) = book
                .entries
                .iter_mut()
                .find(|base_entry| base_entry.id.as_ref() == Some(&entry.id))
            else {
                tracing::warn!(
                    id = %entry.id,
                    "Localized card diff lorebook entry has no matching base entry; skipping it"
                );
                continue;
            };
            if let Some(keys) = &entry.keys {
                base_entry.keys.clone_from(keys);
            }
            if let Some(content) = &entry.content {
                base_entry.content.clone_from(content);
            }
        }
    }
}

/// Removes the embedded locale bag after merging.
///
/// Every load path normalizes to the same in-memory shape: a merged
/// single-language card without the locale bag, so a PNG-loaded card and a
/// folder-loaded card produce identical bytes on save and identical memory
/// hashes. The base-language loader does not strip — import materializes the
/// bag to sidecar files instead.
pub(crate) fn strip_locales(card: &mut CharacterCardV3) {
    let Some(ene) = card.data.extensions.ene.as_mut() else {
        return;
    };
    if ene.locales.take().is_none() {
        return;
    }
    if ene.is_empty() {
        card.data.extensions.ene = None;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{EneExtension, LorebookEntry};

    fn base_card() -> CharacterCardV3 {
        CharacterCardV3 {
            data: crate::CharacterCardData {
                name: "Ada".to_string(),
                description: "Base description".to_string(),
                personality: "Base personality".to_string(),
                first_mes: "Hello!".to_string(),
                nickname: "Ada".to_string(),
                tags: vec!["engineer".to_string()],
                character_book: Some(crate::Lorebook {
                    entries: vec![lore_entry(
                        json!("lore-1"),
                        vec!["cat".to_string(), "kitty".to_string()],
                        "Base lore",
                    )],
                    ..crate::Lorebook::default()
                }),
                ..crate::CharacterCardData::default()
            },
            ..CharacterCardV3::default()
        }
    }

    fn lore_entry(id: serde_json::Value, keys: Vec<String>, content: &str) -> LorebookEntry {
        LorebookEntry {
            keys,
            content: content.to_string(),
            extensions: Default::default(),
            enabled: true,
            insertion_order: 0,
            case_sensitive: None,
            use_regex: false,
            constant: None,
            name: None,
            priority: None,
            id: Some(id),
            comment: None,
            selective: None,
            secondary_keys: None,
            position: None,
        }
    }

    #[test]
    fn merge_replaces_present_fields_and_keeps_absent_ones() {
        let mut card = base_card();
        let diff = LocalizedCharacterFields {
            description: Some("日本語の説明".to_string()),
            first_mes: Some("やっほー！".to_string()),
            nickname: Some("エイダ".to_string()),
            tags: Some(vec!["エンジニア".to_string()]),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        assert_eq!(card.data.description, "日本語の説明");
        assert_eq!(card.data.first_mes, "やっほー！");
        assert_eq!(card.data.nickname, "エイダ");
        assert_eq!(card.data.tags, ["エンジニア"]);
        assert_eq!(card.data.personality, "Base personality");
    }

    #[test]
    fn merge_switches_lorebook_keys_and_content_by_id() {
        let mut card = base_card();
        let diff = LocalizedCharacterFields {
            character_book: Some(LocalizedLorebook {
                entries: vec![LocalizedLorebookEntry {
                    id: json!("lore-1"),
                    keys: Some(vec!["猫".to_string(), "ねこ".to_string()]),
                    content: Some("日本語のロア".to_string()),
                }],
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let entry = &card.data.character_book.expect("book present").entries[0];
        assert_eq!(entry.keys, ["猫", "ねこ"]);
        assert_eq!(entry.content, "日本語のロア");
    }

    #[test]
    fn merge_skips_unmatched_lorebook_ids() {
        let mut card = base_card();
        let diff = LocalizedCharacterFields {
            character_book: Some(LocalizedLorebook {
                entries: vec![LocalizedLorebookEntry {
                    id: json!("lore-missing"),
                    keys: Some(vec!["猫".to_string()]),
                    content: Some("無視される".to_string()),
                }],
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let entry = &card.data.character_book.expect("book present").entries[0];
        assert_eq!(entry.keys, ["cat", "kitty"]);
        assert_eq!(entry.content, "Base lore");
    }

    #[test]
    fn merge_without_base_book_keeps_card_intact() {
        let mut card = base_card();
        card.data.character_book = None;
        let diff = LocalizedCharacterFields {
            character_book: Some(LocalizedLorebook {
                entries: vec![LocalizedLorebookEntry {
                    id: json!("lore-1"),
                    keys: Some(vec!["猫".to_string()]),
                    content: None,
                }],
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        assert!(card.data.character_book.is_none());
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn strip_locales_collapses_a_locales_only_extension_block() {
        let mut card = base_card();
        card.data.extensions.ene = Some(EneExtension {
            locales: Some(
                [(
                    "ja".to_string(),
                    LocalizedCharacterFields {
                        description: Some("日本語の説明".to_string()),
                        ..LocalizedCharacterFields::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            ..EneExtension::default()
        });

        strip_locales(&mut card);

        assert!(card.data.extensions.ene.is_none());
    }

    #[test]
    fn strip_locales_keeps_other_extension_fields() {
        let mut card = base_card();
        card.data.extensions.ene = Some(EneExtension {
            locales: Some(Default::default()),
            affect_baseline: Some(Default::default()),
            ..EneExtension::default()
        });

        strip_locales(&mut card);

        let ene = card.data.extensions.ene.expect("ene kept");
        assert!(ene.locales.is_none());
        assert!(ene.affect_baseline.is_some());
    }
}
