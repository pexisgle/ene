pub fn indentation_flexible_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    let remove_indent = |t: &str| -> String {
        let lines: Vec<&str> = t.lines().collect();
        let non_empty: Vec<&str> = lines.iter().filter(|l| l.trim().len() > 0).copied().collect();
        if non_empty.is_empty() { return t.to_string(); }
        let min_indent = non_empty.iter().map(|l| {
            regex::Regex::new(r"^(\s*)").unwrap().captures(l).and_then(|c| c.get(1)).map(|m| m.len()).unwrap_or(0)
        }).min().unwrap_or(0);
        lines.iter().map(|l| {
            if l.trim().is_empty() { l.to_string() } else { l[min_indent.min(l.len())..].to_string() }
        }).collect::<Vec<_>>().join("\n")
    };

    let normalized_find = remove_indent(old);
    let content_lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = old.lines().collect();

    let mut results = Vec::new();
    for i in 0..=content_lines.len().saturating_sub(find_lines.len()) {
        let block = content_lines[i..i + find_lines.len()].join("\n");
        if remove_indent(&block) == normalized_find {
            let mut start_idx = 0usize;
            for k in 0..i { start_idx += content_lines[k].len() + 1; }
            let mut end_idx = start_idx;
            for k in 0..find_lines.len() {
                end_idx += content_lines[i + k].len();
                if k < find_lines.len() - 1 { end_idx += 1; }
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
