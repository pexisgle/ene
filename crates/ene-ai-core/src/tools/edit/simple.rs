pub fn simple_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    if !content.contains(old) { return None; }
    if replace_all {
        return Some(content.replace(old, new));
    }
    let first = content.find(old)?;
    let last = content.rfind(old)?;
    if first != last { return None; }
    Some(content[..first].to_string() + new + &content[first + old.len()..])
}
