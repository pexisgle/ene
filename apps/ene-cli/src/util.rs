/// Mask an API key for display, showing at most a short prefix and suffix.
pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return "[not set]".to_string();
    }
    if trimmed.chars().count() <= 8 {
        return "[redacted]".to_string();
    }
    let prefix: String = trimmed.chars().take(3).collect();
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

/// Parse a memory kind string into a [`MemoryKind`](ene_store::MemoryKind).
pub fn parse_memory_kind(s: &str) -> Option<ene_store::MemoryKind> {
    match s {
        "episodic" => Some(ene_store::MemoryKind::Episodic),
        "semantic" => Some(ene_store::MemoryKind::Semantic),
        "user_profile" => Some(ene_store::MemoryKind::UserProfile),
        "relationship" => Some(ene_store::MemoryKind::Relationship),
        "affective" => Some(ene_store::MemoryKind::Affective),
        "commitment" => Some(ene_store::MemoryKind::Commitment),
        "preference" => Some(ene_store::MemoryKind::Preference),
        "procedure" => Some(ene_store::MemoryKind::Procedure),
        "reflection" => Some(ene_store::MemoryKind::Reflection),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_api_key_hides_middle() {
        assert_eq!(mask_api_key(""), "[not set]");
        assert_eq!(mask_api_key("short"), "[redacted]");
        assert_eq!(mask_api_key("sk-abcdefghijklmnop"), "sk-…mnop");
    }

    #[test]
    fn parse_memory_kind_known_values() {
        assert_eq!(
            parse_memory_kind("episodic"),
            Some(ene_store::MemoryKind::Episodic)
        );
        assert_eq!(
            parse_memory_kind("preference"),
            Some(ene_store::MemoryKind::Preference)
        );
        assert_eq!(parse_memory_kind("unknown"), None);
    }
}
