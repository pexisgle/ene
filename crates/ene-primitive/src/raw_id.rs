//! Opaque 128-bit identity shape shared by all domain identifiers.
//!
//! [`RawId`] carries no meaning beyond distinguishing one thing from another.
//! Domain crates wrap it in newtypes such as `CompanionId` or `TaskId`, with
//! no conversion between those newtypes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque 128-bit identity, backed by a random (v4) [`Uuid`].
///
/// A [`RawId`] asserts nothing about the thing it names: not its kind, not its
/// owner, and not whether it still exists. It is meaningful only alongside
/// domain context supplied by the owning crate.
///
/// String forms are intentionally absent: this type provides neither
/// [`core::fmt::Display`] nor [`core::str::FromStr`]. Rendering an identity as
/// text invites matching on string prefixes (for example `"task_"`), which
/// correspondence-identity §4.1 forbids. Distinguish identities by their domain
/// newtype and field name instead; when an identity must appear in logs or
/// audit records, record the owning type name together with the value at the
/// call site.
///
/// ```
/// use ene_primitive::raw_id::RawId;
///
/// let id = RawId::new();
/// assert_eq!(id, RawId::from_uuid(id.as_uuid()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawId(Uuid);

impl RawId {
    /// Generates a fresh random (v4) identity.
    ///
    /// Generation never reuses a value: deleting the named thing must not
    /// recycle its identity for something else.
    ///
    /// ```
    /// use ene_primitive::raw_id::RawId;
    ///
    /// let id = RawId::new();
    /// assert_eq!(id.as_uuid().get_version(), Some(uuid::Version::Random));
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wraps an existing UUID, for example one read back from storage.
    ///
    /// The caller guarantees the value still names the same thing it named
    /// when stored; this constructor performs no registry lookup.
    ///
    /// ```
    /// use ene_primitive::raw_id::RawId;
    /// use uuid::Uuid;
    ///
    /// let uuid = Uuid::new_v4();
    /// assert_eq!(RawId::from_uuid(uuid).as_uuid(), uuid);
    /// ```
    #[must_use]
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the wrapped UUID for storage or transport encoding.
    ///
    /// Encode the owning domain type alongside the value so readers never have
    /// to guess what the bytes name.
    ///
    /// ```
    /// use ene_primitive::raw_id::RawId;
    ///
    /// let id = RawId::new();
    /// assert_eq!(RawId::from_uuid(id.as_uuid()), id);
    /// ```
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for RawId {
    /// Generates a fresh random identity, like [`RawId::new`].
    ///
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
