use std::sync::OnceLock;

static WHITESPACE_RE: OnceLock<regex::Regex> = OnceLock::new();

fn whitespace_re() -> &'static regex::Regex {
    WHITESPACE_RE.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"\s+").expect("invalid constant regex")
    })
}

pub fn whitespace_normalized_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let normalize = |t: &str| -> String { whitespace_re().replace_all(t, " ").trim().to_string() };
    let normalized_find = normalize(old);

    let lines: Vec<&str> = content.lines().collect();
    let mut results = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if normalize(line) == normalized_find {
            let start_idx: usize = lines[..i].iter().map(|l| l.len() + 1).sum();
            results.push((start_idx, start_idx + line.len()));
        }
    }

    let find_lines: Vec<&str> = old.lines().collect();
    if find_lines.len() > 1 {
        for i in 0..=lines.len().saturating_sub(find_lines.len()) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if normalize(&block) == normalized_find {
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
    fn test_single_line_whitespace_normalized() {
        let content = "foo  bar\nbaz";
        let result = whitespace_normalized_replace(content, "foo   bar", "QUX", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "QUX\nbaz");
    }

    #[test]
    fn test_multi_line_whitespace_normalized() {
        let content = "  foo\n    bar\n  baz\n";
        let old = "foo\nbar";
        let result = whitespace_normalized_replace(content, old, "QUX", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let content = "foo bar baz";
        let result = whitespace_normalized_replace(content, "qux", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_matches_reject() {
        let content = "foo bar\nfoo bar\n";
        let result = whitespace_normalized_replace(content, "foo bar", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_replace_all() {
        let content = "foo bar\nfoo bar\n";
        let result = whitespace_normalized_replace(content, "foo bar", "QUX", true);
        assert!(result.is_some());
    }
}
