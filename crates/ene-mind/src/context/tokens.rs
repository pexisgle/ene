//! Token estimation helpers for context budget management (#81).

/// Approximate characters per token for heuristic budgeting.
pub const CHARS_PER_TOKEN: usize = 4;

/// Estimate token count from text using the workspace heuristic.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(CHARS_PER_TOKEN).max(1)
}

// Language-aware estimation (#386). The flat [`CHARS_PER_TOKEN`] heuristic is
// tuned for English (~4 chars/token); Japanese packs roughly one token per
// 1–1.5 characters, so counting it at the English ratio under-counts a card by
// 3–4×. The weights below are expressed in twelfths of a token — the LCM of the
// 1/4 (Latin) and 2/3 (CJK, i.e. ~1.5 chars/token) ratios — so the estimate
// stays exact without floating point.

/// Twelfths-of-a-token weight for a Latin / non-CJK character (1/4 token).
const LATIN_TOKEN_UNITS: usize = 3;
/// Twelfths-of-a-token weight for a CJK character (2/3 token ≈ 1.5 chars/token).
const CJK_TOKEN_UNITS: usize = 8;
/// Whole-token divisor for the per-character weights above.
const TOKEN_UNITS: usize = 12;

/// Whether a character is counted at the denser CJK ratio (#386).
///
/// Covers Hiragana, Katakana, CJK ideographs (and their compatibility /
/// extension blocks), CJK punctuation, and fullwidth forms — the ranges a
/// Japanese character card is overwhelmingly composed of.
const fn is_cjk_token_char(c: char) -> bool {
    matches!(
        c,
        '\u{3000}'..='\u{303F}'
            | '\u{3040}'..='\u{309F}'
            | '\u{30A0}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
    )
}

/// Per-character token weight in twelfths of a token.
const fn token_units_for(c: char) -> usize {
    if is_cjk_token_char(c) {
        CJK_TOKEN_UNITS
    } else {
        LATIN_TOKEN_UNITS
    }
}

/// Estimate token count with a language-aware character ratio (#386).
///
/// Unlike [`estimate_tokens`], which assumes a flat [`CHARS_PER_TOKEN`] tuned
/// for English, this counts CJK characters at ~1.5 chars/token and everything
/// else at ~4 chars/token. A Japanese identity kernel therefore budgets against
/// a realistic token figure instead of one 3–4× too small.
pub fn estimate_tokens_language_aware(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let units = text
        .chars()
        .fold(0usize, |acc, c| acc.saturating_add(token_units_for(c)));
    units.div_ceil(TOKEN_UNITS).max(1)
}

/// Convert a token budget to an approximate character budget.
pub const fn tokens_to_chars(tokens: usize) -> usize {
    tokens.saturating_mul(CHARS_PER_TOKEN)
}

/// Truncate text to fit within a token budget (character heuristic).
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let max_chars = tokens_to_chars(max_tokens);
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_uses_char_heuristic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn truncate_to_tokens_limits_chars() {
        let text = "abcdefghij";
        assert_eq!(truncate_to_tokens(text, 1), "abcd");
    }

    #[test]
    fn language_aware_estimate_matches_flat_for_pure_english() {
        // Pure ASCII at exactly 4 chars/token agrees with the flat heuristic.
        assert_eq!(estimate_tokens_language_aware(""), 0);
        assert_eq!(estimate_tokens_language_aware("abcd"), 1);
        assert_eq!(estimate_tokens_language_aware("abcdefgh"), 2);
    }

    #[test]
    fn language_aware_estimate_counts_japanese_denser() {
        // 12 CJK chars at 2/3 token each = 8 tokens, versus the flat
        // heuristic's 12/4 = 3 — the under-count #386 corrects.
        let ja = "今日は良い天気ですね元気";
        assert_eq!(ja.chars().count(), 12);
        assert_eq!(estimate_tokens_language_aware(ja), 8);
        assert!(estimate_tokens_language_aware(ja) > estimate_tokens(ja));
    }

    #[test]
    fn language_aware_estimate_blends_mixed_text() {
        // "Rust" (4 Latin = 1 token) + "で書く" (3 CJK = 2 tokens) = 3 tokens.
        assert_eq!(estimate_tokens_language_aware("Rustで書く"), 3);
    }
}
