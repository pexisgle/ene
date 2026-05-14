pub fn context_aware_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    let find_lines: Vec<&str> = old.lines().collect();
    if find_lines.len() < 3 { return None; }

    let mut search_lines: Vec<&str> = find_lines.clone();
    if search_lines.last() == Some(&"") { search_lines.pop(); }
    if search_lines.len() < 3 { return None; }

    let first_line = search_lines[0].trim();
    let last_line = search_lines[search_lines.len() - 1].trim();
    let content_lines: Vec<&str> = content.lines().collect();

    let mut results = Vec::new();

    for i in 0..content_lines.len() {
        if content_lines[i].trim() != first_line { continue; }
        for j in (i + 2)..content_lines.len() {
            if content_lines[j].trim() == last_line {
                let block_lines = &content_lines[i..=j];

                if block_lines.len() == search_lines.len() {
                    let mut matching = 0usize;
                    let mut total = 0usize;
                    for k in 1..block_lines.len() - 1 {
                        let bl = block_lines[k].trim();
                        let fl = search_lines[k].trim();
                        if bl.len() > 0 || fl.len() > 0 {
                            total += 1;
                            if bl == fl { matching += 1; }
                        }
                    }
                    if total == 0 || matching as f64 / total as f64 >= 0.5 {
                        let mut start_idx = 0usize;
                        for k in 0..i { start_idx += content_lines[k].len() + 1; }
                        let mut end_idx = start_idx;
                        for k in i..=j {
                            end_idx += content_lines[k].len();
                            if k < j { end_idx += 1; }
                        }
                        results.push((start_idx, end_idx));
                    }
                }
                break;
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
