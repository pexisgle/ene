use ene_config::PatternLibrary;
use ene_core::{AffectState, MemoryKind};

/// Heuristic recall intents inferred from the current turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallIntent {
    Semantic,
    Episodic,
    Preference,
    Relationship,
    Affective,
    Procedure,
}

/// Infers heuristic recall intents from the current turn.
///
/// Keyword matching is driven by the language pack for `lang` (see
/// [`ene_config::PatternLibrary`]) — its `intent_keywords` section in
/// particular — so tuning the corpus or adding a language needs no Rust
/// change (#355). `lang` follows the standard pack resolution: aliases are
/// normalized, `ene_config::SUPPORTED_LANGUAGES` carry a compile-time
/// embedded fallback, and unknown languages fall back to English. A topic
/// matching no keywords yields `Semantic` only, as before.
pub fn infer_intents(topic: &str, affect: Option<&AffectState>, lang: &str) -> Vec<RecallIntent> {
    let mut intents = vec![RecallIntent::Semantic];
    let lower = topic.to_lowercase();
    let pack = PatternLibrary::load(lang);
    let keywords = pack.intent_keywords();

    if crate::contains_any(&lower, &keywords.episodic) {
        intents.push(RecallIntent::Episodic);
    }

    if crate::contains_any(&lower, &keywords.preference) {
        intents.push(RecallIntent::Preference);
    }

    if crate::contains_any(&lower, &keywords.relationship) {
        intents.push(RecallIntent::Relationship);
    }

    if crate::contains_any(&lower, &keywords.affective) {
        intents.push(RecallIntent::Affective);
    }

    if crate::contains_any(&lower, &keywords.procedure) {
        intents.push(RecallIntent::Procedure);
    }

    if let Some(state) = affect
        && (state.valence.abs() >= 0.6 || state.arousal.abs() >= 0.6)
    {
        intents.push(RecallIntent::Affective);
    }

    dedupe_intents(&mut intents);
    intents
}

pub fn kinds_for_intents(intents: &[RecallIntent], has_commitments: bool) -> Vec<MemoryKind> {
    let mut kinds = vec![MemoryKind::Semantic, MemoryKind::Episodic];

    for intent in intents {
        match intent {
            RecallIntent::Semantic => {}
            RecallIntent::Episodic => push_unique(&mut kinds, MemoryKind::Episodic),
            RecallIntent::Preference => push_unique(&mut kinds, MemoryKind::Preference),
            RecallIntent::Relationship => push_unique(&mut kinds, MemoryKind::Relationship),
            RecallIntent::Affective => push_unique(&mut kinds, MemoryKind::Affective),
            RecallIntent::Procedure => push_unique(&mut kinds, MemoryKind::Procedure),
        }
    }

    if has_commitments {
        push_unique(&mut kinds, MemoryKind::Commitment);
    }

    kinds
}

fn dedupe_intents(intents: &mut Vec<RecallIntent>) {
    let mut deduped = Vec::with_capacity(intents.len());
    for intent in intents.iter().copied() {
        if !deduped.contains(&intent) {
            deduped.push(intent);
        }
    }
    *intents = deduped;
}

fn push_unique(kinds: &mut Vec<MemoryKind>, kind: MemoryKind) {
    if !kinds.contains(&kind) {
        kinds.push(kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_contains(intents: &[RecallIntent], expected: RecallIntent) {
        assert!(
            intents.contains(&expected),
            "expected {expected:?} in intents {intents:?}"
        );
    }

    fn strong_affect() -> AffectState {
        let mut affect = AffectState::neutral("ene");
        affect.valence = 0.7;
        affect
    }

    #[test]
    fn english_keywords_infer_matching_intents() {
        let cases = [
            ("remember our trip last time", RecallIntent::Episodic),
            ("i like coffee in the morning", RecallIntent::Preference),
            ("are we still friends", RecallIntent::Relationship),
            ("i feel anxious about it", RecallIntent::Affective),
            ("how to set up the plugin", RecallIntent::Procedure),
        ];
        for (topic, expected) in cases {
            assert_contains(&infer_intents(topic, None, "en"), expected);
        }
    }

    #[test]
    fn japanese_keywords_infer_matching_intents() {
        let cases = [
            ("前回の話を思い出して", RecallIntent::Episodic),
            ("好きな食べ物は何", RecallIntent::Preference),
            ("友達と一緒にいた話", RecallIntent::Relationship),
            ("気持ちが不安です", RecallIntent::Affective),
            ("設定のやり方を教えて", RecallIntent::Procedure),
        ];
        for (topic, expected) in cases {
            assert_contains(&infer_intents(topic, None, "ja"), expected);
        }
    }

    #[test]
    fn unknown_language_falls_back_to_english_keywords() {
        // "fr" has no pack and no embedded fallback: the English pack applies,
        // so English keywords still trigger their intents (#355).
        assert_contains(
            &infer_intents("I like coffee", None, "fr"),
            RecallIntent::Preference,
        );
        assert_contains(
            &infer_intents("Remember our trip", None, "fr"),
            RecallIntent::Episodic,
        );
    }

    #[test]
    fn no_keyword_match_yields_semantic_only() {
        assert_eq!(
            infer_intents("what is the weather like", None, "en"),
            vec![RecallIntent::Semantic]
        );
    }

    #[test]
    fn strong_affect_adds_affective_intent_without_keywords() {
        assert_contains(
            &infer_intents("hello", Some(&strong_affect()), "en"),
            RecallIntent::Affective,
        );
    }

    #[test]
    fn language_aliases_resolve_to_the_same_pack() {
        // "jp" (legacy) and "ja-JP" must load the Japanese pack (#354 alias
        // semantics shared with the deterministic extractor).
        assert_contains(
            &infer_intents("前回の話", None, "jp"),
            RecallIntent::Episodic,
        );
        assert_contains(
            &infer_intents("前回の話", None, "ja-JP"),
            RecallIntent::Episodic,
        );
    }
}
