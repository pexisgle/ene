//! Style example chunking and retrieval (#84).

use std::sync::Arc;

use ene_config::{CharacterCardV3, expand_cbs_macros};
use ene_memory::{
    MemoryConfidence, MemoryKind, MemoryScope, MemorySource, MemoryStatus, MemoryStore,
    NewMemoryItem,
};
use ene_provider::EmbeddingProvider;

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
    fn tag(self) -> &'static str {
        match self {
            Self::Greeting => "greeting",
            Self::Comforting => "comforting",
            Self::Joking => "joking",
            Self::SeriousExplanation => "serious_explanation",
            Self::Refusal => "refusal",
            Self::ToolUse => "tool_use",
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

/// Compiles and selects CCv3 dialogue style examples.
#[derive(Debug, Default, Clone, Copy)]
pub struct StyleExampleSelector;

impl StyleExampleSelector {
    /// Split `mes_example` into dialogue chunks.
    #[must_use]
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
                let intent = infer_style_intent(block).unwrap_or(default_intent_for_index(idx));
                (intent, block.trim().to_string())
            })
            .filter(|(_, text)| !text.is_empty())
            .collect()
    }

    /// Compile style chunks into typed memory items for indexing.
    #[must_use]
    pub fn compile_items(card: &CharacterCardV3, user_name: &str) -> Vec<NewMemoryItem> {
        let char_name = card.data.get_character_name();
        let character_id = char_name.to_string();
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
                salience: ene_memory::MemorySalience::new(0.8),
                affect: Default::default(),
                relationship_impact: 0.0,
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                created_at: None,
            })
            .collect()
    }

    /// Select up to `max_examples` style examples for the current turn.
    pub async fn select(
        card: &CharacterCardV3,
        user_name: &str,
        user_input: &str,
        store: Option<&MemoryStore>,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
        config: &CharacterMemoryConfig,
        max_examples: usize,
    ) -> Vec<StyleExample> {
        if !config.style_retrieval || max_examples == 0 {
            return Vec::new();
        }

        let intent = infer_style_intent(user_input).unwrap_or(StyleIntent::Greeting);
        let char_name = card.data.get_character_name();

        if let (Some(store), Some(_embedder)) = (store, embedder)
            && let Ok(selected) = select_from_store(store, char_name, intent, max_examples).await
            && !selected.is_empty()
        {
            return selected;
        }

        select_from_card(card, user_name, intent, max_examples)
    }
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
    store: &MemoryStore,
    character_id: &str,
    intent: StyleIntent,
    max_examples: usize,
) -> Result<Vec<StyleExample>, ene_memory::MemoryError> {
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

fn default_intent_for_index(index: usize) -> StyleIntent {
    match index % 3 {
        0 => StyleIntent::Greeting,
        1 => StyleIntent::Comforting,
        _ => StyleIntent::SeriousExplanation,
    }
}

/// Infer style intent from user text using deterministic keyword heuristics.
#[must_use]
pub fn infer_style_intent(text: &str) -> Option<StyleIntent> {
    let lower = text.to_lowercase();
    if contains_any(
        &lower,
        &[
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
    if contains_any(
        &lower,
        &[
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
    if contains_any(
        &lower,
        &["joke", "funny", "lol", "haha", "冗談", "笑", "面白"],
    ) {
        return Some(StyleIntent::Joking);
    }
    if contains_any(
        &lower,
        &[
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
    if contains_any(
        &lower,
        &[
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
    if contains_any(
        &lower,
        &[
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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

#[cfg(test)]
mod tests {
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
}
