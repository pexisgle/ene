//! Map legacy keyfacts to typed `UserProfile` / Preference memories.

use crate::typed_memory::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    MemoryStatus, NewMemoryItem,
};

/// Classify a legacy keyfact key into `UserProfile` or Preference.
pub fn keyfact_kind_for_key(key: &str) -> MemoryKind {
    let lower = key.to_lowercase();
    if lower.starts_with("pref_")
        || lower.contains("like")
        || lower.contains("dislike")
        || lower.contains("favorite")
        || lower.contains("favourite")
    {
        MemoryKind::Preference
    } else {
        MemoryKind::UserProfile
    }
}

/// Build a typed memory item from a legacy keyfact row.
pub fn keyfact_to_typed_memory(
    card_name: &str,
    user_id: &str,
    key: &str,
    value: &str,
    legacy_id: i64,
) -> NewMemoryItem {
    NewMemoryItem {
        scope: MemoryScope::User,
        character_id: card_name.to_string(),
        user_id: user_id.to_string(),
        kind: keyfact_kind_for_key(key),
        title: key.to_string(),
        content: value.to_string(),
        source: MemorySource::Imported,
        source_ref: Some(format!("legacy:keyfact:{legacy_id}")),
        confidence: MemoryConfidence::new(0.7),
        salience: MemorySalience::new(0.5),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_keys() {
        assert_eq!(keyfact_kind_for_key("pref_color"), MemoryKind::Preference);
        assert_eq!(keyfact_kind_for_key("food_like"), MemoryKind::Preference);
        assert_eq!(
            keyfact_kind_for_key("dislikes_spicy"),
            MemoryKind::Preference
        );
    }

    #[test]
    fn profile_keys() {
        assert_eq!(keyfact_kind_for_key("job"), MemoryKind::UserProfile);
        assert_eq!(keyfact_kind_for_key("name"), MemoryKind::UserProfile);
    }
}
