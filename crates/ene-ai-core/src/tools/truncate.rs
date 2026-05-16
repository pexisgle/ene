/// 出力を制限サイズに収める
pub struct Truncate;

impl Truncate {
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
            let size = line.len() + if out.is_empty() { 0 } else { 1 }; // +1 for newline
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

pub struct TruncateResult {
    pub content: String,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_output_empty_text() {
        let result = Truncate::output("", 10, 1000);
        assert!(!result.truncated);
        assert_eq!(result.content, "");
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
        assert!(result.content.starts_with("..."));
    }

    #[test]
    fn test_tail_by_bytes() {
        let text = "line1\nline2\nthis is a very long line";
        let result = Truncate::tail(text, 100, 20);
        assert!(result.truncated);
        assert!(result.content.contains("truncated"));
    }

    #[test]
    fn test_output_single_line_exceeds_bytes() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let result = Truncate::output(text, 10, 10);
        assert!(result.truncated);
    }

    #[test]
    fn test_tail_single_line_exceeds_bytes() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let result = Truncate::tail(text, 10, 10);
        assert!(result.truncated);
    }
}
