pub fn line_trimmed_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = old.lines().collect();

    if search_lines.is_empty() {
        return None;
    }

    let mut results = Vec::new();
    for i in 0..=original_lines.len().saturating_sub(search_lines.len()) {
        let mut matches = true;
        for j in 0..search_lines.len() {
            if original_lines[i + j].trim() != search_lines[j].trim() {
                matches = false;
                break;
            }
        }
        if matches {
            let start_idx: usize = original_lines[..i].iter().map(|l| l.len() + 1).sum();
            let end_idx = start_idx
                + original_lines[i..i + search_lines.len()]
                    .iter()
                    .map(|l| l.len())
                    .sum::<usize>()
                + search_lines.len().saturating_sub(1);
            results.push((start_idx, end_idx));
        }
    }

    if results.is_empty() {
        return None;
    }
    if !replace_all && results.len() > 1 {
        return None;
    }

    let mut result = content.to_string();
    for (start, end) in results.iter().rev() {
        result = result[..*start].to_string() + new + &result[*end..];
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match_single_line() {
        let content = "foo\nbar\nbaz\n";
        let result = line_trimmed_replace(content, "bar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "foo\nQUX\nbaz\n");
    }

    #[test]
    fn test_trimmed_match() {
        let content = "  foo\n  bar  \n  baz\n";
        let result = line_trimmed_replace(content, "bar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "  foo\nQUX\n  baz\n");
    }

    #[test]
    fn test_multi_line_match() {
        let content = "foo\nbar\nbaz\nqux\n";
        let result = line_trimmed_replace(content, "bar\nbaz", "REPLACED", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "foo\nREPLACED\nqux\n");
    }

    #[test]
    fn test_no_match() {
        let content = "foo\nbar\nbaz\n";
        let result = line_trimmed_replace(content, "qux", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_matches_reject() {
        let content = "foo\nbar\nbaz\nbar\n";
        let result = line_trimmed_replace(content, "bar", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_replace_all() {
        let content = "foo\nbar\nbaz\nbar\n";
        let result = line_trimmed_replace(content, "bar", "QUX", true);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "foo\nQUX\nbaz\nQUX\n");
    }

    #[test]
    fn test_empty_search() {
        let content = "foo\nbar\n";
        let result = line_trimmed_replace(content, "", "QUX", false);
        assert!(result.is_none());
    }
}
