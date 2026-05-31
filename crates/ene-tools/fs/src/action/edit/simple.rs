pub fn simple_replace(content: &str, old: &str, new: &str, replace_all: bool) -> Option<String> {
    if !content.contains(old) {
        return None;
    }
    if replace_all {
        return Some(content.replace(old, new));
    }
    let first = content.find(old)?;
    let last = content.rfind(old)?;
    if first != last {
        return None;
    }
    Some(content[..first].to_string() + new + &content[first + old.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_occurrence() {
        let content = "foo bar baz";
        let result = simple_replace(content, "bar", "QUX", false);
        assert_eq!(result, Some("foo QUX baz".to_string()));
    }

    #[test]
    fn test_multiple_occurrence_reject() {
        let content = "foo bar bar baz";
        let result = simple_replace(content, "bar", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_replace_all() {
        let content = "foo bar bar baz";
        let result = simple_replace(content, "bar", "QUX", true);
        assert_eq!(result, Some("foo QUX QUX baz".to_string()));
    }

    #[test]
    fn test_not_found() {
        let content = "foo bar baz";
        let result = simple_replace(content, "qux", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_old_string() {
        let content = "foo bar";
        let result = simple_replace(content, "", "QUX", false);
        // empty string matches at multiple positions (before every char), so rejected
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_content() {
        let result = simple_replace("", "foo", "bar", false);
        assert!(result.is_none());
    }
}
