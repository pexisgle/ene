//! Identity types for external-service connectors.
//!
//! These describe *who* a connector is — its stable identifier, the OAuth
//! permission scopes it requests, and human-readable display metadata — without
//! any connection lifecycle. Lifecycle management (spawning, supervision,
//! restarts) lives in `ene-plugin-host`; this crate is the authority for
//! credentials and identity only.

use crate::error::ConnectorError;
use std::fmt;
use std::str::FromStr;

/// A stable, namespaced identifier for a connector or stored credential.
///
/// The canonical form is `namespace.name` (e.g. `mcp.github-mcp` or
/// `google.calendar`). The namespace is everything before the first `.`, the
/// name everything after it.
///
/// Accepted characters are ASCII alphanumerics plus `.`, `_`, and `-`. The `-`
/// is essential: natural connector names such as `github-mcp` and
/// `google-calendar` are hyphenated. (Its historical absence was the root cause
/// of #417, where the second hyphenated MCP server collided on a fallback id
/// and was silently dropped.)
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ConnectorId(String);

impl ConnectorId {
    /// Returns `true` when `c` is an accepted identifier character.
    const fn valid_char(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-'
    }

    /// Returns `true` when `id` is non-empty, uses only accepted characters,
    /// and contains at least one `.` namespace separator.
    #[must_use]
    pub fn is_valid(id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let bytes = id.as_bytes();
        if !bytes.iter().all(|&b| Self::valid_char(b)) {
            return false;
        }
        bytes.contains(&b'.')
    }

    /// Creates a connector id, validating the charset and namespace form.
    ///
    /// # Errors
    /// Returns [`ConnectorError::Internal`] when `id` is empty, contains a
    /// character outside `[A-Za-z0-9._-]`, or has no `.` separator.
    pub fn try_new(id: impl Into<String>) -> Result<Self, ConnectorError> {
        let s = id.into();
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(ConnectorError::internal(format!(
                "invalid connector ID: '{s}'"
            )))
        }
    }

    /// Returns the full identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the namespace portion (before the first `.`), or the whole id
    /// when there is no separator.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.split_once('.').map_or(&self.0, |(ns, _)| ns)
    }

    /// Returns the name portion (after the first `.`), or `""` when there is
    /// no separator.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.split_once('.').map_or("", |(_, name)| name)
    }
}

impl fmt::Display for ConnectorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ConnectorId {
    type Err = ConnectorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_new(s)
    }
}

impl From<ConnectorId> for String {
    fn from(id: ConnectorId) -> Self {
        id.0
    }
}

/// An OAuth permission scope requested by a connector (e.g. `calendar.readonly`).
///
/// A thin, validated-by-construction wrapper over the scope string; used when
/// describing what access a connector needs and when driving an OAuth consent
/// flow (see #415).
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PermissionScope(String);

impl PermissionScope {
    /// Creates a permission scope from any string.
    #[must_use]
    pub fn new(scope: impl Into<String>) -> Self {
        Self(scope.into())
    }

    /// Returns the scope as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PermissionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for PermissionScope {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl<'a> From<&'a str> for PermissionScope {
    fn from(s: &'a str) -> Self {
        Self(s.to_owned())
    }
}

/// Human-readable display metadata for a connector.
///
/// Intended for configuration UIs: a stable [`ConnectorId`] plus a friendly
/// `display_name` and optional version/vendor/base-URL/description. It carries
/// no secrets and no lifecycle state.
///
/// The declarative connector design in #414 may fold this metadata into a
/// broader declaration; until then it is retained for display purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorIdentity {
    /// Stable connector identifier.
    pub id: ConnectorId,
    /// Human-readable name for display.
    pub display_name: String,
    /// Optional connector/version string.
    pub version: Option<String>,
    /// Optional vendor name.
    pub vendor: Option<String>,
    /// Optional service base URL.
    pub base_url: Option<String>,
    /// Optional longer description.
    pub description: Option<String>,
}

impl ConnectorIdentity {
    /// Creates an identity with a display name and no optional metadata.
    #[must_use]
    pub fn new(id: ConnectorId, display_name: impl Into<String>) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            version: None,
            vendor: None,
            base_url: None,
            description: None,
        }
    }

    /// Sets the version metadata.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Sets the vendor metadata.
    #[must_use]
    pub fn with_vendor(mut self, vendor: impl Into<String>) -> Self {
        self.vendor = Some(vendor.into());
        self
    }

    /// Sets the service base URL metadata.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Sets the description metadata.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap/unwrap_err for concise failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn accepts_alnum_dot_underscore_hyphen() {
        assert!(ConnectorId::is_valid("mcp.github-mcp"));
        assert!(ConnectorId::is_valid("google.calendar_v2"));
        assert!(ConnectorId::is_valid("a.b"));
    }

    #[test]
    fn hyphenated_id_round_trips() {
        // Root cause of #417: hyphenated names must be accepted, not forced
        // onto a colliding fallback id.
        let id = ConnectorId::try_new("mcp.github-mcp").unwrap();
        assert_eq!(id.as_str(), "mcp.github-mcp");
        assert_eq!(id.namespace(), "mcp");
        assert_eq!(id.name(), "github-mcp");
    }

    #[test]
    fn rejects_empty_missing_separator_and_bad_chars() {
        assert!(!ConnectorId::is_valid(""));
        assert!(!ConnectorId::is_valid("noseparator"));
        assert!(!ConnectorId::is_valid("has space.dot"));
        assert!(!ConnectorId::is_valid("semi;colon.dot"));
        assert!(ConnectorId::try_new("slash/name.x").is_err());
    }

    #[test]
    fn namespace_and_name_split_on_first_dot() {
        let id = ConnectorId::try_new("ns.sub.name").unwrap();
        assert_eq!(id.namespace(), "ns");
        assert_eq!(id.name(), "sub.name");
    }

    #[test]
    fn identity_builders_set_optional_fields() {
        let id = ConnectorId::try_new("mcp.demo").unwrap();
        let identity = ConnectorIdentity::new(id, "Demo")
            .with_version("1.0")
            .with_vendor("Acme")
            .with_base_url("https://demo.example.com")
            .with_description("A demo connector");
        assert_eq!(identity.display_name, "Demo");
        assert_eq!(identity.version.as_deref(), Some("1.0"));
        assert_eq!(identity.vendor.as_deref(), Some("Acme"));
        assert_eq!(
            identity.base_url.as_deref(),
            Some("https://demo.example.com")
        );
        assert_eq!(identity.description.as_deref(), Some("A demo connector"));
    }
}
