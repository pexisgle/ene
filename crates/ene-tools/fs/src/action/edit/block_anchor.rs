use super::levenshtein;

pub fn block_anchor_replace(
    content: &str,
    old: &str,
    new: &str,
    _replace_all: bool,
) -> Option<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let mut search_lines: Vec<&str> = old.lines().collect();

    if search_lines.len() < 3 {
        return None;
    }
    if search_lines.last() == Some(&"") {
        search_lines.pop();
    }
    if search_lines.len() < 3 {
        return None;
    }

    let first_line = search_lines[0].trim();
    let last_line = search_lines[search_lines.len() - 1].trim();
    let search_block_size = search_lines.len();

    let mut candidates = Vec::new();
    for i in 0..original_lines.len() {
        if original_lines[i].trim() != first_line {
            continue;
        }
        for j in (i + 2)..original_lines.len() {
            if original_lines[j].trim() == last_line {
                candidates.push((i, j));
                break;
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    let single_threshold = 0.0f64;
    let multi_threshold = 0.3f64;

    let check_candidate = |start_line: usize, end_line: usize| -> Option<(usize, usize)> {
        let actual_block_size = end_line - start_line + 1;
        let lines_to_check = (search_block_size - 2).min(actual_block_size - 2);
        let mut similarity = 0.0f64;
        if lines_to_check > 0 {
            for j in 1..search_block_size - 1 {
                if j >= actual_block_size - 1 {
                    break;
                }
                let original_line = original_lines[start_line + j].trim();
                let search_line = search_lines[j].trim();
                let max_len = original_line.len().max(search_line.len());
                if max_len == 0 {
                    continue;
                }
                let distance = levenshtein(original_line, search_line);
                similarity += (1.0 - distance as f64 / max_len as f64) / lines_to_check as f64;
                if similarity >= single_threshold {
                    break;
                }
            }
        } else {
            similarity = 1.0;
        }
        if similarity >= single_threshold {
            Some((start_line, end_line))
        } else {
            None
        }
    };

    if candidates.len() == 1 {
        let (s, e) = candidates[0];
        check_candidate(s, e)?;
        let mut start_idx = 0usize;
        for k in 0..s {
            start_idx += original_lines[k].len() + 1;
        }
        let mut end_idx = start_idx;
        for k in s..=e {
            end_idx += original_lines[k].len();
            if k < e {
                end_idx += 1;
            }
        }
        return Some(content[..start_idx].to_string() + new + &content[end_idx..]);
    }

    let mut best: Option<(usize, usize)> = None;
    let mut max_similarity = -1.0f64;

    for (s, e) in candidates {
        if let Some((_, _)) = check_candidate(s, e) {
            let actual_block_size = e - s + 1;
            let lines_to_check = (search_block_size - 2).min(actual_block_size - 2);
            let mut similarity = 0.0f64;
            if lines_to_check > 0 {
                for j in 1..search_block_size - 1 {
                    if j >= actual_block_size - 1 {
                        break;
                    }
                    let original_line = original_lines[s + j].trim();
                    let search_line = search_lines[j].trim();
                    let max_len = original_line.len().max(search_line.len());
                    if max_len == 0 {
                        continue;
                    }
                    let distance = levenshtein(original_line, search_line);
                    similarity += 1.0 - distance as f64 / max_len as f64;
                }
                similarity /= lines_to_check as f64;
            } else {
                similarity = 1.0;
            }
            if similarity > max_similarity {
                max_similarity = similarity;
                best = Some((s, e));
            }
        }
    }

    if max_similarity < multi_threshold {
        return None;
    }
    let (s, e) = best?;
    let mut start_idx = 0usize;
    for k in 0..s {
        start_idx += original_lines[k].len() + 1;
    }
    let mut end_idx = start_idx;
    for k in s..=e {
        end_idx += original_lines[k].len();
        if k < e {
            end_idx += 1;
        }
    }
    Some(content[..start_idx].to_string() + new + &content[end_idx..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_block_match() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let old = "fn foo() {\n    let x = 1;\n    let y = 2;\n}";
        let result = block_anchor_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "fn bar() {}\n");
    }

    #[test]
    fn test_too_few_lines() {
        let content = "foo\nbar\n";
        let old = "foo\nbar\n";
        let result = block_anchor_replace(content, old, "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_similar_middle_lines() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 3;\n}\n";
        let old = "fn foo() {\n    let x = 1;\n    let y = 2;\n}";
        let result = block_anchor_replace(content, old, "fn bar() {}", false);
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let content = "fn foo() {}\n";
        let old = "fn bar() {\n    let x = 1;\n    let y = 2;\n}";
        let result = block_anchor_replace(content, old, "fn baz() {}", false);
        assert!(result.is_none());
    }
}
