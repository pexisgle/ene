pub fn trimmed_boundary_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    let trimmed_find = old.trim();
    if trimmed_find == old { return None; }

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
            if first != last { return None; }
            return Some(content[..first].to_string() + new + &content[first + trimmed_find.len()..]);
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    let find_lines: Vec<&str> = old.lines().collect();

    for i in 0..=lines.len().saturating_sub(find_lines.len()) {
        let block = lines[i..i + find_lines.len()].join("\n");
        if block.trim() == trimmed_find {
            let mut start_idx = 0usize;
            for k in 0..i { start_idx += lines[k].len() + 1; }
            let mut end_idx = start_idx;
            for k in 0..find_lines.len() {
                end_idx += lines[i + k].len();
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
