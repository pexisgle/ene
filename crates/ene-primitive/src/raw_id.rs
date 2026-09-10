//! Opaque 128-bit identity shape shared by all domain identifiers.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque 128-bit identity, backed by a random (v4) [`Uuid`].
///
/// String forms are intentionally absent: this type provides neither
/// [`core::fmt::Display`] nor [`core::str::FromStr`]. Rendering an identity as
/// text invites matching on string prefixes (for example `"task_"`), which
/// correspondence-identity §4.1 forbids. Distinguish identities by their domain
/// newtype and field name instead; when an identity must appear in logs or
/// audit records, record the owning type name together with the value at the
/// call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawId(Uuid);

impl RawId {
    /// Identity values are never reused: deleting the named thing does not let
    /// its identity name something else.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The caller guarantees the value still names the same thing it named when
    /// stored; no registry lookup happens here.
    #[must_use]
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for RawId {
    /// There is no nil or otherwise reserved identity: every default is a new
    /// unique value.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::RawId;
    use uuid::Uuid;

    #[test]
    fn wraps_and_returns_the_same_uuid() {
        let uuid = Uuid::new_v4();
        assert_eq!(RawId::from_uuid(uuid).as_uuid(), uuid);
    }

    #[test]
    fn copies_compare_by_value() {
        let id = RawId::new();
        let twin = id;
        assert_eq!(id, twin);
        assert_eq!(RawId::from_uuid(id.as_uuid()), id);
    }

    #[test]
    fn default_generates_a_fresh_identity() {
        let first = RawId::default();
        let second = RawId::default();
        assert_eq!(first, RawId::from_uuid(first.as_uuid()));
        assert_ne!(first, second);
    }
}
