use std::sync::OnceLock;

static INDENT_RE: OnceLock<regex::Regex> = OnceLock::new();

fn indent_re() -> &'static regex::Regex {
    INDENT_RE.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "constant regex pattern compiled once at first use"
        )]
        regex::Regex::new(r"^(\s*)").expect("invalid constant regex")
    })
}

pub fn indentation_flexible_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let remove_indent = |t: &str| -> String {
        let lines: Vec<&str> = t.lines().collect();
        let non_empty: Vec<&str> = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .copied()
            .collect();
        if non_empty.is_empty() {
            return t.to_string();
        }
        let min_indent = non_empty
            .iter()
            .map(|l| {
                indent_re()
                    .captures(l)
                    .and_then(|c| c.get(1))
                    .map_or(0, |m| m.len())
            })
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    l.to_string()
                } else {
                    l[min_indent.min(l.len())..].to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let normalized_find = remove_indent(old);
    let content_lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = old.lines().collect();

    let mut results = Vec::new();
    for i in 0..=content_lines.len().saturating_sub(find_lines.len()) {
        let block = content_lines[i..i + find_lines.len()].join("\n");
        if remove_indent(&block) == normalized_find {
            let start_idx: usize = content_lines[..i].iter().map(|l| l.len() + 1).sum();
            let end_idx = start_idx
                + content_lines[i..i + find_lines.len()]
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
    fn test_different_indentation() {
        let content = "    fn foo() {\n        println!(\"hello\");\n    }\n";
        // old must have same relative indentation pattern as content block
        let old = "fn foo() {\n    println!(\"hello\");\n}";
        let result = indentation_flexible_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_same_indentation() {
        let content = "fn foo() {\n    println!(\"hello\");\n}\n";
        let old = "fn foo() {\n    println!(\"hello\");\n}";
        let result = indentation_flexible_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "fn bar() {}\n");
    }

    #[test]
    fn test_no_match() {
        let content = "fn foo() {}\n";
        let result = indentation_flexible_replace(content, "fn bar() {}", "fn baz() {}", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_content_indentation() {
        let content = "\n\n";
        let old = "foo";
        let result = indentation_flexible_replace(content, old, "bar", false);
        assert!(result.is_none());
    }
}
