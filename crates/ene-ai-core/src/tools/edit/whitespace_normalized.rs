pub fn whitespace_normalized_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    let normalize = |t: &str| -> String {
        regex::Regex::new(r"\s+").unwrap().replace_all(t, " ").trim().to_string()
    };
    let normalized_find = normalize(old);

    let lines: Vec<&str> = content.lines().collect();
    let mut results = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if normalize(line) == normalized_find {
            let mut start_idx = 0usize;
            for k in 0..i { start_idx += lines[k].len() + 1; }
            results.push((start_idx, start_idx + line.len()));
        }
    }

    let find_lines: Vec<&str> = old.lines().collect();
    if find_lines.len() > 1 {
        for i in 0..=lines.len().saturating_sub(find_lines.len()) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if normalize(&block) == normalized_find {
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
    }

    if results.is_empty() { return None; }
    if !replace_all && results.len() > 1 { return None; }

    let mut result = content.to_string();
    for (start, end) in results.iter().rev() {
        result = result[..*start].to_string() + new + &result[*end..];
    }
    Some(result)
}
