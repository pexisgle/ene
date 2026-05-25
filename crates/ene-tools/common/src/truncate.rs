pub struct Truncate;

/// Result of a truncation operation, indicating whether content was actually cut.
#[derive(Debug, Clone)]
pub struct TruncateResult {
    pub content: String,
    pub truncated: bool,
}

impl Truncate {
    /// Truncates text to a maximum number of Unicode characters.
    /// Appends a `[... truncated, total N chars ...]` notice when cut.
    pub fn chars(text: &str, max_chars: usize) -> String {
        let char_count = text.chars().count();
        if char_count <= max_chars {
            text.to_string()
        } else {
            let byte_end = text
                .char_indices()
                .nth(max_chars)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!(
                "{}\n\n[... truncated, total {} chars ...]",
                &text[..byte_end],
                char_count
            )
        }
    }

    /// テキストを max_lines / max_bytes に収める（head方向）
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
            let size = line.len() + if out.is_empty() { 0 } else { 1 };
            if bytes + size > max_bytes {
                hit_bytes = true;
                break;
            }
            out.push(*line);
            bytes += size;
        }

        let removed = if hit_bytes {
            total_bytes - bytes
        } else {
            text.len() - out.join("\n").len()
        };

        let preview = out.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!(
                "{}\n\n...{} {} truncated...\n\nUse offset/limit or grep to view specific sections.",
                preview, removed, unit
            ),
            truncated: true,
        }
    }

    /// tail方向に切り詰め
    pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
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

        for line in lines.iter().rev().take(max_lines) {
            let size = line.len() + if out.is_empty() { 0 } else { 1 };
            if bytes + size > max_bytes {
                hit_bytes = true;
                break;
            }
            out.insert(0, *line);
            bytes += size;
        }

        let removed = if hit_bytes {
            total_bytes - bytes
        } else {
            text.len() - out.join("\n").len()
        };

        let preview = out.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!("...{} {} truncated...\n\n{}", removed, unit, preview),
            truncated: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chars_no_truncation() {
        assert_eq!(Truncate::chars("hello", 10), "hello");
    }

    #[test]
    fn test_chars_truncation() {
        let truncated = Truncate::chars("hello world", 5);
        assert!(truncated.starts_with("hello"));
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn test_output_no_truncation_needed() {
        let text = "line1\nline2\nline3";
        let result = Truncate::output(text, 10, 1000);
        assert!(!result.truncated);
        assert_eq!(result.content, text);
    }

    #[test]
    fn test_output_by_lines() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = Truncate::output(text, 3, 1000);
        assert!(result.truncated);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn test_output_by_bytes() {
        let text = "this is a very long line that exceeds the byte limit";
        let result = Truncate::output(text, 100, 20);
        assert!(result.truncated);
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn test_tail_no_truncation_needed() {
        let text = "line1\nline2\nline3";
        let result = Truncate::tail(text, 10, 1000);
        assert!(!result.truncated);
        assert_eq!(result.content, text);
    }

    #[test]
    fn test_tail_by_lines() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let result = Truncate::tail(text, 3, 1000);
        assert!(result.truncated);
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("line4"));
        assert!(result.content.contains("line5"));
    }
}
