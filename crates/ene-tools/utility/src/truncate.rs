pub struct Truncate;

pub struct TruncateResult {
    pub content: String,
    pub truncated: bool,
}

impl Truncate {
    /// テキストを max_lines / max_bytes に収める（head方向）
    pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let res = ene_tools_common::truncate::Truncate::output(text, max_lines, max_bytes);
        TruncateResult {
            content: res.content,
            truncated: res.truncated,
        }
    }

    /// tail方向に切り詰め
    pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let res = ene_tools_common::truncate::Truncate::tail(text, max_lines, max_bytes);
        TruncateResult {
            content: res.content,
            truncated: res.truncated,
        }
    }
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
