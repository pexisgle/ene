//! Deterministic keyword/heuristic appraisal rules.

#![expect(
    clippy::arithmetic_side_effects,
    clippy::string_slice,
    reason = "mind pipeline uses intentional turn/score/index arithmetic; special-token and summarizer parsers slice at known ASCII delimiters"
)]
use ene_core::AffectState;

use super::types::{AffectDelta, AffectUpdateReason};

/// A single appraisal rule: keyword patterns and field deltas.
struct AppraisalRule {
    category: &'static str,
    patterns: &'static [&'static str],
    valence: f32,
    arousal: f32,
    affinity: f32,
    trust: f32,
    irritation: f32,
    fatigue: f32,
    curiosity: f32,
}

const RULES: &[AppraisalRule] = &[
    AppraisalRule {
        category: "gratitude",
        patterns: &[
            "thank you",
            "thanks",
            "thank",
            "ありがとう",
            "感謝",
            "grateful",
        ],
        valence: 0.15,
        arousal: 0.0,
        affinity: 0.1,
        trust: 0.05,
        irritation: 0.0,
        fatigue: 0.0,
        curiosity: 0.0,
    },
    AppraisalRule {
        category: "praise",
        patterns: &[
            "great job",
            "well done",
            "amazing",
            "awesome",
            "すごい",
            "素晴らしい",
            "最高",
            "nice work",
            "good job",
        ],
        valence: 0.2,
        arousal: 0.05,
        affinity: 0.12,
        trust: 0.08,
        irritation: 0.0,
        fatigue: 0.0,
        curiosity: 0.05,
    },
    AppraisalRule {
        category: "insult",
        patterns: &[
            "hate you",
            "stupid",
            "idiot",
            "shut up",
            "useless",
            "死ね",
            "バカ",
            "うざい",
        ],
        valence: -0.25,
        arousal: 0.1,
        affinity: -0.15,
        trust: -0.1,
        irritation: 0.2,
        fatigue: 0.0,
        curiosity: 0.0,
    },
    AppraisalRule {
        category: "urgency",
        patterns: &["asap", "urgent", "hurry", "急いで", "今すぐ", "quickly"],
        valence: 0.0,
        arousal: 0.15,
        affinity: 0.0,
        trust: 0.0,
        irritation: 0.05,
        fatigue: 0.0,
        curiosity: 0.0,
    },
];

/// Threshold of recent user turns before long-conversation fatigue applies.
const LONG_CONVERSATION_TURN_THRESHOLD: usize = 16;

/// Fatigue increment per turn beyond the long-conversation threshold.
const LONG_CONVERSATION_FATIGUE_DELTA: f32 = 0.03;

/// Apply deterministic appraisal rules to the affect state.
pub fn apply_appraisal(
    state: &mut AffectState,
    user_message: &str,
    recent_turn_count: usize,
) -> Vec<AffectUpdateReason> {
    let normalized = user_message.to_lowercase();
    let mut reasons = Vec::new();

    for rule in RULES {
        if !rule
            .patterns
            .iter()
            .any(|p| pattern_matches(&normalized, p))
        {
            continue;
        }

        let mut deltas = Vec::new();
        apply_field_delta(state, "valence", rule.valence, &mut deltas);
        apply_field_delta(state, "arousal", rule.arousal, &mut deltas);
        apply_field_delta(state, "affinity", rule.affinity, &mut deltas);
        apply_field_delta(state, "trust", rule.trust, &mut deltas);
        apply_field_delta(state, "irritation", rule.irritation, &mut deltas);
        apply_field_delta(state, "fatigue", rule.fatigue, &mut deltas);
        apply_field_delta(state, "curiosity", rule.curiosity, &mut deltas);

        if !deltas.is_empty() {
            reasons.push(AffectUpdateReason {
                category: rule.category,
                detail: format!(
                    "matched keyword rule `{category}`",
                    category = rule.category
                ),
                deltas,
            });
        }
    }

    // Exclamation marks boost arousal slightly.
    let exclamation_count = user_message.chars().filter(|c| *c == '!').count();
    if exclamation_count > 0 {
        let boost = (exclamation_count as f32 * 0.03).min(0.12);
        let mut deltas = Vec::new();
        apply_field_delta(state, "arousal", boost, &mut deltas);
        reasons.push(AffectUpdateReason {
            category: "urgency",
            detail: format!("{exclamation_count} exclamation mark(s)"),
            deltas,
        });
    }

    if recent_turn_count >= LONG_CONVERSATION_TURN_THRESHOLD {
        let mut deltas = Vec::new();
        apply_field_delta(
            state,
            "fatigue",
            LONG_CONVERSATION_FATIGUE_DELTA,
            &mut deltas,
        );
        reasons.push(AffectUpdateReason {
            category: "fatigue",
            detail: format!(
                "long conversation ({recent_turn_count} recent turns >= {LONG_CONVERSATION_TURN_THRESHOLD})"
            ),
            deltas,
        });
    }

    reasons
}

fn ascii_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a character is katakana (full-width or half-width).
fn is_katakana(c: char) -> bool {
    matches!(c, '\u{30A0}'..='\u{30FF}' | '\u{FF65}'..='\u{FF9F}')
}

/// Whether a character is a CJK ideograph (kanji).
fn is_kanji(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

/// Japanese negative suffixes that flip a keyword's polarity when they follow it.
///
/// Deliberately conservative: bare `ない` is excluded because it also appears in
/// unrelated words (e.g. `死ねない` "cannot"), which would mask genuine signals.
const JA_NEGATION_SUFFIXES: &[&str] = &["じゃない", "ではない", "くない", "ありません"];

/// Match a pattern against normalized user text.
///
/// ASCII single-word patterns use token boundaries; multi-word ASCII patterns
/// use substring matching. Non-ASCII (Japanese) patterns use script-aware
/// boundary matching: the pattern must not be embedded inside a longer run of
/// its own script (so `バカ` does not fire inside `バカンス`), and a trailing
/// negation suffix suppresses the match (so `嫌いじゃない` reads as a preference,
/// not an insult). This keeps Japanese and ASCII matching comparably strict.
fn pattern_matches(normalized: &str, pattern: &str) -> bool {
    if !pattern.is_ascii() {
        return japanese_pattern_matches(normalized, pattern);
    }
    let pattern_lower = pattern.to_lowercase();
    if pattern_lower.contains(' ') {
        normalized.contains(&pattern_lower)
    } else {
        ascii_tokens(normalized)
            .iter()
            .any(|token| token == &pattern_lower)
    }
}

/// Match a non-ASCII pattern with script-run boundaries and negation awareness.
fn japanese_pattern_matches(normalized: &str, pattern: &str) -> bool {
    normalized.match_indices(pattern).any(|(start, matched)| {
        let end = start + matched.len();
        !embedded_in_script_run(normalized, pattern, start, end)
            && !negation_follows(&normalized[end..])
    })
}

/// Whether the match at `[start, end)` is glued into a larger word of the same script.
///
/// Japanese has no whitespace, so a keyword fused to a neighbouring character of
/// its *own* script is part of a longer word rather than a standalone term:
/// `バカ` inside the katakana run `バカンス`, or a kanji keyword inside a longer
/// kanji compound. A script change (kana ↔ kanji), hiragana grammatical glue, and
/// punctuation all count as boundaries, so `大バカ` and `最高の気分` still match.
fn embedded_in_script_run(text: &str, pattern: &str, start: usize, end: usize) -> bool {
    let prev = text[..start].chars().next_back();
    let next = text[end..].chars().next();
    let glued_before = matches!((prev, pattern.chars().next()),
        (Some(adjacent), Some(edge)) if same_script_run(adjacent, edge));
    let glued_after = matches!((next, pattern.chars().next_back()),
        (Some(adjacent), Some(edge)) if same_script_run(adjacent, edge));
    glued_before || glued_after
}

/// Whether `adjacent` continues the same script run as the keyword edge `edge`.
///
/// Only katakana and kanji form runs that fuse words; hiragana (particles,
/// okurigana, copula) is grammatical glue and always acts as a boundary.
fn same_script_run(adjacent: char, edge: char) -> bool {
    (is_katakana(edge) && is_katakana(adjacent)) || (is_kanji(edge) && is_kanji(adjacent))
}

/// Whether the text immediately after a match begins with a Japanese negation.
fn negation_follows(rest: &str) -> bool {
    JA_NEGATION_SUFFIXES
        .iter()
        .any(|suffix| rest.starts_with(suffix))
}

fn apply_field_delta(
    state: &mut AffectState,
    field: &'static str,
    delta: f32,
    deltas: &mut Vec<AffectDelta>,
) {
    if delta.abs() < f32::EPSILON {
        return;
    }
    match field {
        "valence" => {
            let old = state.valence;
            state.valence += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.valence - old,
            });
        }
        "arousal" => {
            let old = state.arousal;
            state.arousal += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.arousal - old,
            });
        }
        "dominance" => {
            let old = state.dominance;
            state.dominance += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.dominance - old,
            });
        }
        "trust" => {
            let old = state.trust;
            state.trust += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.trust - old,
            });
        }
        "affinity" => {
            let old = state.affinity;
            state.affinity += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.affinity - old,
            });
        }
        "irritation" => {
            let old = state.irritation;
            state.irritation += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.irritation - old,
            });
        }
        "curiosity" => {
            let old = state.curiosity;
            state.curiosity += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.curiosity - old,
            });
        }
        "fatigue" => {
            let old = state.fatigue;
            state.fatigue += delta;
            deltas.push(AffectDelta {
                field,
                delta: state.fatigue - old,
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_core::AffectState;

    #[test]
    fn gratitude_raises_valence_and_affinity() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "Thank you so much!", 2);
        assert!(reasons.iter().any(|r| r.category == "gratitude"));
        assert!(state.valence > 0.0);
        assert!(state.affinity > 0.0);
    }

    #[test]
    fn insult_lowers_valence_and_raises_irritation() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "I hate you, you're stupid", 2);
        assert!(reasons.iter().any(|r| r.category == "insult"));
        assert!(state.valence < 0.0);
        assert!(state.irritation > 0.0);
    }

    #[test]
    fn thankless_does_not_trigger_gratitude() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "This is thankless work", 2);
        assert!(!reasons.iter().any(|r| r.category == "gratitude"));
        assert!((state.valence - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn stated_preference_is_not_an_insult() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "きのこが嫌いなんだよね", 2);
        assert!(!reasons.iter().any(|r| r.category == "insult"));
        assert!((state.valence - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negated_dislike_is_not_an_insult() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "この曲嫌いじゃない", 2);
        assert!(!reasons.iter().any(|r| r.category == "insult"));
    }

    #[test]
    fn katakana_substring_bakansu_is_not_an_insult() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "バカンスの予定を立てたい", 2);
        assert!(!reasons.iter().any(|r| r.category == "insult"));
        assert!((state.valence - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negated_insult_is_suppressed() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "別にバカじゃないよ", 2);
        assert!(!reasons.iter().any(|r| r.category == "insult"));
    }

    #[test]
    fn genuine_japanese_insult_still_detected() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "ほんとにバカだな", 2);
        assert!(reasons.iter().any(|r| r.category == "insult"));
        assert!(state.valence < 0.0);
        assert!(state.irritation > 0.0);
    }

    #[test]
    fn japanese_insult_across_script_boundary_still_detected() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "大バカ者", 2);
        assert!(reasons.iter().any(|r| r.category == "insult"));
    }

    #[test]
    fn long_conversation_raises_fatigue() {
        let mut state = AffectState::neutral("ene");
        let reasons = apply_appraisal(&mut state, "hello", LONG_CONVERSATION_TURN_THRESHOLD);
        assert!(reasons.iter().any(|r| r.category == "fatigue"));
        assert!(state.fatigue > 0.0);
    }
}
