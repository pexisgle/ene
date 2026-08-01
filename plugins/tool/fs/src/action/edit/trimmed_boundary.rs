pub fn trimmed_boundary_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let trimmed_find = old.trim();
    if trimmed_find == old {
        return None;
    }

    let mut results = Vec::new();

    if content.contains(trimmed_find) {
        if replace_all {
            let mut positions = Vec::new();
            let mut start = 0usize;
            while let Some(pos) = content[start..].find(trimmed_find) {
                let absolute = start + pos;
                positions.push((absolute, absolute + trimmed_find.len()));
                start = absolute + trimmed_find.len();
            }
            if !positions.is_empty() {
                let mut result = content.to_string();
                for (s, e) in positions.iter().rev() {
                    result = result[..*s].to_string() + new + &result[*e..];
                }
                return Some(result);
            }
        } else {
            let first = content.find(trimmed_find)?;
            let last = content.rfind(trimmed_find)?;
            if first != last {
                return None;
            }
            return Some(
                content[..first].to_string() + new + &content[first + trimmed_find.len()..],
            );
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = old.lines().collect();

    for i in 0..=lines.len().saturating_sub(find_lines.len()) {
        let block = lines[i..i + find_lines.len()].join("\n");
        if block.trim() == trimmed_find {
            let start_idx: usize = lines[..i].iter().map(|l| l.len() + 1).sum();
            let end_idx = start_idx
                + lines[i..i + find_lines.len()]
                    .iter()
                    .map(|l| l.len())
                    .sum::<usize>()
                + find_lines.len().saturating_sub(1);
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
    fn test_trimmed_match() {
        let content = "  foo bar  ";
        let result = trimmed_boundary_replace(content, "  foo bar  ", "QUX", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_trimmed_already_no_whitespace() {
        let content = "foo bar";
        let result = trimmed_boundary_replace(content, "foo bar", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_multi_line_trimmed() {
        let content = "  foo\n  bar  \n";
        let old = "  foo\n  bar  ";
        // `old.trim()` doesn't occur verbatim in `content`, so matching falls
        // through to the line-by-line trimmed comparison.
        let result = trimmed_boundary_replace(content, old, "QUX", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let content = "foo bar baz";
        let result = trimmed_boundary_replace(content, "  qux  ", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_replace_all() {
        let content = "  foo  foo  ";
        let result = trimmed_boundary_replace(content, "  foo  ", "QUX", true);
        assert!(result.is_some());
    }
}
