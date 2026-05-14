pub fn line_trimmed_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    let original_lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = old.lines().collect();

    if search_lines.is_empty() { return None; }

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
            let mut start_idx = 0usize;
            for k in 0..i {
                start_idx += original_lines[k].len() + 1;
            }
            let mut end_idx = start_idx;
            for k in 0..search_lines.len() {
                end_idx += original_lines[i + k].len();
                if k < search_lines.len() - 1 { end_idx += 1; }
            }
            results.push((start_idx, end_idx));
        }
    }

    if results.is_empty() { return None; }
    if !replace_all && results.len() > 1 { return None; }

    let mut result = content.to_string();
    for (start, end) in results.iter().rev() {
        result = result[..*start].to_string() + new + &result[*end..];
    }
    Some(result)
}
