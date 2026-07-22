use ene_store::{AffectState, MemoryKind};

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

pub fn infer_intents(topic: &str, affect: Option<&AffectState>) -> Vec<RecallIntent> {
    let mut intents = vec![RecallIntent::Semantic];
    let lower = topic.to_lowercase();

    if crate::contains_any(
        &lower,
        &[
            "remember",
            "last time",
            "previous",
            "before",
            "前回",
            "この前",
            "覚えて",
            "思い出",
            "あのとき",
            "あの時",
        ],
    ) {
        intents.push(RecallIntent::Episodic);
    }

    if crate::contains_any(
        &lower,
        &[
            "i like",
            "i love",
            "i dislike",
            "i hate",
            "favorite",
            "prefer",
            "好き",
            "嫌い",
            "好み",
            "お気に入り",
        ],
    ) {
        intents.push(RecallIntent::Preference);
    }

    if crate::contains_any(
        &lower,
        &[
            "between us",
            "our relationship",
            "trust",
            "friend",
            "together",
            "関係",
            "信頼",
            "友達",
            "仲良",
            "一緒",
        ],
    ) {
        intents.push(RecallIntent::Relationship);
    }

    if crate::contains_any(
        &lower,
        &[
            "feel",
            "felt",
            "happy",
            "sad",
            "angry",
            "anxious",
            "気持ち",
            "嬉しい",
            "悲しい",
            "怒",
            "不安",
        ],
    ) {
        intents.push(RecallIntent::Affective);
    }

    if crate::contains_any(
        &lower,
        &[
            "how do i",
            "how to",
            "steps",
            "procedure",
            "setup",
            "やり方",
            "手順",
            "方法",
            "設定",
        ],
    ) {
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
