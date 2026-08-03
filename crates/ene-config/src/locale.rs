//! Localized character card diffs and their merge into a base card.
//!
//! A `character.{lang}.json` sidecar (or the embedded
//! `extensions.ene.locales` bag used by PNG distribution) carries only the
//! translatable subset of a `CCv3` card. [`merge_localized_fields`] overlays
//! such a diff onto the base card: every `Some` field replaces the base
//! value, every `None` keeps it, so untranslated fields fall back to the
//! base language. Lorebook entries are matched by `id` and only their `keys`
//! / `secondary_keys` / `content` are replaced — triggers must be translated
//! because they are matched against conversation text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CharacterCardV3, TimePeriod};

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
    /// Localized roleplay fields under `extensions.ene`.
    ///
    /// Like `character_book`, every entry references an existing base block
    /// (`speech` / `ng_expressions` / `style_examples` /
    /// `relationship_stages` / `time_periods` / `scene_behaviors`); diffs
    /// that reference an absent block are skipped with a warning so a card
    /// cannot gain locale-only structure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<LocalizedEneRoleplay>,
}

/// Localized roleplay fields under `extensions.ene` in a card diff.
///
/// Only natural-language text is replaceable: enum selects, numeric
/// thresholds, and matching keys (`id`, `name`) stay tied to the base card.
/// List fields are either full replacements (`ng_expressions`) or
/// entry-matched overlays (`style_examples`, `relationship_stages`,
/// `time_periods`, `scene_behaviors`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedEneRoleplay {
    /// Localized speech-style text fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech: Option<LocalizedSpeechStyle>,
    /// Localized NG expressions (full list replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ng_expressions: Option<Vec<String>>,
    /// Localized labeled style examples, matched against the base by `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_examples: Option<Vec<LocalizedStyleExample>>,
    /// Localized relationship-stage labels/tones, matched by `threshold`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship_stages: Option<Vec<LocalizedRelationshipStage>>,
    /// Localized time-period behaviors, matched by `period`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_periods: Option<Vec<LocalizedTimePeriodBehavior>>,
    /// Localized scene behaviors, matched against the base by `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_behaviors: Option<Vec<LocalizedSceneBehavior>>,
}

/// Localized text fields of `extensions.ene.speech`.
///
/// `length` and `politeness` are enum selects, not language text, so they
/// are intentionally absent (mirroring the non-translated relationship
/// thresholds).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedSpeechStyle {
    /// Localized first-person pronoun.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_person: Option<String>,
    /// Localized second-person address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_person: Option<String>,
    /// Localized verbal tics (full list replacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbal_tics: Option<Vec<String>>,
}

/// Localized label/text of one labeled style example.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedStyleExample {
    /// The base example's `id`.
    pub id: String,
    /// Localized situation label (matched against user input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Localized example text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Localized label/tone of one relationship stage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedRelationshipStage {
    /// The base stage's threshold (the non-translated matching key).
    pub threshold: f32,
    /// Localized stage name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Localized tone instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

/// Localized behavior of one time period.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedTimePeriodBehavior {
    /// The base behavior's period (the non-translated matching key).
    pub period: TimePeriod,
    /// Localized behavior instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
}

/// Localized keywords/behavior of one scene behavior.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", deny_unknown_fields)]
#[schemars(crate = "crate::schemars")]
pub struct LocalizedSceneBehavior {
    /// The base behavior's `name` (the non-translated matching key).
    pub name: String,
    /// Localized scene keywords (matched against localized scene text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// Localized behavior instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
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
    /// Translated secondary trigger keys; Ene's matcher requires at least
    /// one primary AND one secondary key to fire, so untranslated
    /// `secondary_keys` would leave the entry dead in every other language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_keys: Option<Vec<String>>,
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
    if let (Some(localized_book), Some(book)) = (&diff.character_book, data.character_book.as_mut())
    {
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
            if let Some(secondary_keys) = &entry.secondary_keys {
                base_entry.secondary_keys = Some(secondary_keys.clone());
            }
            if let Some(content) = &entry.content {
                base_entry.content.clone_from(content);
            }
        }
    } else if diff.character_book.is_some() {
        tracing::warn!(
            "Localized card diff has character_book entries but the base card has none; skipping them"
        );
    }
    if let Some(localized) = &diff.extensions {
        merge_localized_extensions(&mut data.extensions, localized);
    }
}

/// Overlays localized roleplay fields onto `data.extensions`; `None` fields
/// keep the base value.
///
/// Blocks are matched against the base like lorebook entries: a diff that
/// references an absent block is skipped with a warning so locale diffs can
/// never add structure the base card does not have. `speech` and
/// `ng_expressions` do not need matching (they overlay the whole block).
fn merge_localized_extensions(
    extensions: &mut crate::character_card::Extensions,
    diff: &LocalizedEneRoleplay,
) {
    if let Some(speech) = &diff.speech {
        if let Some(ene) = extensions.ene.as_mut() {
            if let Some(base_speech) = ene.speech.as_mut() {
                if let Some(first_person) = &speech.first_person {
                    base_speech.first_person = Some(first_person.clone());
                }
                if let Some(second_person) = &speech.second_person {
                    base_speech.second_person = Some(second_person.clone());
                }
                if let Some(verbal_tics) = &speech.verbal_tics {
                    base_speech.verbal_tics.clone_from(verbal_tics);
                }
            } else {
                tracing::warn!(
                    "Localized card diff has extensions.ene.speech but the base card has no speech block; skipping it"
                );
            }
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.speech but the base card has no ene block; skipping it"
            );
        }
    }
    if let Some(ng_expressions) = &diff.ng_expressions {
        if let Some(ene) = extensions.ene.as_mut() {
            ene.ng_expressions = Some(ng_expressions.clone());
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.ng_expressions but the base card has none; skipping it"
            );
        }
    }
    if let Some(examples) = &diff.style_examples {
        if let Some(ene) = extensions.ene.as_mut() {
            if let Some(base_examples) = ene.style_examples.as_mut() {
                for example in examples {
                    let Some(base_example) =
                        base_examples.iter_mut().find(|base| base.id == example.id)
                    else {
                        tracing::warn!(
                            id = %example.id,
                            "Localized style example has no matching base id; skipping it"
                        );
                        continue;
                    };
                    if let Some(label) = &example.label {
                        base_example.label.clone_from(label);
                    }
                    if let Some(text) = &example.text {
                        base_example.text.clone_from(text);
                    }
                }
            } else {
                tracing::warn!(
                    "Localized card diff has extensions.ene.style_examples but the base card has no style examples; skipping it"
                );
            }
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.style_examples but the base card has none; skipping it"
            );
        }
    }
    if let Some(stages) = &diff.relationship_stages {
        if let Some(ene) = extensions.ene.as_mut() {
            if let Some(base_stages) = ene.relationship_stages.as_mut() {
                for stage in stages {
                    let Some(base_stage) = base_stages
                        .iter_mut()
                        .find(|base| base.threshold == stage.threshold)
                    else {
                        tracing::warn!(
                            threshold = stage.threshold,
                            "Localized relationship stage has no matching base threshold; skipping it"
                        );
                        continue;
                    };
                    if let Some(label) = &stage.label {
                        base_stage.label.clone_from(label);
                    }
                    if let Some(tone) = &stage.tone {
                        base_stage.tone.clone_from(tone);
                    }
                }
            } else {
                tracing::warn!(
                    "Localized card diff has extensions.ene.relationship_stages but the base card has no relationship stages; skipping it"
                );
            }
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.relationship_stages but the base card has none; skipping it"
            );
        }
    }
    if let Some(periods) = &diff.time_periods {
        if let Some(ene) = extensions.ene.as_mut() {
            if let Some(base_periods) = ene.time_periods.as_mut() {
                for period in periods {
                    let Some(base_period) = base_periods
                        .iter_mut()
                        .find(|base| base.period == period.period)
                    else {
                        tracing::warn!(
                            period = ?period.period,
                            "Localized time period has no matching base period; skipping it"
                        );
                        continue;
                    };
                    if let Some(behavior) = &period.behavior {
                        base_period.behavior.clone_from(behavior);
                    }
                }
            } else {
                tracing::warn!(
                    "Localized card diff has extensions.ene.time_periods but the base card has no time periods; skipping it"
                );
            }
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.time_periods but the base card has none; skipping it"
            );
        }
    }
    if let Some(scenes) = &diff.scene_behaviors {
        if let Some(ene) = extensions.ene.as_mut() {
            if let Some(base_scenes) = ene.scene_behaviors.as_mut() {
                for scene in scenes {
                    let Some(base_scene) =
                        base_scenes.iter_mut().find(|base| base.name == scene.name)
                    else {
                        tracing::warn!(
                            name = %scene.name,
                            "Localized scene behavior has no matching base name; skipping it"
                        );
                        continue;
                    };
                    if let Some(keywords) = &scene.keywords {
                        base_scene.keywords.clone_from(keywords);
                    }
                    if let Some(behavior) = &scene.behavior {
                        base_scene.behavior.clone_from(behavior);
                    }
                }
            } else {
                tracing::warn!(
                    "Localized card diff has extensions.ene.scene_behaviors but the base card has no scene behaviors; skipping it"
                );
            }
        } else {
            tracing::warn!(
                "Localized card diff has extensions.ene.scene_behaviors but the base card has none; skipping it"
            );
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
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{AffectBaseline, EneExtension, LorebookEntry};

    fn base_card() -> CharacterCardV3 {
        CharacterCardV3 {
            data: crate::CharacterCardData {
                name: "Ada".to_string(),
                description: "Base description".to_string(),
                personality: "Base personality".to_string(),
                first_mes: "Hello!".to_string(),
                alternate_greetings: vec!["Hi".to_string()],
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
            extensions: HashMap::default(),
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
            alternate_greetings: Some(vec!["こんにちは".to_string()]),
            nickname: Some("エイダ".to_string()),
            tags: Some(vec!["エンジニア".to_string()]),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        assert_eq!(card.data.description, "日本語の説明");
        assert_eq!(card.data.first_mes, "やっほー！");
        assert_eq!(card.data.alternate_greetings, ["こんにちは"]);
        assert_eq!(card.data.nickname, "エイダ");
        assert_eq!(card.data.tags, ["エンジニア"]);
        assert_eq!(card.data.personality, "Base personality");
    }

    #[test]
    fn merge_switches_lorebook_secondary_keys_by_id() {
        let mut card = base_card();
        card.data
            .character_book
            .as_mut()
            .expect("book present")
            .entries[0]
            .secondary_keys = Some(vec!["pet".to_string()]);
        let diff = LocalizedCharacterFields {
            character_book: Some(LocalizedLorebook {
                entries: vec![LocalizedLorebookEntry {
                    id: json!("lore-1"),
                    keys: None,
                    secondary_keys: Some(vec!["ペット".to_string()]),
                    content: None,
                }],
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let entry = &card.data.character_book.expect("book present").entries[0];
        assert_eq!(entry.secondary_keys, Some(vec!["ペット".to_string()]));
        assert_eq!(
            entry.keys,
            ["cat", "kitty"],
            "absent keys keep the base value"
        );
        assert_eq!(entry.content, "Base lore");
    }

    #[test]
    fn merge_switches_lorebook_keys_and_content_by_id() {
        let mut card = base_card();
        let diff = LocalizedCharacterFields {
            character_book: Some(LocalizedLorebook {
                entries: vec![LocalizedLorebookEntry {
                    id: json!("lore-1"),
                    keys: Some(vec!["猫".to_string(), "ねこ".to_string()]),
                    secondary_keys: None,
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
                    secondary_keys: None,
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
                    secondary_keys: None,
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
            locales: Some(indexmap::IndexMap::default()),
            affect_baseline: Some(AffectBaseline::default()),
            ..EneExtension::default()
        });

        strip_locales(&mut card);

        let ene = card.data.extensions.ene.expect("ene kept");
        assert!(ene.locales.is_none());
        assert!(ene.affect_baseline.is_some());
    }

    fn roleplay_card() -> CharacterCardV3 {
        let mut card = base_card();
        card.data.extensions.ene = Some(EneExtension {
            speech: Some(crate::SpeechStyleDefinition {
                first_person: Some("私".into()),
                second_person: Some("きみ".into()),
                verbal_tics: vec!["だよね".into()],
                ..crate::SpeechStyleDefinition::default()
            }),
            ng_expressions: Some(vec!["死ね".into()]),
            style_examples: Some(vec![crate::LabeledStyleExample {
                id: "angry-1".into(),
                label: "怒っているとき".into(),
                text: "Base angry text".into(),
            }]),
            relationship_stages: Some(vec![crate::RelationshipStage {
                threshold: 0.3,
                label: "close friend".into(),
                tone: "Base warm tone".into(),
            }]),
            time_periods: Some(vec![crate::TimePeriodBehavior {
                period: crate::TimePeriod::Night,
                behavior: "Base night behavior".into(),
            }]),
            scene_behaviors: Some(vec![crate::SceneBehavior {
                name: "working".into(),
                keywords: vec!["作業".into()],
                behavior: "Base work behavior".into(),
            }]),
            ..EneExtension::default()
        });
        card
    }

    #[test]
    fn merge_overlays_roleplay_text_fields() {
        let mut card = roleplay_card();
        let diff = LocalizedCharacterFields {
            extensions: Some(LocalizedEneRoleplay {
                speech: Some(LocalizedSpeechStyle {
                    first_person: Some("わたし".into()),
                    verbal_tics: Some(vec!["ですわ".into()]),
                    ..LocalizedSpeechStyle::default()
                }),
                ng_expressions: Some(vec!["むり".into(), "やだ".into()]),
                ..LocalizedEneRoleplay::default()
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let ene = card.data.extensions.ene.expect("ene present");
        let speech = ene.speech.expect("speech present");
        assert_eq!(speech.first_person.as_deref(), Some("わたし"));
        assert_eq!(
            speech.second_person.as_deref(),
            Some("きみ"),
            "absent diff field keeps the base value"
        );
        assert_eq!(speech.verbal_tics, ["ですわ"]);
        assert_eq!(
            ene.ng_expressions,
            Some(vec!["むり".to_string(), "やだ".to_string()])
        );
    }

    #[test]
    fn merge_matches_roleplay_entries_by_key() {
        let mut card = roleplay_card();
        let diff = LocalizedCharacterFields {
            extensions: Some(LocalizedEneRoleplay {
                style_examples: Some(vec![LocalizedStyleExample {
                    id: "angry-1".into(),
                    label: Some("angry".into()),
                    text: Some("Localized angry text".into()),
                }]),
                relationship_stages: Some(vec![LocalizedRelationshipStage {
                    threshold: 0.3,
                    label: Some("親友".into()),
                    tone: Some("親しみのある口調".into()),
                }]),
                time_periods: Some(vec![LocalizedTimePeriodBehavior {
                    period: crate::TimePeriod::Night,
                    behavior: Some("夜は小声で".into()),
                }]),
                scene_behaviors: Some(vec![LocalizedSceneBehavior {
                    name: "working".into(),
                    keywords: Some(vec!["work".into()]),
                    behavior: Some("Keep replies short".into()),
                }]),
                ..LocalizedEneRoleplay::default()
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let ene = card.data.extensions.ene.expect("ene present");
        let example = &ene.style_examples.expect("examples present")[0];
        assert_eq!(example.label, "angry");
        assert_eq!(example.text, "Localized angry text");
        let stage = &ene.relationship_stages.expect("stages present")[0];
        assert!((stage.threshold - 0.3).abs() < f32::EPSILON);
        assert_eq!(stage.label, "親友");
        assert_eq!(stage.tone, "親しみのある口調");
        let period = &ene.time_periods.expect("periods present")[0];
        assert_eq!(period.period, crate::TimePeriod::Night);
        assert_eq!(period.behavior, "夜は小声で");
        let scene = &ene.scene_behaviors.expect("scenes present")[0];
        assert_eq!(scene.keywords, ["work"]);
        assert_eq!(scene.behavior, "Keep replies short");
    }

    #[test]
    fn merge_skips_roleplay_entries_without_a_base_match() {
        let mut card = roleplay_card();
        let diff = LocalizedCharacterFields {
            extensions: Some(LocalizedEneRoleplay {
                style_examples: Some(vec![LocalizedStyleExample {
                    id: "unknown-id".into(),
                    label: Some("angry".into()),
                    text: Some("must not land".into()),
                }]),
                relationship_stages: Some(vec![LocalizedRelationshipStage {
                    threshold: 0.9,
                    label: Some("best friend".into()),
                    tone: Some("must not land".into()),
                }]),
                time_periods: Some(vec![LocalizedTimePeriodBehavior {
                    period: crate::TimePeriod::Morning,
                    behavior: Some("must not land".into()),
                }]),
                scene_behaviors: Some(vec![LocalizedSceneBehavior {
                    name: "unknown-scene".into(),
                    keywords: Some(vec!["x".into()]),
                    behavior: Some("must not land".into()),
                }]),
                ..LocalizedEneRoleplay::default()
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        let ene = card.data.extensions.ene.expect("ene present");
        let example = &ene.style_examples.expect("examples present")[0];
        assert_eq!(
            example.label, "怒っているとき",
            "unmatched diff must not land"
        );
        assert_eq!(example.text, "Base angry text");
        let stage = &ene.relationship_stages.expect("stages present")[0];
        assert_eq!(stage.label, "close friend");
        let period = &ene.time_periods.expect("periods present")[0];
        assert_eq!(period.behavior, "Base night behavior");
        let scene = &ene.scene_behaviors.expect("scenes present")[0];
        assert_eq!(scene.keywords, ["作業"]);
        assert_eq!(scene.behavior, "Base work behavior");
    }

    #[test]
    fn merge_skips_roleplay_diff_without_a_base_extension_block() {
        let mut card = base_card();
        let diff = LocalizedCharacterFields {
            extensions: Some(LocalizedEneRoleplay {
                speech: Some(LocalizedSpeechStyle {
                    first_person: Some("わたし".into()),
                    ..LocalizedSpeechStyle::default()
                }),
                ng_expressions: Some(vec!["むり".into()]),
                ..LocalizedEneRoleplay::default()
            }),
            ..LocalizedCharacterFields::default()
        };

        merge_localized_fields(&mut card, &diff);

        assert!(
            card.data.extensions.ene.is_none(),
            "a diff must never create an ene block"
        );
    }
}
