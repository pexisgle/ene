//! Deterministic memory extractor — pattern-based, no LLM required.
//!
//! Extracts `MemoryCandidate`s from conversation turns using regex patterns
//! and keyword matching. Supports Japanese and English.
//!
//! Confidence calibration:
//! - Explicit「覚えて」: 0.85
//! - Forget (deletion request): 0.90
//! - Like/dislike: 0.65
//! - Nickname: 0.75
//! - NG instruction: 0.80
//! - Commitment: 0.50
//! - Tool-grounded procedure/reflection/episodic: 0.60–0.70
//! - Soft signals (implicit): 0.30–0.50

#![allow(clippy::unwrap_used)]

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

/// Trim a captured Japanese callsign down to the bare name.
///
/// Drops everything up to and including the last topic/subject particle
/// (`は` / `が`) so `ボクはレン` → `レン` and `私がミク` → `ミク`; if no
/// particle is present, a leading first-person pronoun is stripped so
/// `わたしレン` → `レン` (issue #70). Trimming is skipped when it would
/// leave an empty callsign, so short names are not over-truncated.
fn trim_ja_callsign(name: &str) -> &str {
    let mut token = name.trim();
    for particle in ["は", "が"] {
        if let Some(idx) = token.rfind(particle) {
            let rest = token[idx + particle.len()..].trim();
            if !rest.is_empty() {
                token = rest;
            }
        }
    }
    for pronoun in ["わたし", "私", "ボク", "僕", "俺", "自分"] {
        if let Some(rest) = token.strip_prefix(pronoun) {
            let rest = rest.trim();
            if !rest.is_empty() {
                token = rest;
            }
        }
    }
    token
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
    static RE_WO: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(.+?)を(覚えて|記憶して|メモして|保存して)").unwrap()
    });
    // Explicit colon annotation `覚えて: X`, where the content follows the
    // keyword. The colon is required so a trailing verb continuation such as
    // `覚えておいて` is not mis-captured as content (`おいて`).
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
    static RE_WO: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(.+?)を(忘れて|消して|削除して|やめて)").unwrap());
    // Pattern 2: keyword + content (e.g., "忘れて プロジェクトの話")
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

fn ja_preference(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(好き|嫌い|苦手|大好き|大嫌い)[:：]?\s*(.+)").unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let pref = cap[2].trim().to_string();
        let title_trunc: String = pref.chars().take(20).collect();
        MemoryCandidate {
            kind: MemoryKind::Preference,
            title: format!("{}: {}", &cap[1], title_trunc),
            content: pref,
            source_quote: user_msg.to_string(),
            confidence: 0.65,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        }
    })
}

fn ja_nickname(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(.+?)って呼んで").unwrap());
    RE.captures(user_msg).and_then(|cap| {
        let name = trim_ja_callsign(cap[1].trim()).to_string();
        if name.is_empty() {
            return None;
        }
        Some(MemoryCandidate {
            kind: MemoryKind::UserProfile,
            title: format!("呼び方: {name}"),
            content: format!("User wants to be called '{name}'"),
            source_quote: user_msg.to_string(),
            confidence: 0.75,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        })
    })
}

fn ja_ng_instruction(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(もう|これ以上|二度と).{0,6}(しないで|やめて|やめろ|禁止)").unwrap()
    });
    RE.captures(user_msg).map(|_cap| MemoryCandidate {
        kind: MemoryKind::Procedure,
        title: "NG instruction".to_string(),
        content: user_msg.to_string(),
        source_quote: user_msg.to_string(),
        confidence: 0.80,
        should_persist: true,
        deletion_target_key: None,
        commitment_due: None,
    })
}

fn ja_commitment(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(あとで|次回|明日|来週|今度|今夜).{0,10}(して|確認|やる|話|連絡)").unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let due = cap[1].to_string();
        MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: format!("commitment: {due}"),
            content: user_msg.to_string(),
            source_quote: user_msg.to_string(),
            confidence: 0.50,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: Some(due),
        }
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

fn en_preference(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(I\s+(?:really\s+)?(?:don't\s+like|hate|dislike|love|like|prefer))\s+(.+)")
            .unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let pref = cap[0].to_string();
        MemoryCandidate {
            kind: MemoryKind::Preference,
            title: format!("{}...", pref.chars().take(30).collect::<String>()),
            content: pref,
            source_quote: user_msg.to_string(),
            confidence: 0.65,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        }
    })
}

fn en_nickname(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#"(?i)(?:call|refer to|address)\s+me\s+(?:as\s+)?["']?([^"',.]+?)["']?\s*$"#)
            .unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let name = cap[1].trim().to_string();
        MemoryCandidate {
            kind: MemoryKind::UserProfile,
            title: format!("nickname: {name}"),
            content: format!("User wants to be called '{name}'"),
            source_quote: user_msg.to_string(),
            confidence: 0.75,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        }
    })
}

fn en_ng_instruction(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)(never|don't|stop|no\s+more)\s+(do\s+that|say\s+that|mention|suggest|recommend)",
        )
        .unwrap()
    });
    RE.captures(user_msg).map(|_cap| MemoryCandidate {
        kind: MemoryKind::Procedure,
        title: "NG instruction".to_string(),
        content: user_msg.to_string(),
        source_quote: user_msg.to_string(),
        confidence: 0.80,
        should_persist: true,
        deletion_target_key: None,
        commitment_due: None,
    })
}

fn en_commitment(
    user_msg: &str,
    _asst_msg: &str,
    _tool_results: &[ToolResultSummary],
) -> Option<MemoryCandidate> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)(next\s+time|later|tomorrow|next\s+week|tonight)\s*[,.]?\s*(?:let'?s|we\s+should|I'?ll|can\s+you)\s+(.+)").unwrap()
    });
    RE.captures(user_msg).map(|cap| {
        let due = cap[1].to_string();
        MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: format!("commitment: {due}"),
            content: user_msg.to_string(),
            source_quote: user_msg.to_string(),
            confidence: 0.50,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: Some(due),
        }
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// All Japanese message matchers.
const JA_MATCHERS: &[PatternMatcher] = &[
    ja_explicit_remember,
    ja_forget_request,
    ja_preference,
    ja_nickname,
    ja_ng_instruction,
    ja_commitment,
];

/// All English message matchers.
const EN_MATCHERS: &[PatternMatcher] = &[
    en_explicit_remember,
    en_forget_request,
    en_preference,
    en_nickname,
    en_ng_instruction,
    en_commitment,
];

/// Extract memory candidates deterministically from a conversation turn.
///
/// Pattern matching is locale-aware: `Locale::Ja` applies Japanese patterns,
/// `Locale::En` applies English patterns. The tool-result matcher is always applied.
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
#[cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
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
        .expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(out[0].should_persist);
        assert!(out[0].content.contains("プロジェクトX"));
    }

    #[test]
    fn ja_remember_captures_object_before_keyword() {
        // Japanese puts the object before the verb; the content is what
        // precedes を覚えて, not the trailing verb continuation `おいて`
        // (issue #70 precision guard).
        let out =
            extract(&ja_turn("私の誕生日を覚えておいて"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(
            out[0].content.contains("私の誕生日") && !out[0].content.contains("おいて"),
            "expected content 私の誕生日 without おいて, got: {out:?}"
        );
    }

    #[test]
    fn ja_teach_request_is_not_remembered() {
        // 教えて ("tell me") is a request for information, not a memory
        // instruction, so it must not produce a candidate (issue #70).
        let out = extract(&ja_turn("今日の天気を教えて"), Locale::Ja, 0.0).expect("extract failed");
        assert!(out.is_empty(), "教えて must not be captured: {out:?}");
    }

    #[test]
    fn ja_remember_question_is_skipped() {
        // A question about memory must not be captured as an instruction.
        let out =
            extract(&ja_turn("私の誕生日を覚えてますか"), Locale::Ja, 0.0).expect("extract failed");
        assert!(out.is_empty(), "remember question must not match: {out:?}");
    }

    #[test]
    fn ja_forget_request_creates_deletion_candidate() {
        let out = extract(&ja_turn("さっきのプロジェクトを忘れて"), Locale::Ja, 0.0)
            .expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(!out[0].should_persist);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("さっきのプロジェクト")
        );
    }

    // ── Japanese preference ───────────────────────────────────────────

    #[test]
    fn ja_preference_like() {
        let out = extract(&ja_turn("好き: 猫"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Preference);
        assert_eq!(out[0].confidence, 0.65);
    }

    #[test]
    fn ja_preference_dislike() {
        let out = extract(&ja_turn("嫌い: 納豆"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Preference);
    }

    // ── Japanese nickname ─────────────────────────────────────────────

    #[test]
    fn ja_nickname() {
        let out = extract(&ja_turn("さゆりって呼んで"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::UserProfile);
        assert_eq!(out[0].confidence, 0.75);
        assert!(out[0].title.contains("さゆり"));
    }

    #[test]
    fn ja_nickname_strips_topic_marker() {
        // The topic marker + first-person pronoun must be dropped so the
        // captured callsign is レン, not ボクはレン (issue #70).
        let out =
            extract(&ja_turn("ボクはレンって呼んで"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].title.contains("レン") && !out[0].title.contains("ボク"),
            "expected callsign レン without ボクは prefix, got: {out:?}"
        );
    }

    // ── Japanese NG ───────────────────────────────────────────────────

    #[test]
    fn ja_ng_instruction() {
        let out = extract(&ja_turn("もう提案しないで"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Procedure);
        assert_eq!(out[0].confidence, 0.80);
    }

    // ── Japanese commitment ───────────────────────────────────────────

    #[test]
    fn ja_commitment() {
        let out =
            extract(&ja_turn("あとで X を確認する"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Commitment);
        assert!(out[0].commitment_due.is_some());
    }

    // ── English remember ──────────────────────────────────────────────

    #[test]
    fn en_explicit_remember_pickup() {
        let out = extract(
            &en_turn("Please remember that I have a meeting tomorrow"),
            Locale::En,
            0.0,
        )
        .expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Semantic);
        assert!(out[0].content.contains("meeting"));
    }

    #[test]
    fn indirect_question_without_mark_is_skipped() {
        // No trailing '?' but clearly a question — must not be captured
        // as a remember instruction.
        let out = extract(&en_turn("do you remember my birthday"), Locale::En, 0.0)
            .expect("extract failed");
        assert!(out.is_empty(), "indirect question must not match: {out:?}");
    }

    #[test]
    fn imperative_remember_still_matches() {
        // Guard against the question detector being too aggressive.
        let out = extract(
            &en_turn("please remember that I take my coffee black"),
            Locale::En,
            0.0,
        )
        .expect("extract failed");
        assert_eq!(out.len(), 1, "imperative remember must still match");
    }

    #[test]
    fn en_remember_empty_content_is_rejected() {
        // A remember keyword with only trailing whitespace must not produce
        // a persistable candidate with empty content (Bugbot medium finding).
        let out =
            extract(&en_turn("please remember    "), Locale::En, 0.0).expect("extract failed");
        assert!(
            out.is_empty(),
            "empty remember content must not match: {out:?}"
        );
    }

    // ── English forget ────────────────────────────────────────────────

    #[test]
    fn en_forget_request_creates_deletion_candidate() {
        let out = extract(&en_turn("Forget about my ex-girlfriend"), Locale::En, 0.0)
            .expect("extract failed");
        assert_eq!(out.len(), 1);
        assert!(!out[0].should_persist);
        assert_eq!(
            out[0].deletion_target_key.as_deref(),
            Some("my ex-girlfriend")
        );
    }

    // ── English preference ────────────────────────────────────────────

    #[test]
    fn en_preference_like() {
        let out = extract(&en_turn("I like mushrooms"), Locale::En, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Preference);
    }

    #[test]
    fn en_preference_dislike() {
        let out =
            extract(&en_turn("I don't like mushrooms"), Locale::En, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Preference);
    }

    // ── English nickname ──────────────────────────────────────────────

    #[test]
    fn en_nickname() {
        let out = extract(&en_turn("call me Alex"), Locale::En, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::UserProfile);
        assert!(out[0].title.contains("Alex"));
    }

    // ── English commitment ────────────────────────────────────────────

    #[test]
    fn en_commitment() {
        let out = extract(
            &en_turn("Next time, let's discuss the design"),
            Locale::En,
            0.0,
        )
        .expect("extract failed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MemoryKind::Commitment);
        assert_eq!(out[0].commitment_due.as_deref(), Some("Next time"));
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
        let out = extract(&turn, Locale::Ja, 0.0).expect("extract failed");
        assert!(out.iter().any(|c| c.kind == MemoryKind::Procedure));
        assert!(out.iter().any(|c| c.content.contains("fs")));
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        let out = extract(&empty_turn(), Locale::Ja, 0.0).expect("extract failed");
        assert!(out.is_empty());
    }

    #[test]
    fn locale_mismatch_returns_empty() {
        let out = extract(&ja_turn("覚えて: test"), Locale::En, 0.0).expect("extract failed");
        assert!(out.is_empty());
    }

    #[test]
    fn no_pattern_match_returns_empty() {
        let out = extract(&ja_turn("今日の天気は？"), Locale::Ja, 0.0).expect("extract failed");
        assert!(out.is_empty());
    }

    // ── Confidence filtering ──────────────────────────────────────────

    #[test]
    fn respects_min_confidence_threshold() {
        let out = extract(&ja_turn("好き: 猫"), Locale::Ja, 0.70).expect("extract failed");
        assert!(
            out.is_empty(),
            "confidence 0.65 should be below threshold 0.70"
        );
    }

    #[test]
    fn confidence_above_threshold_passes() {
        let out =
            extract(&ja_turn("覚えて: 重要なこと"), Locale::Ja, 0.80).expect("extract failed");
        assert_eq!(out.len(), 1);
        assert!(out[0].confidence >= 0.80);
    }

    // ── Assistant message not matched ──────────────────────────────────

    #[test]
    fn does_not_extract_from_assistant_message() {
        let turn = TurnInput {
            user_message: "hello",
            assistant_message: Some("覚えて: 私の秘密"),
            tool_results: &[],
        };
        let out = extract(&turn, Locale::Ja, 0.0).expect("extract failed");
        assert!(out.is_empty());
    }

    // ── Deduplication ─────────────────────────────────────────────────

    #[test]
    fn deduplicates_by_title_and_kind() {
        let msg = "覚えて: プロジェクトX";
        let out = extract(&ja_turn(msg), Locale::Ja, 0.0).expect("extract failed");
        // Should not duplicate even though the same pattern fires
        assert_eq!(out.len(), 1);
    }

    // ── Fullwidth / mixed input ───────────────────────────────────────

    #[test]
    fn fullwidth_colon_still_matches() {
        let out =
            extract(&ja_turn("覚えて：全角コロンテスト"), Locale::Ja, 0.0).expect("extract failed");
        assert_eq!(out.len(), 1);
    }

    // ── Min confidence default (from config) ──────────────────────────

    #[test]
    fn default_min_confidence_is_065() {
        let out = extract(&ja_turn("好き: 猫"), Locale::Ja, 0.65).expect("extract failed");
        assert_eq!(
            out.len(),
            1,
            "confidence 0.65 should pass at threshold 0.65"
        );
    }
}
