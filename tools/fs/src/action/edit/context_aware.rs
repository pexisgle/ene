pub fn context_aware_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    let find_lines: Vec<&str> = old.lines().collect();
    if find_lines.len() < 3 {
        return None;
    }

    let mut search_lines: Vec<&str> = find_lines.clone();
    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }
    if search_lines.len() < 3 {
        return None;
    }

    let first_line = search_lines[0].trim();
    let last_line = search_lines[search_lines.len() - 1].trim();
    let content_lines: Vec<&str> = content.lines().collect();

    let mut results = Vec::new();

    for (i, line) in content_lines.iter().enumerate() {
        if line.trim() != first_line {
            continue;
        }
        for (j, line) in content_lines.iter().enumerate().skip(i + 2) {
            if line.trim() == last_line {
                let block_lines = &content_lines[i..=j];

                if block_lines.len() == search_lines.len() {
                    let mut matching = 0usize;
                    let mut total = 0usize;
                    for k in 1..block_lines.len() - 1 {
                        let bl = block_lines[k].trim();
                        let fl = search_lines[k].trim();
                        if !bl.is_empty() || !fl.is_empty() {
                            total += 1;
                            if bl == fl {
                                matching += 1;
                            }
                        }
                    }
                    if total == 0 || matching as f64 / total as f64 >= 0.5 {
                        let start_idx: usize = content_lines[..i].iter().map(|l| l.len() + 1).sum();
                        let end_idx = start_idx
                            + content_lines[i..=j].iter().map(|l| l.len()).sum::<usize>()
                            + (j - i);
                        results.push((start_idx, end_idx));
                    }
                }
                break;
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
    fn test_context_match_with_similar_middle() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let old = "fn foo() {\n    let x = 1;\n    let y = 3;\n}";
        let result = context_aware_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_too_few_lines() {
        let content = "foo\nbar\n";
        let old = "foo\nbar\n";
        let result = context_aware_replace(content, old, "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_no_match_first_line() {
        let content = "fn foo() {}\n";
        let old = "fn bar() {\n    let x = 1;\n    let y = 2;\n}";
        let result = context_aware_replace(content, old, "fn baz() {}", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_exact_match() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let old = "fn foo() {\n    let x = 1;\n    let y = 2;\n}";
        let result = context_aware_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "fn bar() {}\n");
    }
}
