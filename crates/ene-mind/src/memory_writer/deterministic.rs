//! Deterministic memory extractor — minimal pattern safety net.
//!
//! Only explicit forget (deletion) instructions are pattern-matched. Explicit
//! remember requests are owned by the LLM extractor (#354); preferences,
//! schedules, nicknames, and other soft signals are left to the LLM too.
//! Tool-result grounding remains a separate, configurable path.
//!
//! Forget patterns are loaded from per-language packs
//! (`assets/lang/{lang}/patterns.json`) via [`ene_config::PatternLibrary`], so
//! adding a language is a data-only change — no Rust code. A language without
//! a pack falls back to English patterns.
//!
//! Confidence calibration:
//! - Forget (deletion request): 0.90
//! - Tool-grounded procedure/reflection/episodic: 0.60–0.70 (when enabled)

#![expect(
    clippy::string_slice,
    reason = "special-token and summarizer parsers slice at known ASCII delimiters"
)]
use regex::Regex;

use super::candidate::{Locale, MemoryCandidate, TurnInput};
use super::tool_grounding;
use crate::config::ToolGroundingConfig;
use crate::error::CognitionError;
use ene_config::PatternLibrary;
use ene_core::MemoryKind;

/// Normalise Unicode with NFKC (fullwidth → ASCII, combined → single).
fn nfkc(s: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfkc(s).collect()
}

/// Interrogative sentence openers (lowercase). A `forget` keyword inside a
/// question is not an instruction — e.g. "did you forget my name?" (issue #70
/// precision guard).
const EN_QUESTION_STARTERS: &[&str] = &[
    "do you",
    "did you",
    "does",
    "can you",
    "could you",
    "will you",
    "would you",
    "have you",
    "are you",
    "what",
    "when",
    "where",
    "who",
    "why",
    "how",
    "which",
    "is it",
    "was it",
];

/// Return `true` when `msg` reads as an English question rather than an
/// instruction: it contains a `?` (the fullwidth `？` is already folded
/// to ASCII by [`nfkc`]) or begins with an interrogative opener, which
/// catches questions written without a trailing `?`.
fn is_en_question(msg: &str) -> bool {
    if msg.contains('?') {
        return true;
    }
    let head = msg.trim_start().to_lowercase();
    EN_QUESTION_STARTERS.iter().any(|q| {
        head.starts_with(q)
            && !head[q.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
    })
}

/// Confidence for deterministic forget candidates.
const FORGET_CONFIDENCE: f32 = 0.90;

/// Builds a deletion-request candidate from a forget pattern match.
fn forget_candidate(user_msg: &str, target: &str) -> MemoryCandidate {
    let title_trunc: String = target.chars().take(20).collect();
    MemoryCandidate {
        kind: MemoryKind::Semantic,
        title: format!("forget: {title_trunc}"),
        content: format!("User requested to forget: {target}"),
        source_quote: user_msg.to_string(),
        confidence: FORGET_CONFIDENCE,
        should_persist: false,
        deletion_target_key: Some(target.to_string()),
        commitment_due: None,
        tags: Vec::new(),
    }
}

/// Matches forget patterns from `patterns` against `user_msg`, returning the
/// candidate of the **first** matching pattern in pack order. This preserves
/// the historical semantics of the hand-written matchers, where the
/// object-before-keyword pattern (`Xを忘れて`) took precedence over the
/// keyword-first pattern (`忘れて X`). A pattern is skipped when the message
/// reads as a question and the pattern opts into the question guard. Invalid
/// regexes are logged and skipped so a bad pack entry cannot break extraction.
fn match_forget_patterns(user_msg: &str, patterns: &PatternLibrary) -> Option<MemoryCandidate> {
    for pattern in patterns.forget_patterns() {
        if pattern.skip_questions && is_en_question(user_msg) {
            continue;
        }
        let re = match Regex::new(&pattern.regex) {
            Ok(re) => re,
            Err(error) => {
                tracing::warn!(
                    name = %pattern.name,
                    error = %error,
                    "invalid forget pattern regex; skipping pattern"
                );
                continue;
            }
        };
        let Some(caps) = re.captures(user_msg) else {
            continue;
        };
        let Some(group) = caps.get(pattern.target_group) else {
            tracing::warn!(
                name = %pattern.name,
                target_group = pattern.target_group,
                "forget pattern has no such capture group; skipping pattern"
            );
            continue;
        };
        let target = group.as_str().trim();
        if target.is_empty() {
            continue;
        }
        return Some(forget_candidate(user_msg, target));
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extract memory candidates deterministically from a conversation turn.
///
/// Matching is limited to explicit forget instructions driven by the language
/// pack for `locale` (see [`ene_config::PatternLibrary`]); languages without a
/// pack fall back to English patterns. Explicit remember requests are owned by
/// the LLM extractor and are not pattern-matched. Soft signals (preferences,
/// schedules, nicknames, …) are not pattern-matched either. Tool-result
/// grounding is applied when enabled via [`ToolGroundingConfig`].
///
/// Candidates are deduplicated by (title, kind) across pattern and
/// tool-grounding hits.
pub fn extract(
    turn: &TurnInput<'_>,
    locale: &Locale,
    min_confidence: f32,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    extract_with_tool_grounding(
        turn,
        locale,
        min_confidence,
        &ToolGroundingConfig::default(),
    )
}

/// Same as [`extract`], but with explicit tool-grounding configuration.
pub fn extract_with_tool_grounding(
    turn: &TurnInput<'_>,
    locale: &Locale,
    min_confidence: f32,
    tool_grounding_cfg: &ToolGroundingConfig,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    let user_norm = nfkc(turn.user_message);

    let mut candidates: Vec<MemoryCandidate> = Vec::new();

    // Forget-pattern matcher (language-pack driven; falls back to English
    // patterns for languages without a pack).
    let patterns = PatternLibrary::load(locale.code());
    if let Some(candidate) = match_forget_patterns(&user_norm, &patterns) {
        candidates.push(candidate);
    }

    // Tool-result matcher (always applied when enabled).
    candidates.extend(tool_grounding::extract_tool_candidates(
        turn.tool_results,
        tool_grounding_cfg,
    ));

    // Filter by min_confidence and deduplicate by (title, kind)
    let mut seen = std::collections::HashSet::new();
    let filtered: Vec<MemoryCandidate> = candidates
        .into_iter()
        .filter(|c| {
            if c.source_quote.is_empty() {
                c.confidence >= tool_grounding_cfg.min_confidence
            } else {
                c.confidence >= min_confidence
            }
        })
        .filter(|c| {
            let key = (c.title.clone(), c.kind, c.should_persist);
            seen.insert(key)
        })
        .collect();

    Ok(filtered)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::indexing_slicing,
        reason = "unit/integration tests use unwrap/expect and fixed indices for concise assertions"
    )
)]
mod tests {
    use super::super::candidate::ToolResultSummary;
    use super::*;

    fn ja_turn(msg: &str) -> TurnInput<'_> {
        TurnInput {
            user_message: msg,
            assistant_message: None,
            tool_results: &[],
        }
    }

    fn en_turn(msg: &str) -> TurnInput<'_> {
        TurnInput {
            user_message: msg,
            assistant_message: None,
            tool_results: &[],
        }
    }

    fn empty_turn() -> TurnInput<'static> {
        TurnInput {
            user_message: "",
            assistant_message: None,
            tool_results: &[],
        }
    }

    // ── Remember is LLM-owned (#354) ──────────────────────────────────

    #[test]
    fn remember_is_no_longer_detected_deterministically() {
        for (msg, locale) in [
            (
                "覚えて: プロジェクトXの話をしている",
                &Locale::resolve("ja"),
            ),
            ("私の誕生日を覚えておいて", &Locale::resolve("ja")),
            (
                "Please remember that I have a meeting tomorrow",
                &Locale::resolve("en"),
            ),
        ] {
            let out = extract(&ja_turn(msg), locale, 0.0)
                .expect("deterministic extraction always succeeds");
            assert!(
                out.is_empty(),
                "explicit remember must be LLM-owned, not pattern-matched: {msg:?} -> {out:?}"
            );
        }
    }

    #[test]
    fn ja_teach_request_is_not_remembered() {
        let out = extract(&ja_turn("今日の天気を教えて"), &Locale::resolve("ja"), 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "教えて must not be captured: {out:?}");
    }

    // ── Japanese forget ───────────────────────────────────────────────

    #[test]
    fn ja_forget_request_creates_deletion_candidate() {
        let out = extract(
            &ja_turn("さっきのプロジェクトを忘れて"),
            &Locale::resolve("ja"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(!out[0].should_persist);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("さっきのプロジェクト")
        );
    }

    #[test]
    fn ja_forget_after_keyword_captures_content() {
        let out = extract(
            &ja_turn("忘れて プロジェクトの話"),
            &Locale::resolve("ja"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("プロジェクトの話")
        );
    }

    #[test]
    fn ja_forget_question_still_matches() {
        // Japanese forget patterns do not opt into the English question guard,
        // so a question-marked forget request is still applied.
        let out = extract(
            &ja_turn("さっきの話を忘れて？"),
            &Locale::resolve("ja"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].deletion_target_key.as_deref(), Some("さっきの話"));
    }

    #[test]
    fn fullwidth_colon_still_matches() {
        let out = extract(
            &ja_turn("忘れて：全角コロンテスト"),
            &Locale::resolve("ja"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("全角コロンテスト")
        );
    }

    #[test]
    fn soft_signals_are_not_pattern_matched() {
        for msg in [
            "好き: 猫",
            "さゆりって呼んで",
            "もう提案しないで",
            "あとで X を確認する",
            "今日はこのアプリeneの進捗報告をします。メリットを教えて",
        ] {
            let out = extract(&ja_turn(msg), &Locale::resolve("ja"), 0.0)
                .expect("deterministic extraction always succeeds");
            assert!(
                out.is_empty(),
                "soft signal must not match pattern: {msg:?} -> {out:?}"
            );
        }
    }

    // ── English forget ────────────────────────────────────────────────

    #[test]
    fn en_forget_request_creates_deletion_candidate() {
        let out = extract(
            &en_turn("Forget about my ex-girlfriend"),
            &Locale::resolve("en"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert!(!out[0].should_persist);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("my ex-girlfriend")
        );
    }

    #[test]
    fn en_forget_question_is_skipped() {
        let out = extract(
            &en_turn("did you forget my name?"),
            &Locale::resolve("en"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "forget question must not match: {out:?}");
    }

    #[test]
    fn en_soft_signals_are_not_pattern_matched() {
        for msg in [
            "I like mushrooms",
            "call me Alex",
            "Next time, let's discuss the design",
            "today I have a presentation about ene",
        ] {
            let out = extract(&en_turn(msg), &Locale::resolve("en"), 0.0)
                .expect("deterministic extraction always succeeds");
            assert!(
                out.is_empty(),
                "soft signal must not match pattern: {msg:?} -> {out:?}"
            );
        }
    }

    // ── Language fallback (#354) ──────────────────────────────────────

    #[test]
    fn language_without_pack_falls_back_to_english_patterns() {
        // "zh" has no pattern pack: the loader falls back to English patterns,
        // so English forget requests still work for zh users.
        let out = extract(
            &en_turn("Forget about my ex-girlfriend"),
            &Locale::resolve("zh"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("my ex-girlfriend")
        );
    }

    #[test]
    fn jp_alias_resolves_to_ja_patterns() {
        let out = extract(
            &ja_turn("さっきのプロジェクトを忘れて"),
            &Locale::resolve("jp"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("さっきのプロジェクト")
        );
    }

    // ── Tool procedure ────────────────────────────────────────────────

    #[test]
    fn tool_success_extracts_procedure() {
        let tools = vec![ToolResultSummary {
            tool_name: "fs".to_string(),
            success: true,
            summary: "wrote file.txt".to_string(),
        }];
        let turn = TurnInput {
            user_message: "",
            assistant_message: None,
            tool_results: &tools,
        };
        let cfg = ToolGroundingConfig {
            persist_success_procedure: true,
            ..ToolGroundingConfig::default()
        };
        let out = extract_with_tool_grounding(&turn, &Locale::resolve("ja"), 0.0, &cfg)
            .expect("deterministic extraction always succeeds");
        assert!(out.iter().any(|c| c.kind == MemoryKind::Procedure));
        assert!(out.iter().any(|c| c.content.contains("fs")));
    }

    #[test]
    fn tool_success_skipped_by_default() {
        let tools = vec![ToolResultSummary {
            tool_name: "fs".to_string(),
            success: true,
            summary: "wrote file.txt".to_string(),
        }];
        let turn = TurnInput {
            user_message: "",
            assistant_message: None,
            tool_results: &tools,
        };
        let out = extract_with_tool_grounding(
            &turn,
            &Locale::resolve("ja"),
            0.0,
            &ToolGroundingConfig::default(),
        )
        .expect("deterministic extraction always succeeds");
        assert!(
            out.is_empty(),
            "default tool grounding must not auto-persist successes: {out:?}"
        );
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        let out = extract(&empty_turn(), &Locale::resolve("ja"), 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn japanese_message_with_english_locale_returns_empty() {
        let out = extract(&ja_turn("さっきの話を忘れて"), &Locale::resolve("en"), 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "en patterns must not match Japanese text");
    }

    #[test]
    fn no_pattern_match_returns_empty() {
        let out = extract(&ja_turn("今日の天気は？"), &Locale::resolve("ja"), 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn confidence_above_threshold_passes() {
        let out = extract(&ja_turn("さっきの話を忘れて"), &Locale::resolve("ja"), 0.80)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert!(out[0].confidence >= 0.80);
    }

    #[test]
    fn does_not_extract_from_assistant_message() {
        let turn = TurnInput {
            user_message: "hello",
            assistant_message: Some("忘れて: 私の秘密"),
            tool_results: &[],
        };
        let out = extract(&turn, &Locale::resolve("ja"), 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn first_matching_pattern_wins() {
        // "Xを忘れて Y" matches both Japanese patterns; the object-before-
        // keyword pattern comes first in the pack and takes precedence (the
        // historical RE_WO early-return semantics).
        let out = extract(
            &ja_turn("プロジェクトXを忘れて 明日の話"),
            &Locale::resolve("ja"),
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("プロジェクトX"),
            "RE_WO (object before keyword) must win over RE_AFTER"
        );
    }
}
