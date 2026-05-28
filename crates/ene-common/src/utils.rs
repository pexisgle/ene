/// Truncates a string to a specified length (appends `...` if exceeded)
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_within_limit() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_at_limit() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_exceeds_limit() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_unicode() {
        let text = "こんにちは世界";
        assert_eq!(truncate(text, 3), "こんに...");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn test_truncate_zero_limit() {
        assert_eq!(truncate("hello", 0), "...");
    }
}
