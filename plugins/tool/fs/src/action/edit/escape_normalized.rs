pub fn escape_normalized_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let unescape = |s: &str| -> String {
        let mut result = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('r') => result.push('\r'),
                    Some('\\') | None => result.push('\\'),
                    Some('"') => result.push('"'),
                    Some('\'') => result.push('\''),
                    Some('`') => result.push('`'),
                    Some('$') => result.push('$'),
                    Some(other) => {
                        result.push('\\');
                        result.push(other);
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    };

    let unescaped_find = unescape(old);

    if content.contains(&unescaped_find) {
        if replace_all {
            return Some(content.replace(&unescaped_find, new));
        }
        let first = content.find(&unescaped_find)?;
        let last = content.rfind(&unescaped_find)?;
        if first != last {
            return None;
        }
        return Some(content[..first].to_string() + new + &content[first + unescaped_find.len()..]);
    }

    let lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = unescaped_find.lines().collect();
    let mut results = Vec::new();

    for i in 0..=lines.len().saturating_sub(find_lines.len().max(1)) {
        let block = lines[i..(i + find_lines.len()).min(lines.len())].join("\n");
        let unescaped_block = unescape(&block);
        if unescaped_block == unescaped_find {
            let start_idx: usize = lines[..i].iter().map(|l| l.len() + 1).sum();
            let block_len = find_lines.len().min(lines.len() - i);
            let end_idx = start_idx
                + lines[i..i + block_len]
                    .iter()
                    .map(|l| l.len())
                    .sum::<usize>()
                + block_len.saturating_sub(1).min(lines.len() - i - 1);
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
    fn test_escape_newline() {
        let content = "foo\nbar\n";
        let result = escape_normalized_replace(content, "foo\\nbar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "QUX\n");
    }

    #[test]
    fn test_escape_tab() {
        let content = "foo\tbar\n";
        let result = escape_normalized_replace(content, "foo\\tbar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "QUX\n");
    }

    #[test]
    fn test_no_escape_needed() {
        let content = "foo bar baz";
        let result = escape_normalized_replace(content, "bar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "foo QUX baz");
    }

    #[test]
    fn test_multiple_matches_reject() {
        let content = "foo\nbar\nfoo\nbar\n";
        let result = escape_normalized_replace(content, "foo\\nbar", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_replace_all() {
        let content = "foo\nbar\nfoo\nbar\n";
        let result = escape_normalized_replace(content, "foo\\nbar", "QUX", true);
        assert!(result.is_some());
    }
}
