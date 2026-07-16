//! Deterministic memory extractor — minimal pattern safety net.
//!
//! Only explicit remember / forget instructions are pattern-matched.
//! Preferences, schedules, nicknames, and other soft signals are left to the
//! LLM extractor. Tool-result grounding remains a separate, configurable path.
//!
//! Confidence calibration:
//! - Explicit「覚えて」 / remember: 0.85
//! - Forget (deletion request): 0.90
//! - Tool-grounded procedure/reflection/episodic: 0.60–0.70 (when enabled)

use regex::Regex;

use super::candidate::{Locale, MemoryCandidate, ToolResultSummary, TurnInput};
use super::tool_grounding;
use crate::config::ToolGroundingConfig;
use crate::error::CognitionError;
use ene_store::typed_memory::MemoryKind;

/// Normalise Unicode with NFKC (fullwidth → ASCII, combined → single).
fn nfkc(s: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfkc(s).collect()
}

/// Interrogative sentence openers (lowercase). A `remember` / `forget`
/// keyword inside a question is not an instruction — e.g.
/// "do you remember my birthday" (issue #70 precision guard).
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

/// Return `true` when `msg` reads as a Japanese question rather than an
/// instruction: it contains a `?` (the fullwidth `？` is already folded
/// to ASCII by [`nfkc`]) or ends with the interrogative particle `か`
/// (covers `〜ますか` / `〜ですか` / `覚えてるか`). This prevents phrasing
/// like `私の誕生日を覚えてますか` from being captured as a remember
/// instruction (issue #70 precision guard).
fn is_ja_question(msg: &str) -> bool {
    let trimmed = msg.trim();
    trimmed.contains('?') || trimmed.ends_with('か')
}

// ---------------------------------------------------------------------------
// Pattern matcher type
// ---------------------------------------------------------------------------

type PatternMatcher = fn(&str, &str, &[ToolResultSummary]) -> Option<MemoryCandidate>;

// ---------------------------------------------------------------------------
// Japanese patterns
// ---------------------------------------------------------------------------

fn ja_explicit_remember(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    // Questions such as `私の誕生日を覚えてますか` are requests, not
    // instructions, and must not be captured (issue #70 precision guard).
    if is_ja_question(user_msg) {
        return None;
    }
    // Japanese places the object before the verb, so the memorable content
    // precedes the keyword: `X を覚えて(おいて)`. Capturing group 1 is the
    // object. `教えて` ("tell me") is deliberately excluded because it is a
    // request for information, not a memory instruction.
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE_WO: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(.+?)を(覚えて|記憶して|メモして|保存して)").unwrap()
    });
    // Explicit colon annotation `覚えて: X`, where the content follows the
    // keyword. The colon is required so a trailing verb continuation such as
    // `覚えておいて` is not mis-captured as content (`おいて`).
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE_COLON: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(覚えて|記憶して|メモして|保存して)[:：]\s*(.+)").unwrap()
    });

    let content = RE_WO
        .captures(user_msg)
        .map(|cap| cap[1].trim().to_string())
        .or_else(|| {
            RE_COLON
                .captures(user_msg)
                .map(|cap| cap[2].trim().to_string())
        })?;

    if content.is_empty() {
        return None;
    }
    let title: String = content.chars().take(20).collect();
    Some(MemoryCandidate {
        kind: MemoryKind::Semantic,
        title: format!("{title}..."),
        content,
        source_quote: user_msg.to_string(),
        confidence: 0.85,
        should_persist: true,
        deletion_target_key: None,
        commitment_due: None,
    })
}

fn ja_forget_request(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    // Pattern 1: content + を + keyword (e.g., "プロジェクトを忘れて")
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE_WO: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(.+?)を(忘れて|消して|削除して|やめて)").unwrap());
    // Pattern 2: keyword + content (e.g., "忘れて プロジェクトの話")
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE_AFTER: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(忘れて|消して|削除して|やめて)[:：]?\s*(.+)").unwrap()
    });

    if let Some(cap) = RE_WO.captures(user_msg) {
        let target = cap[1].trim().to_string();
        let title_trunc: String = target.chars().take(20).collect();
        return Some(MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: format!("forget: {title_trunc}"),
            content: format!("User requested to forget: {target}"),
            source_quote: user_msg.to_string(),
            confidence: 0.90,
            should_persist: false,
            deletion_target_key: Some(target),
            commitment_due: None,
        });
    }

    RE_AFTER.captures(user_msg).and_then(|cap| {
        let after = cap[2].trim();
        if after.is_empty() {
            return None;
        }
        let target = after.to_string();
        let title_trunc: String = target.chars().take(20).collect();
        Some(MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: format!("forget: {title_trunc}"),
            content: format!("User requested to forget: {target}"),
            source_quote: user_msg.to_string(),
            confidence: 0.90,
            should_persist: false,
            deletion_target_key: Some(target),
            commitment_due: None,
        })
    })
}

// ---------------------------------------------------------------------------
// English patterns
// ---------------------------------------------------------------------------

fn en_explicit_remember(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    if is_en_question(user_msg) {
        return None;
    }
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)(please\s+)?(remember|note|keep in mind|don't forget)[:：]?\s+(?:that\s+)?(.+)",
        )
        .unwrap()
    });
    RE.captures(user_msg).and_then(|cap| {
        let content = cap[3].trim().to_string();
        if content.is_empty() {
            return None;
        }
        Some(MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: format!("{}...", content.chars().take(20).collect::<String>()),
            content,
            source_quote: user_msg.to_string(),
            confidence: 0.85,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        })
    })
}

fn en_forget_request(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    if is_en_question(user_msg) {
        return None;
    }
    #[expect(clippy::unwrap_used, reason = "constant regex pattern")]
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(forget|erase|drop|stop remembering)\s+(?:about\s+)?(.+)").unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let target = cap[2].trim().to_string();
        MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: format!("forget: {}", target.chars().take(20).collect::<String>()),
            content: format!("User requested to forget: {target}"),
            source_quote: user_msg.to_string(),
            confidence: 0.90,
            should_persist: false,
            deletion_target_key: Some(target),
            commitment_due: None,
        }
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Japanese message matchers: explicit remember / forget only.
const JA_MATCHERS: &[PatternMatcher] = &[ja_explicit_remember, ja_forget_request];

/// English message matchers: explicit remember / forget only.
const EN_MATCHERS: &[PatternMatcher] = &[en_explicit_remember, en_forget_request];

/// Extract memory candidates deterministically from a conversation turn.
///
/// Pattern matching is locale-aware and limited to explicit remember / forget
/// instructions. Soft signals (preferences, schedules, nicknames, …) are not
/// pattern-matched — the LLM extractor owns those. Tool-result grounding is
/// applied when enabled via [`ToolGroundingConfig`].
///
/// Candidates are deduplicated by (title, kind) when multiple matchers fire on
/// the same message. Only the first match (by matcher order) is kept.
pub fn extract(
    turn: &TurnInput<'_>,
    locale: Locale,
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
    locale: Locale,
    min_confidence: f32,
    tool_grounding_cfg: &ToolGroundingConfig,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    let user_norm = nfkc(turn.user_message);
    let asst_norm = turn.assistant_message.map(nfkc).unwrap_or_default();

    let mut candidates: Vec<MemoryCandidate> = Vec::new();

    // Message matchers
    let matchers = match locale {
        Locale::Ja => JA_MATCHERS,
        Locale::En => EN_MATCHERS,
    };
    for matcher in matchers {
        if let Some(c) = matcher(&user_norm, &asst_norm, turn.tool_results) {
            candidates.push(c);
        }
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
        reason = "unit/integration tests use unwrap/expect for concise assertions"
    )
)]
mod tests {
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

    // ── Japanese remember ─────────────────────────────────────────────

    #[test]
    fn ja_explicit_remember_pickup() {
        let out = extract(
            &ja_turn("覚えて: プロジェクトXの話をしている"),
            Locale::Ja,
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(out[0].should_persist);
        assert!(out[0].content.contains("プロジェクトX"));
    }

    #[test]
    fn ja_remember_captures_object_before_keyword() {
        let out = extract(&ja_turn("私の誕生日を覚えておいて"), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(
            out[0].content.contains("私の誕生日") && !out[0].content.contains("おいて"),
            "expected content 私の誕生日 without おいて, got: {out:?}"
        );
    }

    #[test]
    fn ja_teach_request_is_not_remembered() {
        let out = extract(&ja_turn("今日の天気を教えて"), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "教えて must not be captured: {out:?}");
    }

    #[test]
    fn ja_remember_question_is_skipped() {
        let out = extract(&ja_turn("私の誕生日を覚えてますか"), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "remember question must not match: {out:?}");
    }

    #[test]
    fn ja_forget_request_creates_deletion_candidate() {
        let out = extract(&ja_turn("さっきのプロジェクトを忘れて"), Locale::Ja, 0.0)
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
    fn soft_signals_are_not_pattern_matched() {
        for msg in [
            "好き: 猫",
            "さゆりって呼んで",
            "もう提案しないで",
            "あとで X を確認する",
            "今日はこのアプリeneの進捗報告をします。メリットを教えて",
        ] {
            let out = extract(&ja_turn(msg), Locale::Ja, 0.0)
                .expect("deterministic extraction always succeeds");
            assert!(
                out.is_empty(),
                "soft signal must not match pattern: {msg:?} -> {out:?}"
            );
        }
    }

    // ── English remember ──────────────────────────────────────────────

    #[test]
    fn en_explicit_remember_pickup() {
        let out = extract(
            &en_turn("Please remember that I have a meeting tomorrow"),
            Locale::En,
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(out[0].content.contains("meeting"));
    }

    #[test]
    fn indirect_question_without_mark_is_skipped() {
        let out = extract(&en_turn("do you remember my birthday"), Locale::En, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty(), "indirect question must not match: {out:?}");
    }

    #[test]
    fn imperative_remember_still_matches() {
        let out = extract(
            &en_turn("please remember that I take my coffee black"),
            Locale::En,
            0.0,
        )
        .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1, "imperative remember must still match");
    }

    #[test]
    fn en_remember_empty_content_is_rejected() {
        let out = extract(&en_turn("please remember    "), Locale::En, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(
            out.is_empty(),
            "empty remember content must not match: {out:?}"
        );
    }

    #[test]
    fn en_forget_request_creates_deletion_candidate() {
        let out = extract(&en_turn("Forget about my ex-girlfriend"), Locale::En, 0.0)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert!(!out[0].should_persist);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("my ex-girlfriend")
        );
    }

    #[test]
    fn en_soft_signals_are_not_pattern_matched() {
        for msg in [
            "I like mushrooms",
            "call me Alex",
            "Next time, let's discuss the design",
            "today I have a presentation about ene",
        ] {
            let out = extract(&en_turn(msg), Locale::En, 0.0)
                .expect("deterministic extraction always succeeds");
            assert!(
                out.is_empty(),
                "soft signal must not match pattern: {msg:?} -> {out:?}"
            );
        }
    }

    // ── Tool procedure ────────────────────────────────────────────────

    #[test]
    fn tool_success_extracts_procedure_when_enabled() {
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
            ..Default::default()
        };
        let out = extract_with_tool_grounding(&turn, Locale::Ja, 0.0, &cfg)
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
        let out =
            extract(&turn, Locale::Ja, 0.0).expect("deterministic extraction always succeeds");
        assert!(
            out.is_empty(),
            "default tool grounding must not auto-keep successes: {out:?}"
        );
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        let out = extract(&empty_turn(), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn locale_mismatch_returns_empty() {
        let out = extract(&ja_turn("覚えて: test"), Locale::En, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn no_pattern_match_returns_empty() {
        let out = extract(&ja_turn("今日の天気は？"), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn confidence_above_threshold_passes() {
        let out = extract(&ja_turn("覚えて: 重要なこと"), Locale::Ja, 0.80)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
        assert!(out[0].confidence >= 0.80);
    }

    #[test]
    fn does_not_extract_from_assistant_message() {
        let turn = TurnInput {
            user_message: "hello",
            assistant_message: Some("覚えて: 私の秘密"),
            tool_results: &[],
        };
        let out =
            extract(&turn, Locale::Ja, 0.0).expect("deterministic extraction always succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn deduplicates_by_title_and_kind() {
        let msg = "覚えて: プロジェクトX";
        let out = extract(&ja_turn(msg), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn fullwidth_colon_still_matches() {
        let out = extract(&ja_turn("覚えて：全角コロンテスト"), Locale::Ja, 0.0)
            .expect("deterministic extraction always succeeds");
        assert_eq!(out.len(), 1);
    }
}
