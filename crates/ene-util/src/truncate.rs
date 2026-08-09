//! Smart content truncation helpers.

use std::collections::VecDeque;

/// Helper struct for high-performance and detailed string truncation.
pub struct Truncate;

/// Result of a truncation operation, indicating whether content was actually cut.
#[derive(Debug, Clone)]
pub struct TruncateResult {
    /// The truncated (or original) text content.
    pub content: String,
    /// Whether the content was actually truncated.
    pub truncated: bool,
}

impl Truncate {
    /// Simply truncates text to a maximum number of Unicode characters and appends `...` if exceeded.
    pub fn simple(text: &str, max_chars: usize) -> String {
        if text.chars().count() <= max_chars {
            text.to_string()
        } else {
            let end = text
                .char_indices()
                .nth(max_chars)
                .map_or(text.len(), |(i, _)| i);
            format!("{}...", text.get(..end).unwrap_or(text))
        }
    }

    /// Truncates text to a maximum number of Unicode characters and appends a detailed notice when cut.
    pub fn detailed(text: &str, max_chars: usize) -> String {
        let char_count = text.chars().count();
        if char_count <= max_chars {
            text.to_string()
        } else {
            let byte_end = text
                .char_indices()
                .nth(max_chars)
                .map_or(text.len(), |(i, _)| i);
            format!(
                "{}\n\n[... truncated, total {} chars ...]",
                text.get(..byte_end).unwrap_or(text),
                char_count
            )
        }
    }

    /// Alias for detailed truncation, used by tools.
    pub fn chars(text: &str, max_chars: usize) -> String {
        Self::detailed(text, max_chars)
    }

    /// Fits text within `max_lines` / `max_bytes` (head direction)
    pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let lines: Vec<&str> = text.lines().collect();
        let total_bytes = text.len();

        if lines.len() <= max_lines && total_bytes <= max_bytes {
            return TruncateResult {
                content: text.to_string(),
                truncated: false,
            };
        }

        let mut out = Vec::new();
        let mut bytes = 0usize;
        let mut hit_bytes = false;

        for line in lines.iter().take(max_lines) {
            let size = line.len().saturating_add(usize::from(!out.is_empty()));
            if bytes.saturating_add(size) > max_bytes {
                hit_bytes = true;
                break;
            }
            out.push(*line);
            bytes = bytes.saturating_add(size);
        }

        // Account for the trailing newline that `text.lines()` strips:
        // `text.len()` includes it, but `out.join("\n")` does not.
        let trailing_newline = usize::from(text.ends_with('\n'));
        let kept_bytes = bytes.saturating_add(trailing_newline);
        let removed = total_bytes.saturating_sub(kept_bytes);

        let preview = out.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!(
                "{preview}\n\n...{removed} {unit} truncated...\n\nUse offset/limit or grep to view specific sections."
            ),
            truncated: true,
        }
    }

    /// Truncation from the tail direction
    pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let lines: Vec<&str> = text.lines().collect();
        let total_bytes = text.len();

        if lines.len() <= max_lines && total_bytes <= max_bytes {
            return TruncateResult {
                content: text.to_string(),
                truncated: false,
            };
        }

        let mut out: VecDeque<&str> = VecDeque::new();
        let mut bytes = 0usize;
        let mut hit_bytes = false;

        for line in lines.iter().rev().take(max_lines) {
            let size = line.len().saturating_add(usize::from(!out.is_empty()));
            if bytes.saturating_add(size) > max_bytes {
                hit_bytes = true;
                break;
            }
            out.push_front(*line);
            bytes = bytes.saturating_add(size);
        }

        let removed = if hit_bytes {
            total_bytes.saturating_sub(bytes)
        } else {
            // Account for the trailing newline that `text.lines()` strips.
            let trailing_newline = usize::from(text.ends_with('\n'));
            let kept_bytes = out
                .iter()
                .map(|l| l.len())
                .sum::<usize>()
                .saturating_add(out.len().saturating_sub(1))
                .saturating_add(trailing_newline);
            total_bytes.saturating_sub(kept_bytes)
        };

        let preview: Vec<&str> = out.into();
        let preview = preview.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!("...{removed} {unit} truncated...\n\n{preview}"),
            truncated: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};

    proptest::proptest! {
        /// `simple` never drops or invents leading characters and appends
        /// the ellipsis exactly when the input exceeds the char limit.
        #[test]
        fn simple_respects_char_limit_and_prefix(
            text in "\\PC{0,256}",
            max_chars in 0usize..=64,
        ) {
            let out = Truncate::simple(&text, max_chars);
            let prefix: String = text.chars().take(max_chars).collect();
            prop_assert!(out.starts_with(&prefix));
            if text.chars().count() <= max_chars {
                prop_assert_eq!(out, text);
            } else {
                prop_assert!(out.ends_with("..."));
            }
        }

        /// `detailed` preserves the leading characters and flags truncation.
        #[test]
        fn detailed_respects_char_limit_and_prefix(
            text in "\\PC{0,256}",
            max_chars in 0usize..=64,
        ) {
            let out = Truncate::detailed(&text, max_chars);
            let prefix: String = text.chars().take(max_chars).collect();
            prop_assert!(out.starts_with(&prefix));
            if text.chars().count() <= max_chars {
                prop_assert_eq!(out, text);
            } else {
                prop_assert!(out.contains("truncated"));
            }
        }

        /// `output` truncates exactly when the line or byte bounds are
        /// exceeded, and always reports the truncation in the notice.
        #[test]
        fn output_truncates_iff_over_bounds(
            text in "\\PC{0,256}",
            max_lines in 0usize..=16,
            max_bytes in 0usize..=256,
        ) {
            let result = Truncate::output(&text, max_lines, max_bytes);
            let fits = text.lines().count() <= max_lines && text.len() <= max_bytes;
            prop_assert_eq!(result.truncated, !fits);
            if fits {
                prop_assert_eq!(result.content, text);
            } else {
                prop_assert!(result.content.contains("truncated"));
            }
        }

        /// `tail` truncates exactly when the line or byte bounds are
        /// exceeded, and always reports the truncation in the notice.
        #[test]
        fn tail_truncates_iff_over_bounds(
            text in "\\PC{0,256}",
            max_lines in 0usize..=16,
            max_bytes in 0usize..=256,
        ) {
            let result = Truncate::tail(&text, max_lines, max_bytes);
            let fits = text.lines().count() <= max_lines && text.len() <= max_bytes;
            prop_assert_eq!(result.truncated, !fits);
            if fits {
                prop_assert_eq!(result.content, text);
            } else {
                prop_assert!(result.content.contains("truncated"));
            }
        }
    }

    #[test]
    fn simple_returns_input_under_limit() {
        assert_eq!(Truncate::simple("hello", 10), "hello");
    }

    #[test]
    fn simple_truncates_with_ellipsis_over_limit() {
        assert_eq!(Truncate::simple("hello world", 5), "hello...");
    }

    #[test]
    fn detailed_returns_input_under_limit() {
        assert_eq!(Truncate::detailed("hello", 10), "hello");
    }

    #[test]
    fn detailed_truncates_with_metadata_over_limit() {
        let truncated = Truncate::detailed("hello world", 5);
        assert!(truncated.starts_with("hello"));
        assert!(truncated.contains("truncated"));
        assert!(truncated.contains("11 chars"));
    }

    #[test]
    fn chars_returns_input_under_limit() {
        assert_eq!(Truncate::chars("hello", 10), "hello");
    }

    #[test]
    fn chars_truncates_over_limit() {
        let truncated = Truncate::chars("hello world", 5);
        assert!(truncated.starts_with("hello"));
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn output_returns_full_text_when_within_bounds() {
        let text = "line1\nline2\nline3";
        let result = Truncate::output(text, 10, 1000);
        assert!(!result.truncated);
        assert_eq!(result.content, text);
    }

    #[test]
    fn output_truncates_by_line_count() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = Truncate::output(text, 3, 1000);
        assert!(result.truncated);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn output_truncates_by_byte_size() {
        let text = "this is a very long line that exceeds the byte limit";
        let result = Truncate::output(text, 100, 20);
        assert!(result.truncated);
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn tail_returns_full_text_when_within_bounds() {
        let text = "line1\nline2\nline3";
        let result = Truncate::tail(text, 10, 1000);
        assert!(!result.truncated);
        assert_eq!(result.content, text);
    }

    #[test]
    fn tail_keeps_last_n_lines() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = Truncate::tail(text, 3, 1000);
        assert!(result.truncated);
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("line4"));
        assert!(result.content.contains("line5"));
    }
}
