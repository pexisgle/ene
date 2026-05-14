pub fn multi_occurrence_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    if !replace_all { return None; }
    if !content.contains(old) { return None; }
    Some(content.replace(old, new))
}
