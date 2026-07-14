//! Token estimation helpers for context budget management (#81).

/// Approximate characters per token for heuristic budgeting.
pub const CHARS_PER_TOKEN: usize = 4;

/// Estimate token count from text using the workspace heuristic.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(CHARS_PER_TOKEN).max(1)
}

/// Convert a token budget to an approximate character budget.
#[must_use]
pub const fn tokens_to_chars(tokens: usize) -> usize {
    tokens.saturating_mul(CHARS_PER_TOKEN)
}

/// Truncate text to fit within a token budget (character heuristic).
#[must_use]
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
}
