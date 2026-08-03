//! Style example chunking and retrieval.

use std::sync::Arc;

use ene_ai::EmbeddingProvider;
use ene_config::{CharacterCardV3, expand_cbs_macros};
use ene_core::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemoryPort, MemoryPortError, MemoryScope,
    MemorySource, MemoryStatus, NewMemoryItem,
};

use crate::config::CharacterMemoryConfig;

/// Prefix for style-example `source_ref` values.
pub const STYLE_SOURCE_PREFIX: &str = "ccv3:style:";

/// Turn intent categories for style example selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StyleIntent {
    /// Greeting or small talk openers.
    Greeting,
    /// Comforting or empathetic responses.
    Comforting,
    /// Playful or joking tone.
    Joking,
    /// Serious factual explanations.
    SeriousExplanation,
    /// Polite refusal boundaries.
    Refusal,
    /// Tool-use or action narration style.
    ToolUse,
}

impl StyleIntent {
    const fn tag(self) -> &'static str {
        match self {
            Self::Greeting => "greeting",
            Self::Comforting => "comforting",
            Self::Joking => "joking",
            Self::SeriousExplanation => "serious_explanation",
            Self::Refusal => "refusal",
            Self::ToolUse => "tool_use",
        }
    }

    /// Resolves a labeled-example label to a canonical intent, if it is one
    /// of the selector's tags.
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "greeting" => Some(Self::Greeting),
            "comforting" => Some(Self::Comforting),
            "joking" => Some(Self::Joking),
            "serious_explanation" => Some(Self::SeriousExplanation),
            "refusal" => Some(Self::Refusal),
            "tool_use" => Some(Self::ToolUse),
            _ => None,
        }
    }
}

/// A selected style example ready for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleExample {
    /// Rendered example dialogue text.
    pub text: String,
    /// Intent category that matched.
    pub intent: StyleIntent,
}

/// Compiles and selects `CCv3` dialogue style examples.
#[derive(Debug, Default, Clone, Copy)]
pub struct StyleExampleSelector;

impl StyleExampleSelector {
    /// Split `mes_example` into dialogue chunks.
    pub fn chunk_mes_example(
        raw: &str,
        char_name: &str,
        user_name: &str,
    ) -> Vec<(StyleIntent, String)> {
        let expanded = expand_cbs_macros(raw.trim(), char_name, user_name);
        if expanded.is_empty() {
            return Vec::new();
        }

        let blocks: Vec<&str> = if expanded.contains("<START>") {
            expanded
                .split("<START>")
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .collect()
        } else {
            vec![expanded.as_str()]
        };

        blocks
            .into_iter()
            .enumerate()
            .map(|(idx, block)| {
                let intent =
                    infer_style_intent(block).unwrap_or_else(|| default_intent_for_index(idx));
                (intent, block.trim().to_string())
            })
            .filter(|(_, text)| !text.is_empty())
            .collect()
    }

    /// Compile style chunks into typed memory items for indexing.
    pub fn compile_items(card: &CharacterCardV3, user_name: &str) -> Vec<NewMemoryItem> {
        let char_name = card.data.get_character_name();
        let character_id = card.data.get_character_id().to_string();
        if let Some(examples) = labeled_examples(card) {
            return examples
                .into_iter()
                .enumerate()
                .map(|(index, example)| {
                    let tag = match StyleIntent::from_tag(&example.label) {
                        Some(intent) => intent.tag(),
                        None => example.label.as_str(),
                    };
                    NewMemoryItem {
                        scope: MemoryScope::Character,
                        character_id: character_id.clone(),
                        user_id: String::new(),
                        kind: MemoryKind::Procedure,
                        title: format!("[style:{tag}] example {index}"),
                        content: example.text,
                        source: MemorySource::Ccv3,
                        source_ref: Some(format!("{STYLE_SOURCE_PREFIX}{index}")),
                        confidence: MemoryConfidence::new(1.0),
                        salience: ene_core::MemorySalience::new(0.8),
                        affect: AffectAnnotation::default(),
                        relationship_impact: 0.0,
                        valid_from: None,
                        valid_until: None,
                        status: MemoryStatus::Active,
                        supersedes_id: None,
                        pinned: false,
                        created_at: None,
                        commitment_id: None,
                    }
                })
                .collect();
        }
        Self::chunk_mes_example(&card.data.mes_example, char_name, user_name)
            .into_iter()
            .enumerate()
            .map(|(index, (intent, text))| NewMemoryItem {
                scope: MemoryScope::Character,
                character_id: character_id.clone(),
                user_id: String::new(),
                kind: MemoryKind::Procedure,
                title: format!("[style:{}] example {}", intent.tag(), index),
                content: text,
                source: MemorySource::Ccv3,
                source_ref: Some(format!("{STYLE_SOURCE_PREFIX}{index}")),
                confidence: MemoryConfidence::new(1.0),
                salience: ene_core::MemorySalience::new(0.8),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
                commitment_id: None,
            })
            .collect()
    }

    /// Select up to `max_examples` style examples for the current turn.
    pub async fn select(
        card: &CharacterCardV3,
        user_name: &str,
        user_input: &str,
        store: Option<&dyn MemoryPort>,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
        _config: &CharacterMemoryConfig,
        max_examples: usize,
    ) -> Vec<StyleExample> {
        if max_examples == 0 {
            return Vec::new();
        }

        let intent = infer_style_intent(user_input).unwrap_or(StyleIntent::Greeting);
        let character_id = card.data.get_character_id();

        // Labeled examples are structured, so selection is card-direct and
        // never competes with (or loses to) the compiled memory pool.
        if let Some(examples) = labeled_examples(card) {
            return select_labeled(&examples, user_input, intent, max_examples);
        }

        if let (Some(store), Some(_embedder)) = (store, embedder)
            && let Ok(selected) = select_from_store(store, character_id, intent, max_examples).await
            && !selected.is_empty()
        {
            return selected;
        }

        select_from_card(card, user_name, intent, max_examples)
    }
}

/// The card's labeled style examples; `None` when absent or empty.
fn labeled_examples(card: &CharacterCardV3) -> Option<Vec<ene_config::LabeledStyleExample>> {
    card.data
        .get_ene_extension()
        .and_then(|ext| ext.style_examples)
        .filter(|examples| !examples.is_empty())
}

/// Select labeled examples by deterministic label matching.
///
/// A label equal to a canonical intent tag selects through the intent
/// pipeline; any other non-empty label is matched as a case-insensitive
/// substring of the user's input. No match falls back to the first
/// `max_examples` examples, mirroring the flat-example fallback.
fn select_labeled(
    examples: &[ene_config::LabeledStyleExample],
    user_input: &str,
    intent: StyleIntent,
    max_examples: usize,
) -> Vec<StyleExample> {
    let lower_input = user_input.to_lowercase();
    let mut matched: Vec<StyleExample> = examples
        .iter()
        .filter(|example| {
            StyleIntent::from_tag(&example.label) == Some(intent)
                || label_matches(&example.label, &lower_input)
        })
        .map(|example| StyleExample {
            text: example.text.clone(),
            intent,
        })
        .take(max_examples)
        .collect();
    if matched.is_empty() {
        matched = examples
            .iter()
            .take(max_examples)
            .map(|example| StyleExample {
                text: example.text.clone(),
                intent,
            })
            .collect();
    }
    matched
}

fn label_matches(label: &str, lower_input: &str) -> bool {
    !label.trim().is_empty() && lower_input.contains(&label.to_lowercase())
}

fn select_from_card(
    card: &CharacterCardV3,
    user_name: &str,
    intent: StyleIntent,
    max_examples: usize,
) -> Vec<StyleExample> {
    let char_name = card.data.get_character_name();
    let chunks =
        StyleExampleSelector::chunk_mes_example(&card.data.mes_example, char_name, user_name);
    if chunks.is_empty() {
        return Vec::new();
    }

    let mut matched: Vec<StyleExample> = chunks
        .into_iter()
        .filter(|(chunk_intent, _)| *chunk_intent == intent)
        .map(|(chunk_intent, text)| StyleExample {
            text,
            intent: chunk_intent,
        })
        .take(max_examples)
        .collect();

    if matched.is_empty() {
        matched =
            StyleExampleSelector::chunk_mes_example(&card.data.mes_example, char_name, user_name)
                .into_iter()
                .take(max_examples)
                .map(|(chunk_intent, text)| StyleExample {
                    text,
                    intent: chunk_intent,
                })
                .collect();
    }

    matched
}

async fn select_from_store(
    store: &dyn MemoryPort,
    character_id: &str,
    intent: StyleIntent,
    max_examples: usize,
) -> Result<Vec<StyleExample>, MemoryPortError> {
    let items = store
        .list_typed_memories_by_source_prefix(character_id, STYLE_SOURCE_PREFIX, 64)
        .await?;

    let intent_tag = intent.tag();
    let intent_matches: Vec<_> = items
        .iter()
        .filter(|item| item.title.contains(&format!("[style:{intent_tag}]")))
        .collect();

    let pool = if intent_matches.is_empty() {
        items.iter().collect::<Vec<_>>()
    } else {
        intent_matches
    };

    Ok(pool
        .into_iter()
        .take(max_examples)
        .map(|item| StyleExample {
            text: item.content.clone(),
            intent,
        })
        .collect())
}

const fn default_intent_for_index(index: usize) -> StyleIntent {
    match index % 3 {
        0 => StyleIntent::Greeting,
        1 => StyleIntent::Comforting,
        _ => StyleIntent::SeriousExplanation,
    }
}

/// Infer style intent from user text using deterministic keyword heuristics.
pub fn infer_style_intent(text: &str) -> Option<StyleIntent> {
    let lower = text.to_lowercase();
    if crate::contains_any(
        &lower,
        [
            "hello",
            "hi ",
            "hey",
            "good morning",
            "good evening",
            "こんにちは",
            "おはよう",
            "こんばんは",
            "やあ",
        ],
    ) {
        return Some(StyleIntent::Greeting);
    }
    if crate::contains_any(
        &lower,
        [
            "sorry",
            "sad",
            "upset",
            "comfort",
            "cheer up",
            "it's okay",
            "大丈夫",
            "悲し",
            "落ち込",
            "慰め",
        ],
    ) {
        return Some(StyleIntent::Comforting);
    }
    if crate::contains_any(
        &lower,
        ["joke", "funny", "lol", "haha", "冗談", "笑", "面白"],
    ) {
        return Some(StyleIntent::Joking);
    }
    if crate::contains_any(
        &lower,
        [
            "explain",
            "why ",
            "how does",
            "what is",
            "describe",
            "説明",
            "理由",
            "教えて",
        ],
    ) {
        return Some(StyleIntent::SeriousExplanation);
    }
    if crate::contains_any(
        &lower,
        [
            "can't",
            "cannot",
            "won't",
            "refuse",
            "don't",
            "できない",
            "拒否",
            "無理",
        ],
    ) {
        return Some(StyleIntent::Refusal);
    }
    if crate::contains_any(
        &lower,
        [
            "tool",
            "search",
            "file",
            "execute",
            "run ",
            "ツール",
            "検索",
            "実行",
        ],
    ) {
        return Some(StyleIntent::ToolUse);
    }
    None
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index into fixed-size fixture vectors"
    )]
    use super::*;

    #[test]
    fn chunks_start_delimited_examples() {
        let raw = "<START>\n{{user}}: Hi\n{{char}}: Hello there!";
        let chunks = StyleExampleSelector::chunk_mes_example(raw, "Ene", "User");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].1.contains("Hello there"));
    }

    #[test]
    fn infer_greeting_intent() {
        assert_eq!(
            infer_style_intent("Hello there!"),
            Some(StyleIntent::Greeting)
        );
    }

    #[test]
    fn select_from_card_respects_intent() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.mes_example = "<START>\n{{user}}: Hi\n{{char}}: Hey!\n<START>\n{{user}}: explain\n{{char}}: Here is why.".into();
        let selected = select_from_card(&card, "User", StyleIntent::Greeting, 1);
        assert_eq!(selected.len(), 1);
        assert!(selected[0].text.contains("Hey"));
    }

    fn labeled_card() -> CharacterCardV3 {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.mes_example = "<START>\n{{user}}: Hi\n{{char}}: Flat greeting".into();
        card.data.extensions.ene = Some(ene_config::EneExtension {
            style_examples: Some(vec![
                ene_config::LabeledStyleExample {
                    id: "g-1".into(),
                    label: "greeting".into(),
                    text: "Labeled greeting".into(),
                },
                ene_config::LabeledStyleExample {
                    id: "a-1".into(),
                    label: "angry".into(),
                    text: "Labeled angry reply".into(),
                },
                ene_config::LabeledStyleExample {
                    id: "f-1".into(),
                    label: "first meeting".into(),
                    text: "Labeled first-meeting reply".into(),
                },
            ]),
            ..ene_config::EneExtension::default()
        });
        card
    }

    #[test]
    fn labeled_examples_replace_flat_mes_example_selection() {
        let card = labeled_card();
        let selected = select_labeled(
            &labeled_examples(&card).expect("labeled examples present"),
            "hello there",
            StyleIntent::Greeting,
            2,
        );
        assert_eq!(selected.len(), 1, "only the greeting label matches");
        assert_eq!(selected[0].text, "Labeled greeting");
    }

    #[test]
    fn labeled_examples_match_labels_as_input_substrings() {
        let card = labeled_card();
        let examples = labeled_examples(&card).expect("labeled examples present");
        let selected = select_labeled(
            &examples,
            "I'm really angry right now",
            StyleIntent::SeriousExplanation,
            1,
        );
        assert_eq!(selected[0].text, "Labeled angry reply");

        let first_meeting = select_labeled(
            &examples,
            "It's our first meeting, right?",
            StyleIntent::SeriousExplanation,
            1,
        );
        assert_eq!(first_meeting[0].text, "Labeled first-meeting reply");
    }

    #[test]
    fn labeled_examples_fall_back_to_first_entries_without_a_match() {
        let card = labeled_card();
        let selected = select_labeled(
            &labeled_examples(&card).expect("labeled examples present"),
            "unrelated topic",
            StyleIntent::Joking,
            2,
        );
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].text, "Labeled greeting");
        assert_eq!(selected[1].text, "Labeled angry reply");
    }

    #[test]
    fn compile_items_uses_labeled_examples_when_defined() {
        let card = labeled_card();
        let items = StyleExampleSelector::compile_items(&card, "User");
        assert_eq!(items.len(), 3);
        assert!(items[0].title.contains("[style:greeting]"));
        assert!(items[1].title.contains("[style:angry]"));
        assert!(items[2].title.contains("[style:first meeting]"));
        assert_eq!(items[0].content, "Labeled greeting");
    }

    #[test]
    fn empty_label_never_matches_input() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.extensions.ene = Some(ene_config::EneExtension {
            style_examples: Some(vec![ene_config::LabeledStyleExample {
                id: "e-1".into(),
                label: String::new(),
                text: "Empty label".into(),
            }]),
            ..ene_config::EneExtension::default()
        });
        let selected = select_labeled(
            &labeled_examples(&card).expect("labeled examples present"),
            "",
            StyleIntent::Greeting,
            1,
        );
        assert_eq!(selected[0].text, "Empty label", "fallback still returns it");
    }
}
