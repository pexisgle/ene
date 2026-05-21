use crate::error::ToolError;

#[derive(Clone)]
pub enum PatchOperation {
    Update {
        path: String,
        old_content: String,
        new_content: String,
    },
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
}

pub fn parse_code_block(lines: &[&str], start: usize) -> Result<(String, usize), ToolError> {
    if start >= lines.len() || !lines[start].trim().starts_with("```") {
        return Err(ToolError::ToolExecutionError(format!(
            "Expected code block starting with ``` at line {}",
            start + 1
        )));
    }

    let mut content = Vec::new();
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "```" {
            return Ok((content.join("\n"), i - start));
        }
        content.push(line);
        i += 1;
    }

    Err(ToolError::ToolExecutionError(
        "Unclosed code block in patch".to_string(),
    ))
}

pub fn parse_update_block(
    lines: &[&str],
    start: usize,
) -> Result<(String, String, usize), ToolError> {
    let (old_content, consumed1) = parse_code_block(lines, start)?;
    let next = start + consumed1 + 1;

    if next >= lines.len() || !lines[next].trim().starts_with("```") {
        return Err(ToolError::ToolExecutionError(format!(
            "Expected second code block for new content at line {}",
            next + 1
        )));
    }
    let (new_content, consumed2) = parse_code_block(lines, next)?;

    Ok((old_content, new_content, consumed1 + 1 + consumed2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_code_block_simple() {
        let lines = vec![
            "```",
            "content line 1",
            "content line 2",
            "```",
            "next line",
        ];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "content line 1\ncontent line 2");
        assert_eq!(consumed, 3);
    }

    #[test]
    fn test_parse_code_block_with_lang() {
        let lines = vec!["```rust", "fn main() {}", "```", "next"];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "fn main() {}");
        assert_eq!(consumed, 2);
    }

    #[test]
    fn test_parse_code_block_empty() {
        let lines = vec!["```", "```", "next"];
        let (content, consumed) = parse_code_block(&lines, 0).unwrap();
        assert_eq!(content, "");
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_parse_code_block_missing_start() {
        let lines = vec!["not a code block", "next"];
        let result = parse_code_block(&lines, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_code_block_unclosed() {
        let lines = vec!["```", "content", "more content"];
        let result = parse_code_block(&lines, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_update_block() {
        let lines = vec!["```", "old content", "```", "```", "new content", "```"];
        let (old, new, consumed) = parse_update_block(&lines, 0).unwrap();
        assert_eq!(old, "old content");
        assert_eq!(new, "new content");
        assert_eq!(consumed, 5);
    }

    #[test]
    fn test_parse_update_block_missing_second_block() {
        let lines = vec!["```", "old content", "```", "not a code block"];
        let result = parse_update_block(&lines, 0);
        assert!(result.is_err());
    }
}
