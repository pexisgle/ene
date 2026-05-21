pub fn multi_occurrence_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<String> {
    if !replace_all {
        return None;
    }
    if !content.contains(old) {
        return None;
    }
    Some(content.replace(old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_all() {
        let content = "foo bar foo bar";
        let result = multi_occurrence_replace(content, "foo", "QUX", true);
        assert_eq!(result, Some("QUX bar QUX bar".to_string()));
    }

    #[test]
    fn test_not_replace_all() {
        let content = "foo bar foo bar";
        let result = multi_occurrence_replace(content, "foo", "QUX", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_not_found() {
        let result = multi_occurrence_replace("foo bar", "qux", "QUX", true);
        assert!(result.is_none());
    }
}
